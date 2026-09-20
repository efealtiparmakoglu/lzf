//! LZF1 container framing: the file header (magic, version, stored name),
//! block-size constant and trailer layout. Field order and encodings are
//! normative — see FORMAT.md.

use std::io::{Read, Write};

use crate::error::{Error, Result};

/// Every LZF1 file starts with these bytes.
pub const MAGIC: &[u8; 4] = b"LZF1";
/// Container version implemented by this crate.
pub const VERSION: u8 = 1;
/// Maximum number of input bytes per block.
pub const BLOCK_SIZE: usize = 1 << 20; // 1 MiB

/// Parsed file header.
#[derive(Clone, Debug)]
pub struct Header {
    /// Informational copy of the original file name (basename only, as
    /// stored). Consumers must NOT use it as a filesystem path without
    /// sanitization.
    pub name: String,
}

/// Serializes the header; returns the number of bytes written.
pub fn write_header<W: Write>(w: &mut W, name: &[u8]) -> Result<usize> {
    let name: Vec<u8> = name.iter().copied().take(255).collect();
    let mut buf = Vec::with_capacity(4 + 1 + 1 + name.len());
    buf.extend_from_slice(MAGIC);
    buf.push(VERSION);
    buf.push(name.len() as u8);
    buf.extend_from_slice(&name);
    w.write_all(&buf)?;
    Ok(buf.len())
}

/// Parses and validates a file header.
pub fn read_header<R: Read>(r: &mut R) -> Result<Header> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)
        .map_err(|_| Error::Corrupt("not an LZF1 file (truncated header)"))?;
    if &magic != MAGIC {
        return Err(Error::Corrupt("bad magic"));
    }
    let mut one = [0u8; 1];
    r.read_exact(&mut one)?;
    if one[0] != VERSION {
        return Err(Error::Corrupt("unsupported version"));
    }
    r.read_exact(&mut one)?;
    let name_len = one[0] as usize;
    let mut name = vec![0u8; name_len];
    r.read_exact(&mut name)?;
    Ok(Header {
        name: String::from_utf8_lossy(&name).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let mut buf = Vec::new();
        let n = write_header(&mut buf, b"notes.txt").unwrap();
        assert_eq!(n, buf.len());
        assert_eq!(n, 4 + 1 + 1 + 9);
        let mut cur = std::io::Cursor::new(&buf);
        let h = read_header(&mut cur).unwrap();
        assert_eq!(h.name, "notes.txt");
        assert_eq!(cur.position() as usize, n);
    }

    #[test]
    fn empty_name_is_fine() {
        let mut buf = Vec::new();
        write_header(&mut buf, b"").unwrap();
        let h = read_header(&mut std::io::Cursor::new(&buf)).unwrap();
        assert_eq!(h.name, "");
    }

    #[test]
    fn long_name_truncated_to_255() {
        let mut buf = Vec::new();
        let long = vec![b'a'; 400];
        write_header(&mut buf, &long).unwrap();
        let h = read_header(&mut std::io::Cursor::new(&buf)).unwrap();
        assert_eq!(h.name.len(), 255);
    }

    #[test]
    fn bad_magic_detected() {
        let mut buf = Vec::new();
        write_header(&mut buf, b"x").unwrap();
        buf[3] = b'2';
        assert!(matches!(
            read_header(&mut std::io::Cursor::new(&buf)),
            Err(Error::Corrupt("bad magic"))
        ));
    }
}
