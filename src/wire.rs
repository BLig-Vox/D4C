//! WebSocket writes retain queued bytes across temporary socket backpressure.
use std::{
    io::{self, Read, Write},
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket};
pub fn send<S: Read + Write>(
    socket: &mut WebSocket<S>,
    message: Message,
) -> tungstenite::Result<()> {
    send_with_timeout(socket, message, Duration::from_secs(30))
}
fn send_with_timeout<S: Read + Write>(
    socket: &mut WebSocket<S>,
    message: Message,
    budget: Duration,
) -> tungstenite::Result<()> {
    let deadline = Instant::now() + budget;
    let mut result = socket.send(message);
    loop {
        match result {
            Ok(()) => return Ok(()),
            Err(tungstenite::Error::Io(ref error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "WebSocket write recovery timed out",
                    )
                    .into());
                }
                std::thread::sleep(Duration::from_millis(10));
                // Tungstenite retains the frame after an I/O failure. Resending
                // the message here would duplicate application commands.
                result = socket.flush();
            }
            Err(error) => return Err(error),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tungstenite::protocol::Role;
    struct SlowWriter {
        bytes: Vec<u8>,
        writes: usize,
        fail_at: usize,
        kind: io::ErrorKind,
    }
    impl Read for SlowWriter {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::WouldBlock.into())
        }
    }
    impl Write for SlowWriter {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            if self.fail_at == 0 || self.writes == self.fail_at {
                return Err(self.kind.into());
            }
            let n = b.len().min(3);
            self.bytes.extend_from_slice(&b[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    fn socket(fail_at: usize, kind: io::ErrorKind) -> WebSocket<SlowWriter> {
        WebSocket::from_raw_socket(
            SlowWriter {
                bytes: vec![],
                writes: 0,
                fail_at,
                kind,
            },
            Role::Server,
            None,
        )
    }
    #[test]
    fn would_block_keeps_one_frame() {
        let mut ws = socket(1, io::ErrorKind::WouldBlock);
        send(&mut ws, Message::Text("hello".into())).unwrap();
        assert_eq!(ws.get_ref().bytes, b"\x81\x05hello");
    }
    #[test]
    fn timeout_after_partial_write_keeps_one_frame() {
        let mut ws = socket(2, io::ErrorKind::TimedOut);
        send(&mut ws, Message::Text("hello".into())).unwrap();
        assert_eq!(ws.get_ref().bytes, b"\x81\x05hello");
    }
    #[test]
    fn permanently_blocked_writer_has_a_deadline() {
        let mut ws = socket(0, io::ErrorKind::WouldBlock);
        assert!(send_with_timeout(&mut ws, Message::Text("hello".into()), Duration::ZERO).is_err());
        assert_eq!(ws.get_ref().writes, 1);
    }
    #[test]
    fn fatal_disconnect_is_not_retried() {
        let mut ws = socket(1, io::ErrorKind::BrokenPipe);
        assert!(send(&mut ws, Message::Text("hello".into())).is_err());
        assert_eq!(ws.get_ref().writes, 1);
    }
}
