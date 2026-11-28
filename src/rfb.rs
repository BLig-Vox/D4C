//! Small bounded RFB 3.8 client for D4C-managed private Xvnc servers.
//! Protocol reference: RFC 6143; pixel format is explicitly negotiated BGRX32.
use flate2::{Decompress, FlushDecompress, Status};
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    time::Duration,
};
pub const MAX_WIDTH: u16 = 2560;
pub const MAX_HEIGHT: u16 = 1600;
const MAX_BUFFER: usize = 32 * 1024 * 1024;
pub trait RfbStream: Read + Write + Send {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
}
impl RfbStream for TcpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        TcpStream::set_read_timeout(self, timeout)
    }
}
#[cfg(unix)]
impl RfbStream for std::os::unix::net::UnixStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        std::os::unix::net::UnixStream::set_read_timeout(self, timeout)
    }
}
pub struct Session {
    pub socket: Box<dyn RfbStream>,
    pub width: u16,
    pub height: u16,
    pub name: String,
}
fn err(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn u16be(v: &[u8]) -> u16 {
    u16::from_be_bytes(v[..2].try_into().unwrap())
}
fn u32be(v: &[u8]) -> u32 {
    u32::from_be_bytes(v[..4].try_into().unwrap())
}
fn dimensions(w: u16, h: u16) -> io::Result<usize> {
    if w == 0 || h == 0 || w > MAX_WIDTH || h > MAX_HEIGHT {
        return Err(err("Framebuffer size exceeds limit"));
    }
    Ok(w as usize * h as usize * 4)
}

pub(crate) struct Rectangle {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub encoding: i32,
    pub header_len: usize,
    pub payload_len: usize,
}

pub(crate) fn rectangle(header: &[u8]) -> io::Result<Option<Rectangle>> {
    if header.len() < 12 {
        return Ok(None);
    }
    let width = u16be(&header[4..]);
    let height = u16be(&header[6..]);
    let encoding = i32::from_be_bytes(header[8..12].try_into().unwrap());
    let (header_len, payload_len) = match encoding {
        0 => {
            if width > MAX_WIDTH || height > MAX_HEIGHT {
                return Err(err("Rectangle too large"));
            }
            (12, width as usize * height as usize * 4)
        }
        6 => {
            if header.len() < 16 {
                return Ok(None);
            }
            let size = u32be(&header[12..]) as usize;
            if size > MAX_BUFFER {
                return Err(err("Compressed rectangle too large"));
            }
            (16, size)
        }
        -223 => {
            dimensions(width, height)?;
            (12, 0)
        }
        _ => return Err(err("Unsupported rectangle encoding")),
    };
    Ok(Some(Rectangle {
        x: u16be(header),
        y: u16be(&header[2..]),
        width,
        height,
        encoding,
        header_len,
        payload_len,
    }))
}
impl Session {
    pub fn connect(port: u16) -> io::Result<Self> {
        let s = TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_secs(4),
        )?;
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        s.set_write_timeout(Some(Duration::from_secs(5)))?;
        s.set_nodelay(true)?;
        Self::handshake(Box::new(s))
    }
    #[cfg(unix)]
    pub fn connect_local(path: &std::path::Path) -> io::Result<Self> {
        let s = std::os::unix::net::UnixStream::connect(path)?;
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        s.set_write_timeout(Some(Duration::from_secs(5)))?;
        Self::handshake(Box::new(s))
    }
    fn handshake(mut s: Box<dyn RfbStream>) -> io::Result<Self> {
        let mut version = [0; 12];
        s.read_exact(&mut version)?;
        if &version[..8] != b"RFB 003." {
            return Err(err("Invalid RFB banner"));
        }
        s.write_all(b"RFB 003.008\n")?;
        let mut n = [0];
        s.read_exact(&mut n)?;
        if n[0] == 0 {
            let mut length = [0; 4];
            s.read_exact(&mut length)?;
            let len = u32be(&length) as usize;
            if len == 0 || len > 4096 {
                return Err(err("VNC rejected connection"));
            }
            let mut reason = vec![0; len];
            s.read_exact(&mut reason)?;
            return Err(err(&format!(
                "VNC rejected connection: {}",
                String::from_utf8_lossy(&reason)
            )));
        }
        let mut types = vec![0; n[0] as usize];
        s.read_exact(&mut types)?;
        if !types.contains(&1) {
            return Err(err("Managed VNC must use private-loopback None security"));
        }
        s.write_all(&[1])?;
        let mut result = [0; 4];
        s.read_exact(&mut result)?;
        if u32be(&result) != 0 {
            return Err(err("VNC authentication failed"));
        }
        s.write_all(&[1])?;
        let mut init = [0; 24];
        s.read_exact(&mut init)?;
        let width = u16be(&init);
        let height = u16be(&init[2..]);
        dimensions(width, height)?;
        let len = u32be(&init[20..]) as usize;
        if len > 16384 {
            return Err(err("VNC desktop name too large"));
        }
        let mut name = vec![0; len];
        s.read_exact(&mut name)?;
        s.write_all(&[
            0, 0, 0, 0, 32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0,
        ])?;
        let mut enc = vec![2, 0, 0, 3];
        for e in [6_i32, 0, -223] {
            enc.extend(e.to_be_bytes());
        }
        s.write_all(&enc)?;
        s.set_read_timeout(Some(Duration::from_millis(150)))?;
        Ok(Self {
            socket: s,
            width,
            height,
            name: String::from_utf8_lossy(&name).into_owned(),
        })
    }
    pub fn request(&mut self, incremental: bool) -> io::Result<()> {
        let mut req = vec![3, u8::from(incremental), 0, 0, 0, 0];
        req.extend(self.width.to_be_bytes());
        req.extend(self.height.to_be_bytes());
        self.socket.write_all(&req)
    }
}
pub fn key_event(key: u32, down: bool) -> [u8; 8] {
    let mut b = [4, u8::from(down), 0, 0, 0, 0, 0, 0];
    b[4..].copy_from_slice(&key.to_be_bytes());
    b
}
pub fn pointer_event(x: u16, y: u16, buttons: u8) -> [u8; 6] {
    let mut b = [5, buttons, 0, 0, 0, 0];
    b[2..4].copy_from_slice(&x.to_be_bytes());
    b[4..6].copy_from_slice(&y.to_be_bytes());
    b
}
pub fn text_events(text: &str) -> Vec<u8> {
    let mut b = Vec::new();
    for ch in text.chars().take(8192) {
        let code = match ch {
            '\n' | '\r' => 0xff0d,
            '\t' => 0xff09,
            c if (c as u32) < 256 => c as u32,
            c => 0x01000000 | c as u32,
        };
        b.extend(key_event(code, true));
        b.extend(key_event(code, false));
    }
    b
}
pub fn named_key(name: &str) -> Option<u32> {
    Some(match name.to_lowercase().as_str() {
        "enter" | "return" => 0xff0d,
        "tab" => 0xff09,
        "escape" | "esc" => 0xff1b,
        "backspace" => 0xff08,
        "delete" => 0xffff,
        "insert" => 0xff63,
        "home" => 0xff50,
        "end" => 0xff57,
        "pageup" => 0xff55,
        "pagedown" => 0xff56,
        "left" => 0xff51,
        "up" => 0xff52,
        "right" => 0xff53,
        "down" => 0xff54,
        "shift" => 0xffe1,
        "ctrl" | "control" => 0xffe3,
        "alt" => 0xffe9,
        "super" | "meta" => 0xffeb,
        "space" => 32,
        name if name.starts_with('f')
            && name[1..]
                .parse::<u32>()
                .is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            0xffbd + name[1..].parse::<u32>().unwrap()
        }
        _ if name.chars().count() == 1 => name.chars().next().unwrap() as u32,
        _ => return None,
    })
}

pub struct Decoder {
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
    buffer: Vec<u8>,
    zlib: Decompress,
    pub updates: u64,
}
impl Decoder {
    pub fn new(w: u16, h: u16) -> io::Result<Self> {
        Ok(Self {
            width: w,
            height: h,
            rgba: vec![0; dimensions(w, h)?],
            buffer: Vec::new(),
            zlib: Decompress::new(true),
            updates: 0,
        })
    }
    pub fn feed(&mut self, bytes: &[u8]) -> io::Result<bool> {
        if self.buffer.len().saturating_add(bytes.len()) > MAX_BUFFER {
            return Err(err("RFB receive buffer exceeded"));
        }
        self.buffer.extend(bytes);
        let mut changed = false;
        loop {
            let Some(kind) = self.buffer.first().copied() else {
                break;
            };
            if kind == 2 {
                self.buffer.drain(..1);
                continue;
            }
            if kind == 3 {
                if self.buffer.len() < 8 {
                    break;
                }
                let len = u32be(&self.buffer[4..]) as usize;
                if len > 1024 * 1024 {
                    return Err(err("VNC clipboard too large"));
                }
                if self.buffer.len() < 8 + len {
                    break;
                }
                self.buffer.drain(..8 + len);
                continue;
            }
            if kind == 1 {
                if self.buffer.len() < 6 {
                    break;
                }
                let len = 6 + u16be(&self.buffer[4..]) as usize * 6;
                if self.buffer.len() < len {
                    break;
                }
                self.buffer.drain(..len);
                continue;
            }
            if kind != 0 {
                return Err(err("Unsupported VNC server message"));
            }
            if self.buffer.len() < 4 {
                break;
            }
            let count = u16be(&self.buffer[2..]) as usize;
            let mut pos = 4;
            let mut rectangles = Vec::new();
            let mut incomplete = false;
            for _ in 0..count {
                let Some(rect) = rectangle(&self.buffer[pos..])? else {
                    incomplete = true;
                    break;
                };
                pos += rect.header_len;
                if self.buffer.len() < pos + rect.payload_len {
                    incomplete = true;
                    break;
                }
                pos += rect.payload_len;
                rectangles.push(rect);
            }
            if incomplete {
                break;
            }
            let mut start = 4;
            for Rectangle {
                x,
                y,
                width: w,
                height: h,
                encoding,
                header_len,
                payload_len: len,
            } in rectangles
            {
                let end = start + header_len + len;
                let source_bytes = &self.buffer[start + header_len..end];
                start = end;
                if encoding == -223 {
                    self.width = w;
                    self.height = h;
                    self.rgba = vec![0; dimensions(w, h)?];
                    changed = true;
                    continue;
                }
                if x as u32 + w as u32 > self.width as u32
                    || y as u32 + h as u32 > self.height as u32
                {
                    return Err(err("Rectangle lies outside framebuffer"));
                }
                let expected = w as usize * h as usize * 4;
                let data = if encoding == 6 {
                    let mut out = vec![0; expected + 64];
                    let before = self.zlib.total_out();
                    let input_before = self.zlib.total_in();
                    let status = self
                        .zlib
                        .decompress(source_bytes, &mut out, FlushDecompress::Sync)
                        .map_err(|e| err(&e.to_string()))?;
                    if self.zlib.total_out() - before != expected as u64
                        || self.zlib.total_in() - input_before != len as u64
                    {
                        return Err(err("Invalid compressed rectangle length"));
                    }
                    out.truncate(expected);
                    if status == Status::StreamEnd {
                        self.zlib = Decompress::new(true);
                    }
                    out
                } else {
                    source_bytes.to_vec()
                };
                for row in 0..h as usize {
                    for col in 0..w as usize {
                        let source = (row * w as usize + col) * 4;
                        let dest =
                            ((y as usize + row) * self.width as usize + x as usize + col) * 4;
                        self.rgba[dest..dest + 4].copy_from_slice(&[
                            data[source + 2],
                            data[source + 1],
                            data[source],
                            255,
                        ]);
                    }
                }
                changed = true;
            }
            self.buffer.drain(..pos);
            self.updates += 1;
        }
        Ok(changed)
    }
    pub fn png(&self) -> io::Result<Vec<u8>> {
        use image::ImageEncoder;
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(
                &self.rgba,
                self.width as u32,
                self.height as u32,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| err(&e.to_string()))?;
        Ok(out)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_raw_frames_are_atomic_and_bounded() {
        let mut d = Decoder::new(2, 1).unwrap();
        let msg = [
            0, 0, 0, 1, 0, 0, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0, 17, 34, 51, 0, 60, 70, 80, 0,
        ];
        assert!(!d.feed(&msg[..20]).unwrap());
        assert_eq!(d.updates, 0);
        assert!(d.feed(&msg[20..]).unwrap());
        assert_eq!(&d.rgba[..4], &[51, 34, 17, 255]);
        assert_eq!(d.updates, 1);
        assert!(Decoder::new(4096, 4096).is_err());
    }
    #[test]
    fn mouse_keyboard_and_unicode_wire_encoding() {
        assert_eq!(pointer_event(513, 7, 1), [5, 1, 2, 1, 0, 7]);
        assert_eq!(named_key("F12"), Some(0xffc9));
        let b = text_events("日");
        assert_eq!(u32be(&b[4..8]), 0x010065e5);
        assert_eq!(b.len(), 16);
    }
    #[test]
    fn out_of_bounds_rectangle_is_rejected() {
        let mut d = Decoder::new(1, 1).unwrap();
        let msg = [0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(d.feed(&msg).is_err());
    }
}
