#![cfg(target_os = "linux")]

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use vox_d4c_server::rfb::{Decoder, Session, key_event, pointer_event};
use x11rb::{
    connection::Connection,
    protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt},
};

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct TestDirectory(PathBuf);

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn frame(session: &mut Session, decoder: &mut Decoder, incremental: bool) -> bool {
    let updates = decoder.updates;
    session.request(incremental).unwrap();
    let mut bytes = [0; 65536];
    let mut changed = false;
    while decoder.updates == updates {
        let count = session.socket.read(&mut bytes).unwrap();
        assert_ne!(count, 0);
        changed |= decoder.feed(&bytes[..count]).unwrap();
    }
    changed
}

#[test]
#[ignore = "requires Xvfb; run with --ignored"]
fn native_x11_screen_and_input() {
    let xvfb = std::env::var_os("D4C_TEST_XVFB").unwrap_or_else(|| "Xvfb".into());
    let mut x = Process(
        Command::new(xvfb)
            .args([
                "-displayfd",
                "1",
                "-screen",
                "0",
                "320x240x24",
                "-nolisten",
                "tcp",
                "-noreset",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut number = String::new();
    BufReader::new(x.0.stdout.take().unwrap())
        .read_line(&mut number)
        .unwrap();
    let display = format!(":{}", number.trim());
    let (conn, screen) = x11rb::connect(Some(&display)).unwrap();
    let root = conn.setup().roots[screen].root;
    let paint = |color| {
        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().background_pixel(color),
        )
        .unwrap()
        .check()
        .unwrap();
        conn.clear_area(false, root, 0, 0, 0, 0)
            .unwrap()
            .check()
            .unwrap();
    };
    paint(0x2255cc);

    let dir = std::env::temp_dir().join(format!("d4c-native-{}", uuid::Uuid::new_v4()));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    let dir = TestDirectory(dir);
    let socket = dir.0.join("host-screen.sock");
    let mut server = Process(
        Command::new(env!("CARGO_BIN_EXE_vox-d4c-server"))
            .arg("--native-screen")
            .arg(&socket)
            .env("DISPLAY", display)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "native server exited"
        );
        assert!(Instant::now() < deadline, "no native socket");
        thread::sleep(Duration::from_millis(10));
    }
    let mut session = Session::connect_local(&socket).unwrap();
    assert_eq!(
        std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    session
        .socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut decoder = Decoder::new(session.width, session.height).unwrap();
    assert_eq!((session.width, session.height), (320, 240));
    assert!(frame(&mut session, &mut decoder, false));
    assert_eq!(&decoder.rgba[..4], &[0x22, 0x55, 0xcc, 0xff]);
    assert!(!frame(&mut session, &mut decoder, true));
    paint(0x33aa66);
    assert!(frame(&mut session, &mut decoder, true));
    assert_eq!(&decoder.rgba[..4], &[0x33, 0xaa, 0x66, 0xff]);

    session
        .socket
        .write_all(&pointer_event(120, 90, 1))
        .unwrap();
    session
        .socket
        .write_all(&key_event(b'A'.into(), true))
        .unwrap();
    frame(&mut session, &mut decoder, true); // Wait until the server processes both inputs.
    let pointer = conn.query_pointer(root).unwrap().reply().unwrap();
    assert_eq!((pointer.root_x, pointer.root_y), (120, 90));
    assert!(
        u16::from(pointer.mask) & 0x101 == 0x101,
        "left button and Shift held"
    );
    let shift = || u16::from(conn.query_pointer(root).unwrap().reply().unwrap().mask) & 0x101;
    drop(session);
    let deadline = Instant::now() + Duration::from_secs(3);
    while shift() != 0 {
        assert!(
            Instant::now() < deadline,
            "held input was not released on disconnect"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
