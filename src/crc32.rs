//! CRC-32 (IEEE 802.3, polynomial 0xEDB88320), the checksum used in the LZF1
//! trailer. The lookup table is built at compile time.

const fn make_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = make_table();

/// Incremental CRC-32 accumulator.
#[derive(Clone)]
pub struct Crc32 {
    state: u32,
}

impl Crc32 {
    pub fn new() -> Self {
        Crc32 { state: !0 }
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut state = self.state;
        for &b in data {
            state = CRC_TABLE[((state ^ b as u32) & 0xFF) as usize] ^ (state >> 8);
        }
        self.state = state;
    }

    pub fn finalize(&self) -> u32 {
        !self.state
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// One-shot CRC-32 of a byte slice.
pub fn checksum(data: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(data);
    crc.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(checksum(b""), 0x0000_0000);
        assert_eq!(checksum(b"123456789"), 0xCBF4_3926);
        assert_eq!(checksum(b"hello"), 0x3610_A686);
        // Concatenation equals incremental update.
        let mut crc = Crc32::new();
        crc.update(b"hello ");
        crc.update(b"world");
        assert_eq!(crc.finalize(), checksum(b"hello world"));
    }
}
