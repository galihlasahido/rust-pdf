//! A tiny, panic-free big-endian bit reader shared by [`super::image`]
//! (unpacking `1`/`2`/`4`/`8`/`16`-bit-per-component image samples, ISO
//! 32000-1:2008 8.9.5.2) and [`super::function`] (unpacking arbitrary
//! `1..=32`-bit-per-sample Type 0 sampled-function data, ISO 32000-1
//! 7.10.2).
//!
//! Untrusted input: reading past the end of the underlying byte slice
//! (e.g. a `/Width`/`/Height`/`/BitsPerSample` combination that claims more
//! samples than the actual decoded stream contains) returns `0` for the
//! missing bits rather than panicking or indexing out of bounds.

/// Reads fixed-width unsigned integers from a byte slice, most-significant
/// bit first, tracking position in bits (not bytes) so callers can express
/// e.g. a 12-bit sample width directly.
pub(super) struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: u64,
}

impl<'a> BitReader<'a> {
    /// Starts reading `data` from its first bit.
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    /// Repositions the reader to an absolute bit offset (used by
    /// [`super::function`]'s multi-dimensional sample lookup, which must
    /// seek directly to an arbitrary sample rather than reading
    /// sequentially).
    pub(super) fn seek_bit(&mut self, bit_pos: u64) {
        self.bit_pos = bit_pos;
    }

    /// Reads `bits` (must be `1..=32`; larger values saturate to reading at
    /// most 32) as a big-endian unsigned integer and advances the position.
    /// Bits past the end of `data` read as `0` (never panics/indexes out of
    /// bounds -- untrusted input may declare more samples than are actually
    /// present in a truncated/corrupt decoded stream).
    pub(super) fn read_bits(&mut self, bits: u32) -> u32 {
        let bits = bits.min(32);
        let mut value: u32 = 0;
        for _ in 0..bits {
            let byte_idx = (self.bit_pos / 8) as usize;
            let bit_idx = 7 - (self.bit_pos % 8) as u32;
            let bit = self
                .data
                .get(byte_idx)
                .map(|b| (b >> bit_idx) & 1)
                .unwrap_or(0);
            value = (value << 1) | u32::from(bit);
            self.bit_pos += 1;
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_msb_first_bytes() {
        let mut r = BitReader::new(&[0b1010_0000]);
        assert_eq!(r.read_bits(1), 1);
        assert_eq!(r.read_bits(1), 0);
        assert_eq!(r.read_bits(1), 1);
        assert_eq!(r.read_bits(1), 0);
    }

    #[test]
    fn reads_multi_bit_values() {
        let mut r = BitReader::new(&[0xFF, 0x00]);
        assert_eq!(r.read_bits(4), 0xF);
        assert_eq!(r.read_bits(4), 0xF);
        assert_eq!(r.read_bits(8), 0x00);
    }

    #[test]
    fn seek_repositions() {
        let mut r = BitReader::new(&[0b1111_0000, 0b0000_1111]);
        r.seek_bit(8);
        assert_eq!(r.read_bits(4), 0);
        assert_eq!(r.read_bits(4), 0xF);
    }

    #[test]
    fn reading_past_end_returns_zero_not_panic() {
        let mut r = BitReader::new(&[0xFF]);
        r.seek_bit(4);
        assert_eq!(r.read_bits(16), 0b1111_0000_0000_0000);
    }
}
