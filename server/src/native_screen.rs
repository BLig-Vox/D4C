//! Local RFB bridge for an explicitly shared X11 session. No TCP listener.
use crate::rfb::{MAX_HEIGHT, MAX_WIDTH};
use flate2::{Compress, Compression, FlushCompress};
use std::{
    collections::HashMap,
    error::Error,
    fs,
    io::{self, Read, Write},
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{
        xproto::{self, ConnectionExt, ImageFormat, ImageOrder},
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const FORMAT: [u8; 16] = [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0];
const FRAME_INTERVAL: Duration = Duration::from_millis(67);
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CLIENTS: usize = 8;

pub fn serve(path: &Path) -> Result<()> {
    let dir = path.parent().ok_or("Missing socket directory")?;
    let dir = fs::metadata(dir)?;
    // SAFETY: geteuid does not access any caller-owned memory.
    if !dir.is_dir() || dir.uid() != unsafe { libc::geteuid() } || dir.mode() & 0o077 != 0 {
        return Err("Native screen socket requires a directory private to this user (0700)".into());
    }
    XScreen::open()?;
    let listener = UnixListener::bind(path)?;
    let _cleanup = SocketFile(path);
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    let clients = Arc::new(AtomicUsize::new(0));
    for socket in listener.incoming() {
        let socket = socket?;
        if clients.load(Ordering::Relaxed) >= MAX_CLIENTS {
            continue;
        }
        clients.fetch_add(1, Ordering::Relaxed);
        let clients = clients.clone();
        thread::spawn(move || {
            if let Err(err) = handle(socket) {
                eprintln!("Native screen: {err}");
            }
            clients.fetch_sub(1, Ordering::Relaxed);
        });
    }
    Ok(())
}

struct SocketFile<'a>(&'a Path);

impl Drop for SocketFile<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0);
    }
}

fn take<const N: usize>(socket: &mut UnixStream) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    socket.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn handle(mut socket: UnixStream) -> Result<()> {
    socket.set_read_timeout(Some(IO_TIMEOUT))?;
    socket.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut screen = XScreen::open()?;
    let mut size = screen.size()?;
    socket.write_all(b"RFB 003.008\n")?;
    if take::<12>(&mut socket)? != *b"RFB 003.008\n" {
        return Err("Expected RFB 3.8".into());
    }
    // The enclosing 0700 directory and 0600 socket authenticate local peers.
    socket.write_all(&[1, 1])?;
    if take::<1>(&mut socket)? != [1] {
        return Err("Unsupported RFB security type".into());
    }
    socket.write_all(&[0; 4])?;
    take::<1>(&mut socket)?;
    let name = b"D4C Native X11";
    let mut init = Vec::new();
    init.extend(size.0.to_be_bytes());
    init.extend(size.1.to_be_bytes());
    init.extend(FORMAT);
    init.extend((name.len() as u32).to_be_bytes());
    init.extend(name);
    socket.write_all(&init)?;

    let mut cached = None;
    let mut resize = false;
    let mut use_zlib = false;
    let mut encoder = Compress::new(Compression::fast(), true);
    let mut compressed = Vec::new();
    let mut next_frame = Instant::now();
    loop {
        // Input connections can sit idle indefinitely; bound reads once a message starts.
        socket.set_read_timeout(None)?;
        let message = match take::<1>(&mut socket) {
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            other => other?[0],
        };
        socket.set_read_timeout(Some(IO_TIMEOUT))?;
        match message {
            0 => {
                let msg = take::<19>(&mut socket)?;
                if msg[3..] != FORMAT {
                    return Err("Native screen requires BGRX32".into());
                }
            }
            2 => {
                let [_, hi, lo] = take(&mut socket)?;
                let count = u16::from_be_bytes([hi, lo]);
                if count > 256 {
                    return Err("Too many RFB encodings".into());
                }
                resize = false;
                use_zlib = false;
                for _ in 0..count {
                    match i32::from_be_bytes(take(&mut socket)?) {
                        -223 => resize = true,
                        6 => use_zlib = true,
                        _ => {}
                    }
                }
            }
            3 => {
                let [incremental, a, b, c, d, e, f, g, h] = take(&mut socket)?;
                let x = u16::from_be_bytes([a, b]);
                let y = u16::from_be_bytes([c, d]);
                let w = u16::from_be_bytes([e, f]);
                let h = u16::from_be_bytes([g, h]);
                thread::sleep(next_frame.saturating_duration_since(Instant::now()));
                next_frame = Instant::now() + FRAME_INTERVAL;
                let current = screen.size()?;
                if current != size {
                    if !resize {
                        return Err("X11 screen size changed; reconnect to resize".into());
                    }
                    size = current;
                    cached = None;
                    rectangle(&mut socket, (0, 0, size.0, size.1), -223)?;
                    continue;
                }
                let area = (
                    x,
                    y,
                    w.min(size.0.saturating_sub(x)),
                    h.min(size.1.saturating_sub(y)),
                );
                if area.2 == 0 || area.3 == 0 {
                    socket.write_all(&[0; 4])?;
                    continue;
                }
                let pixels = screen.pixels(area)?;
                if incremental != 0
                    && cached.as_ref().is_some_and(|(last_area, last_pixels)| {
                        *last_area == area && *last_pixels == pixels
                    })
                {
                    socket.write_all(&[0; 4])?;
                    continue;
                }
                if use_zlib {
                    compressed.clear();
                    // Room for the worst-case expansion of an incompressible frame and sync flush.
                    compressed.reserve(pixels.len() + pixels.len() / 100 + 64);
                    let before = encoder.total_in();
                    encoder.compress_vec(&pixels, &mut compressed, FlushCompress::Sync)?;
                    if encoder.total_in() - before != pixels.len() as u64
                        || compressed.len() == compressed.capacity()
                    {
                        return Err("Incomplete RFB zlib frame".into());
                    }
                    rectangle(&mut socket, area, 6)?;
                    socket.write_all(&(compressed.len() as u32).to_be_bytes())?;
                    socket.write_all(&compressed)?;
                } else {
                    rectangle(&mut socket, area, 0)?;
                    socket.write_all(&pixels)?;
                }
                cached = Some((area, pixels));
            }
            4 => {
                let [down, _, _, a, b, c, d] = take(&mut socket)?;
                screen.key(u32::from_be_bytes([a, b, c, d]), down != 0)?;
            }
            5 => {
                let [buttons, a, b, c, d] = take(&mut socket)?;
                let x = u16::from_be_bytes([a, b]).min(size.0 - 1);
                let y = u16::from_be_bytes([c, d]).min(size.1 - 1);
                screen.pointer(x, y, buttons)?;
            }
            _ => return Err(format!("Unsupported native RFB message {message}").into()),
        }
    }
}

type Area = (u16, u16, u16, u16);

fn rectangle(socket: &mut UnixStream, (x, y, w, h): Area, encoding: i32) -> io::Result<()> {
    let mut header = [0; 16];
    header[3] = 1;
    header[4..6].copy_from_slice(&x.to_be_bytes());
    header[6..8].copy_from_slice(&y.to_be_bytes());
    header[8..10].copy_from_slice(&w.to_be_bytes());
    header[10..12].copy_from_slice(&h.to_be_bytes());
    header[12..16].copy_from_slice(&encoding.to_be_bytes());
    socket.write_all(&header)
}

struct XScreen {
    conn: RustConnection,
    root: u32,
    keys: HashMap<u32, (u8, bool)>,
    held: HashMap<u32, (u8, bool)>,
    implicit_shift: Option<u8>,
    buttons: u8,
}

impl XScreen {
    fn open() -> Result<Self> {
        let (conn, index) = x11rb::connect(None)?;
        let setup = conn.setup();
        let screen = &setup.roots[index];
        let visual = screen
            .allowed_depths
            .iter()
            .flat_map(|depth| &depth.visuals)
            .find(|visual| visual.visual_id == screen.root_visual)
            .ok_or("Missing X11 root visual")?;
        if setup.image_byte_order != ImageOrder::LSB_FIRST
            || (visual.red_mask, visual.green_mask, visual.blue_mask) != (0xff0000, 0xff00, 0xff)
            || !setup
                .pixmap_formats
                .iter()
                .any(|format| format.depth == screen.root_depth && format.bits_per_pixel == 32)
        {
            return Err(
                "Unsupported X11 framebuffer format (expected little-endian 32-bit RGB)".into(),
            );
        }
        conn.xtest_get_version(2, 2)?.reply()?;
        let min = setup.min_keycode;
        let map = conn
            .get_keyboard_mapping(min, setup.max_keycode - min + 1)?
            .reply()?;
        let mut keys = HashMap::new();
        for (i, pair) in map
            .keysyms
            .chunks_exact(map.keysyms_per_keycode as usize)
            .enumerate()
        {
            for (level, &symbol) in pair.iter().take(2).enumerate() {
                if symbol != 0 {
                    let entry = keys.entry(symbol).or_insert((min + i as u8, level == 1));
                    if level == 0 && entry.1 {
                        *entry = (min + i as u8, false);
                    }
                }
            }
        }
        let root = screen.root;
        let screen = Self {
            conn,
            root,
            keys,
            held: HashMap::new(),
            implicit_shift: None,
            buttons: 0,
        };
        screen.size()?;
        Ok(screen)
    }

    fn size(&self) -> Result<(u16, u16)> {
        let geometry = self.conn.get_geometry(self.root)?.reply()?;
        if geometry.width == 0
            || geometry.height == 0
            || geometry.width > MAX_WIDTH
            || geometry.height > MAX_HEIGHT
        {
            return Err(format!(
                "X11 screen is {}x{}; D4C supports up to {MAX_WIDTH}x{MAX_HEIGHT}",
                geometry.width, geometry.height
            )
            .into());
        }
        Ok((geometry.width, geometry.height))
    }

    fn pixels(&self, (x, y, w, h): Area) -> Result<Vec<u8>> {
        let image = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                x as i16,
                y as i16,
                w,
                h,
                u32::MAX,
            )?
            .reply()?;
        if image.data.len() != usize::from(w) * usize::from(h) * 4 {
            return Err("Unexpected X11 image size".into());
        }
        Ok(image.data)
    }

    fn fake(&self, event: u8, detail: u8, x: u16, y: u16) -> Result<()> {
        self.conn
            .xtest_fake_input(event, detail, 0, self.root, x as i16, y as i16, 0)?
            .check()?;
        Ok(())
    }

    fn pointer(&mut self, x: u16, y: u16, buttons: u8) -> Result<()> {
        self.fake(xproto::MOTION_NOTIFY_EVENT, 0, x, y)?;
        for n in 0..7 {
            let mask = 1 << n;
            if (buttons ^ self.buttons) & mask != 0 {
                let pressed = buttons & mask != 0;
                self.fake(
                    if pressed {
                        xproto::BUTTON_PRESS_EVENT
                    } else {
                        xproto::BUTTON_RELEASE_EVENT
                    },
                    n + 1,
                    0,
                    0,
                )?;
                self.buttons ^= mask;
            }
        }
        Ok(())
    }

    fn key(&mut self, symbol: u32, down: bool) -> Result<()> {
        if down {
            let Some(&(code, shifted)) = self.keys.get(&symbol) else {
                return Ok(());
            };
            if shifted
                && self.implicit_shift.is_none()
                && !self
                    .conn
                    .query_pointer(self.root)?
                    .reply()?
                    .mask
                    .contains(xproto::KeyButMask::SHIFT)
                && let Some(&(shift, _)) = self.keys.get(&0xffe1)
            {
                self.fake(xproto::KEY_PRESS_EVENT, shift, 0, 0)?;
                self.implicit_shift = Some(shift);
            }
            self.fake(xproto::KEY_PRESS_EVENT, code, 0, 0)?;
            self.held.insert(symbol, (code, shifted));
        } else if let Some((code, _)) = self.held.remove(&symbol) {
            if self.implicit_shift != Some(code)
                && !self.held.values().any(|&(held, _)| held == code)
            {
                self.fake(xproto::KEY_RELEASE_EVENT, code, 0, 0)?;
            }
            if !self.held.values().any(|&(_, shifted)| shifted)
                && let Some(shift) = self.implicit_shift.take()
                && !self.held.values().any(|&(held, _)| held == shift)
            {
                self.fake(xproto::KEY_RELEASE_EVENT, shift, 0, 0)?;
            }
        }
        Ok(())
    }
}

impl Drop for XScreen {
    fn drop(&mut self) {
        let mut keys: Vec<u8> = self.held.values().map(|&(code, _)| code).collect();
        keys.extend(self.implicit_shift);
        keys.sort_unstable();
        keys.dedup();
        for code in keys {
            let _ = self.fake(xproto::KEY_RELEASE_EVENT, code, 0, 0);
        }
        for n in 0..7 {
            if self.buttons & (1 << n) != 0 {
                let _ = self.fake(xproto::BUTTON_RELEASE_EVENT, n + 1, 0, 0);
            }
        }
    }
}
