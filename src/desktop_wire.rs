//! Negotiated, lossless compression for desktop byte chunks.
use flate2::{Compression, Decompress, FlushDecompress, Status, write::ZlibEncoder};
use std::io::{self, Write};
pub const NAME: &str = "zlib-chunks-v1";
pub const MAX_CHUNK: usize = 65536;
pub fn encode(bytes: &[u8]) -> io::Result<Vec<u8>> {
    if bytes.is_empty() || bytes.len() > MAX_CHUNK {
        return Err(io::Error::other("Invalid desktop chunk size"));
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(bytes)?;
    let compressed = encoder.finish()?;
    let packed = compressed.len() < bytes.len();
    let mut output = Vec::with_capacity(
        5 + if packed {
            compressed.len()
        } else {
            bytes.len()
        },
    );
    output.push(u8::from(packed));
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    output.extend_from_slice(if packed { &compressed } else { bytes });
    Ok(output)
}
pub fn decode(bytes: &[u8]) -> io::Result<Vec<u8>> {
    if bytes.len() < 5 || bytes.len() > MAX_CHUNK + 5 {
        return Err(io::Error::other("Invalid desktop packet"));
    }
    let size = u32::from_be_bytes(bytes[1..5].try_into().unwrap()) as usize;
    if size == 0 || size > MAX_CHUNK {
        return Err(io::Error::other("Desktop chunk exceeds limit"));
    }
    match bytes[0] {
        0 if bytes.len() == size + 5 => Ok(bytes[5..].to_vec()),
        1 => {
            let mut output = vec![0; size + 1];
            let mut decoder = Decompress::new(true);
            let status = decoder
                .decompress(&bytes[5..], &mut output, FlushDecompress::Finish)
                .map_err(io::Error::other)?;
            if status != Status::StreamEnd
                || decoder.total_out() != size as u64
                || decoder.total_in() != (bytes.len() - 5) as u64
            {
                return Err(io::Error::other("Invalid compressed desktop chunk"));
            }
            output.truncate(size);
            Ok(output)
        }
        _ => Err(io::Error::other("Invalid desktop chunk encoding")),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declared_size_and_trailing_data_are_enforced() {
        let mut packed = encode(&vec![4; 1000]).unwrap();
        packed[1..5].copy_from_slice(&1u32.to_be_bytes());
        assert!(decode(&packed).is_err());
        let mut packed = encode(&vec![4; 1000]).unwrap();
        packed.push(0);
        assert!(decode(&packed).is_err());
    }
    #[test]
    fn desktop_pixels_compress_and_round_trip() {
        let input = vec![42; MAX_CHUNK];
        let packed = encode(&input).unwrap();
        assert!(packed.len() < input.len() / 10);
        assert_eq!(decode(&packed).unwrap(), input);
    }
    #[test]
    fn fragmented_rfb_stream_remains_byte_exact() {
        let input: Vec<_> = (0..MAX_CHUNK * 3 + 7).map(|n| (n % 251) as u8).collect();
        let mut output = vec![];
        for c in input.chunks(MAX_CHUNK) {
            output.extend(decode(&encode(c).unwrap()).unwrap());
        }
        assert_eq!(output, input);
    }
    #[test]
    fn corrupt_or_oversized_chunks_are_rejected() {
        assert!(decode(&[1, 0, 2, 0, 0, 1, 2, 3]).is_err());
        assert!(decode(&[0, 0, 0, 0, 8, 1]).is_err());
        assert!(encode(&vec![0; MAX_CHUNK + 1]).is_err());
    }
}
