//! CCITTFaxDecode filter (ISO 32000-1:2008 Section 7.4.6).
//!
//! Implements Group 4 (`K < 0`, pure two-dimensional / "MMR") decoding,
//! which is by far the most common `CCITTFaxDecode` variant found in
//! real-world PDF files (scanned monochrome pages). The run-length and mode
//! Huffman tables below follow the standard tables published in ITU-T
//! Recommendation T.4 (Tables 2-4, also reused by T.6/Group 4).
//!
//! # Known limitations
//!
//! - Only `K < 0` (pure 2D / Group 4) is implemented. Group 3 1D (`K == 0`)
//!   and mixed 1D/2D Group 3 (`K > 0`) are not implemented and return
//!   [`CompressionError::DecompressionFailed`] rather than guessing.
//! - The Huffman run-length tables were not hand-transcribed from the spec
//!   text (an earlier draft that did so contained a transcription bug,
//!   caught by the prefix-free self-check test below). Instead they were
//!   derived mechanically: the equivalent lookup tables were fetched from
//!   Mozilla's `pdf.js` (`src/core/ccitt.js`, Apache-2.0 - itself a JS port
//!   of Xpdf/Glyph & Cog's C++ decoder) and every valid code word was
//!   enumerated by exhaustively driving that table's own lookup logic,
//!   re-implemented in a throwaway script, eliminating manual bit-pattern
//!   transcription. This has *not* been validated against a corpus of real
//!   scanner/Acrobat-produced CCITT streams (no reference fixtures were
//!   available in this environment); production use should be preceded by
//!   conformance testing against real-world samples.

use crate::error::CompressionError;
use crate::filter::MAX_DECODED_SIZE;

/// Parameters controlling `CCITTFaxDecode`, taken from `/DecodeParms`
/// (ISO 32000-1 Table 11) and (for `/Rows`) the image's `/Height`.
#[derive(Debug, Clone, Copy)]
pub struct CcittParams {
    /// `/K` - encoding scheme. Only `K < 0` (Group 4) is supported.
    pub k: i64,
    /// `/Columns` (default 1728).
    pub columns: i64,
    /// `/Rows` (default 0, meaning "use the image's /Height").
    pub rows: i64,
    /// `/BlackIs1` (default false: `0` = black, `1` = white).
    pub black_is_1: bool,
    /// `/EncodedByteAlign` (default false).
    pub encoded_byte_align: bool,
}

impl Default for CcittParams {
    fn default() -> Self {
        Self {
            k: 0,
            columns: 1728,
            rows: 0,
            black_is_1: false,
            encoded_byte_align: false,
        }
    }
}

/// A single Huffman table entry: `(bit_length, code_value, run_length)`.
type CodeEntry = (u8, u16, u32);

// ITU-T T.4 Table 2: terminating codes (run 0-63) for white runs. Verified
// by mechanically deriving the full (bit-length, code, run) mapping from
// Mozilla pdf.js's `ccitt.js` (Apache-2.0), itself a JS port of Xpdf's
// (Glyph & Cog) C++ CCITT decoder, rather than being hand-transcribed from
// the spec text - see the module docs for why.
#[rustfmt::skip]
const WHITE_TERMINATING: &[CodeEntry] = &[
    (8,0b00110101,0),(6,0b000111,1),(4,0b0111,2),(4,0b1000,3),(4,0b1011,4),(4,0b1100,5),
    (4,0b1110,6),(4,0b1111,7),(5,0b10011,8),(5,0b10100,9),(5,0b00111,10),(5,0b01000,11),
    (6,0b001000,12),(6,0b000011,13),(6,0b110100,14),(6,0b110101,15),(6,0b101010,16),(6,0b101011,17),
    (7,0b0100111,18),(7,0b0001100,19),(7,0b0001000,20),(7,0b0010111,21),(7,0b0000011,22),(7,0b0000100,23),
    (7,0b0101000,24),(7,0b0101011,25),(7,0b0010011,26),(7,0b0100100,27),(7,0b0011000,28),(8,0b00000010,29),
    (8,0b00000011,30),(8,0b00011010,31),(8,0b00011011,32),(8,0b00010010,33),(8,0b00010011,34),(8,0b00010100,35),
    (8,0b00010101,36),(8,0b00010110,37),(8,0b00010111,38),(8,0b00101000,39),(8,0b00101001,40),(8,0b00101010,41),
    (8,0b00101011,42),(8,0b00101100,43),(8,0b00101101,44),(8,0b00000100,45),(8,0b00000101,46),(8,0b00001010,47),
    (8,0b00001011,48),(8,0b01010010,49),(8,0b01010011,50),(8,0b01010100,51),(8,0b01010101,52),(8,0b00100100,53),
    (8,0b00100101,54),(8,0b01011000,55),(8,0b01011001,56),(8,0b01011010,57),(8,0b01011011,58),(8,0b01001010,59),
    (8,0b01001011,60),(8,0b00110010,61),(8,0b00110011,62),(8,0b00110100,63),
];

// ITU-T T.4 Table 2: makeup codes for white runs (64-1728). See provenance
// note on [`WHITE_TERMINATING`].
#[rustfmt::skip]
const WHITE_MAKEUP: &[CodeEntry] = &[
    (5,0b11011,64),(5,0b10010,128),(6,0b010111,192),(7,0b0110111,256),(8,0b00110110,320),(8,0b00110111,384),
    (8,0b01100100,448),(8,0b01100101,512),(8,0b01101000,576),(8,0b01100111,640),(9,0b011001100,704),(9,0b011001101,768),
    (9,0b011010010,832),(9,0b011010011,896),(9,0b011010100,960),(9,0b011010101,1024),(9,0b011010110,1088),(9,0b011010111,1152),
    (9,0b011011000,1216),(9,0b011011001,1280),(9,0b011011010,1344),(9,0b011011011,1408),(9,0b010011000,1472),(9,0b010011001,1536),
    (9,0b010011010,1600),(6,0b011000,1664),(9,0b010011011,1728),
];

// ITU-T T.4 Table 3: terminating codes (run 0-63) for black runs. See
// provenance note on [`WHITE_TERMINATING`].
#[rustfmt::skip]
const BLACK_TERMINATING: &[CodeEntry] = &[
    (10,0b0000110111,0),(3,0b010,1),(2,0b11,2),(2,0b10,3),(3,0b011,4),(4,0b0011,5),
    (4,0b0010,6),(5,0b00011,7),(6,0b000101,8),(6,0b000100,9),(7,0b0000100,10),(7,0b0000101,11),
    (7,0b0000111,12),(8,0b00000100,13),(8,0b00000111,14),(9,0b000011000,15),(10,0b0000010111,16),(10,0b0000011000,17),
    (10,0b0000001000,18),(11,0b00001100111,19),(11,0b00001101000,20),(11,0b00001101100,21),(11,0b00000110111,22),(11,0b00000101000,23),
    (11,0b00000010111,24),(11,0b00000011000,25),(12,0b000011001010,26),(12,0b000011001011,27),(12,0b000011001100,28),(12,0b000011001101,29),
    (12,0b000001101000,30),(12,0b000001101001,31),(12,0b000001101010,32),(12,0b000001101011,33),(12,0b000011010010,34),(12,0b000011010011,35),
    (12,0b000011010100,36),(12,0b000011010101,37),(12,0b000011010110,38),(12,0b000011010111,39),(12,0b000001101100,40),(12,0b000001101101,41),
    (12,0b000011011010,42),(12,0b000011011011,43),(12,0b000001010100,44),(12,0b000001010101,45),(12,0b000001010110,46),(12,0b000001010111,47),
    (12,0b000001100100,48),(12,0b000001100101,49),(12,0b000001010010,50),(12,0b000001010011,51),(12,0b000000100100,52),(12,0b000000110111,53),
    (12,0b000000111000,54),(12,0b000000100111,55),(12,0b000000101000,56),(12,0b000001011000,57),(12,0b000001011001,58),(12,0b000000101011,59),
    (12,0b000000101100,60),(12,0b000001011010,61),(12,0b000001100110,62),(12,0b000001100111,63),
];

// ITU-T T.4 Table 3: makeup codes for black runs (64-1728). See provenance
// note on [`WHITE_TERMINATING`].
#[rustfmt::skip]
const BLACK_MAKEUP: &[CodeEntry] = &[
    (10,0b0000001111,64),(12,0b000011001000,128),(12,0b000011001001,192),(12,0b000001011011,256),(12,0b000000110011,320),(12,0b000000110100,384),
    (12,0b000000110101,448),(13,0b0000001101100,512),(13,0b0000001101101,576),(13,0b0000001001010,640),(13,0b0000001001011,704),(13,0b0000001001100,768),
    (13,0b0000001001101,832),(13,0b0000001110010,896),(13,0b0000001110011,960),(13,0b0000001110100,1024),(13,0b0000001110101,1088),(13,0b0000001110110,1152),
    (13,0b0000001110111,1216),(13,0b0000001010010,1280),(13,0b0000001010011,1344),(13,0b0000001010100,1408),(13,0b0000001010101,1472),(13,0b0000001011010,1536),
    (13,0b0000001011011,1600),(13,0b0000001100100,1664),(13,0b0000001100101,1728),
];

// ITU-T T.4 Table 4: extended makeup codes, shared by white and black
// (runs 1792-2560).
#[rustfmt::skip]
const EXT_MAKEUP: &[CodeEntry] = &[
    (11,0b00000001000,1792),(11,0b00000001100,1856),(11,0b00000001101,1920),
    (12,0b000000010010,1984),(12,0b000000010011,2048),(12,0b000000010100,2112),
    (12,0b000000010101,2176),(12,0b000000010110,2240),(12,0b000000010111,2304),
    (12,0b000000011100,2368),(12,0b000000011101,2432),(12,0b000000011110,2496),
    (12,0b000000011111,2560),
];

/// MSB-first bit reader with byte-alignment support (`EncodedByteAlign`).
struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_pos: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    fn at_end(&self) -> bool {
        self.byte_pos >= self.data.len()
    }

    fn align_to_byte(&mut self) {
        if self.bit_pos != 0 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
    }

    /// Peeks the next bit without consuming it; returns `None` at EOF.
    fn peek_bit(&self) -> Option<u8> {
        if self.byte_pos >= self.data.len() {
            return None;
        }
        Some((self.data[self.byte_pos] >> (7 - self.bit_pos)) & 1)
    }

    fn consume_bit(&mut self) {
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
    }
}

/// Reads a run length (terminating + any makeup codes) for the given
/// colour, by walking the bit stream one bit at a time and matching
/// against the appropriate Huffman tables.
fn read_run(reader: &mut BitReader, white: bool) -> Result<u32, CompressionError> {
    let mut total = 0u32;
    loop {
        let run = read_one_code(reader, white)?;
        total = total.checked_add(run).ok_or_else(|| {
            CompressionError::DecompressionFailed("CCITTFaxDecode: run length overflow".into())
        })?;
        // Makeup codes are always multiples of 64 and >= 64; a terminating
        // code (< 64) ends the run.
        if run < 64 {
            return Ok(total);
        }
    }
}

fn read_one_code(reader: &mut BitReader, white: bool) -> Result<u32, CompressionError> {
    let tables: [&[CodeEntry]; 3] = if white {
        [WHITE_TERMINATING, WHITE_MAKEUP, EXT_MAKEUP]
    } else {
        [BLACK_TERMINATING, BLACK_MAKEUP, EXT_MAKEUP]
    };

    let mut code: u16 = 0;
    for bit_len in 1..=13u8 {
        let bit = reader.peek_bit().ok_or_else(|| {
            CompressionError::DecompressionFailed(
                "CCITTFaxDecode: unexpected end of data while reading run code".into(),
            )
        })?;
        reader.consume_bit();
        code = (code << 1) | bit as u16;

        for table in &tables {
            for &(len, val, run) in *table {
                if len == bit_len && val == code {
                    return Ok(run);
                }
            }
        }
    }

    Err(CompressionError::DecompressionFailed(
        "CCITTFaxDecode: invalid Huffman run-length code".into(),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Pass,
    Horizontal,
    Vertical(i8), // offset -3..=3
    Eol,
}

/// Reads a single 2D mode code (T.6 mode codes, shared with T.4 2D lines).
fn read_mode(reader: &mut BitReader) -> Result<Mode, CompressionError> {
    // Try progressively longer prefixes against the known mode code set.
    let mut code: u32 = 0;
    for bit_len in 1..=12u8 {
        let bit = reader.peek_bit().ok_or_else(|| {
            CompressionError::DecompressionFailed(
                "CCITTFaxDecode: unexpected end of data while reading mode code".into(),
            )
        })?;
        reader.consume_bit();
        code = (code << 1) | bit as u32;

        let m = match (bit_len, code) {
            (1, 0b1) => Some(Mode::Vertical(0)),
            (3, 0b011) => Some(Mode::Vertical(1)),
            (3, 0b010) => Some(Mode::Vertical(-1)),
            (3, 0b001) => Some(Mode::Horizontal),
            (4, 0b0001) => Some(Mode::Pass),
            (6, 0b000011) => Some(Mode::Vertical(2)),
            (6, 0b000010) => Some(Mode::Vertical(-2)),
            (7, 0b0000011) => Some(Mode::Vertical(3)),
            (7, 0b0000010) => Some(Mode::Vertical(-3)),
            (12, 0b000000000001) => Some(Mode::Eol),
            _ => None,
        };
        if let Some(m) = m {
            return Ok(m);
        }
    }

    Err(CompressionError::DecompressionFailed(
        "CCITTFaxDecode: invalid 2D mode code".into(),
    ))
}

/// Finds `b1` (first changing element on the reference line to the right of
/// `a0`, with colour opposite `a0_color`) and `b2` (the next changing
/// element after `b1`). `ref_line` holds sorted changing-element positions;
/// even indices mark white->black transitions, odd indices black->white.
fn find_b1_b2(ref_line: &[i32], a0: i32, a0_is_black: bool, columns: i32) -> (i32, i32) {
    // Find first index with position > a0.
    let mut idx = match ref_line.binary_search(&a0) {
        Ok(i) => i + 1,
        Err(i) => i,
    };
    // b1 must be a transition *to* the colour opposite a0's colour.
    // Even index => transition to black; odd index => transition to white.
    let want_black_transition = !a0_is_black;
    let idx_is_black_transition = idx % 2 == 0;
    if idx_is_black_transition != want_black_transition {
        idx += 1;
    }

    let b1 = ref_line.get(idx).copied().unwrap_or(columns);
    let b2 = ref_line.get(idx + 1).copied().unwrap_or(columns);
    (b1, b2)
}

/// Decodes data encoded with the `CCITTFaxDecode` filter (Group 4 only).
///
/// Returns packed 1-bit-per-pixel rows (MSB first), each row padded to a
/// whole byte, honouring `/BlackIs1`.
pub fn decode_ccitt(data: &[u8], params: CcittParams) -> Result<Vec<u8>, CompressionError> {
    if params.k >= 0 {
        return Err(CompressionError::DecompressionFailed(
            "CCITTFaxDecode: only Group 4 (K < 0) is supported".to_string(),
        ));
    }

    let columns = params.columns.clamp(1, 1 << 20) as i32;
    let row_bytes = (columns as u64).div_ceil(8) as usize;

    // Cap rows to keep memory bounded even on corrupt/malicious /Rows.
    let max_rows = if params.rows > 0 {
        params.rows.clamp(1, 1_000_000) as usize
    } else {
        1_000_000
    };

    let mut reader = BitReader::new(data);
    let mut out = Vec::new();
    // Reference line starts as an imaginary all-white line: no transitions.
    let mut ref_line: Vec<i32> = Vec::new();
    let mut rows_decoded = 0usize;

    while rows_decoded < max_rows {
        if reader.at_end() {
            break;
        }
        if params.encoded_byte_align {
            reader.align_to_byte();
            if reader.at_end() {
                break;
            }
        }

        let mut coding_line: Vec<i32> = Vec::new();
        let mut a0: i32 = -1;
        let mut a0_is_black = false;

        loop {
            if a0 >= columns {
                break;
            }
            let mode = read_mode(&mut reader)?;
            if mode == Mode::Eol {
                // Two consecutive EOLs (EOFB) or a stray EOL: treat as
                // end-of-data for this image.
                coding_line.push(columns);
                break;
            }

            let (b1, b2) = find_b1_b2(&ref_line, a0, a0_is_black, columns);

            match mode {
                Mode::Pass => {
                    coding_line_push_run(&mut coding_line, b2, a0_is_black);
                    a0 = b2;
                    // colour unchanged
                }
                Mode::Horizontal => {
                    let start = if a0 < 0 { 0 } else { a0 };
                    let run1 = read_run(&mut reader, !a0_is_black)?;
                    let run2 = read_run(&mut reader, a0_is_black)?;
                    let a1 = (start + run1 as i32).min(columns);
                    let a2 = (a1 + run2 as i32).min(columns);
                    coding_line.push(a1);
                    coding_line.push(a2);
                    a0 = a2;
                    // colour unchanged after two runs
                }
                Mode::Vertical(offset) => {
                    let a1 = (b1 + offset as i32).clamp(0, columns);
                    coding_line.push(a1);
                    a0 = a1;
                    a0_is_black = !a0_is_black;
                }
                Mode::Eol => unreachable!(),
            }

            if coding_line.len() > columns as usize + 4 {
                return Err(CompressionError::DecompressionFailed(
                    "CCITTFaxDecode: malformed row (too many transitions)".to_string(),
                ));
            }
        }

        let row = render_row(&coding_line, columns, row_bytes, params.black_is_1);
        out.extend_from_slice(&row);
        rows_decoded += 1;

        if out.len() > MAX_DECODED_SIZE {
            return Err(CompressionError::DecompressionFailed(
                "CCITTFaxDecode: decoded output exceeds maximum allowed size".to_string(),
            ));
        }

        ref_line = coding_line;
        ref_line.retain(|&p| p < columns);
    }

    Ok(out)
}

/// Pass-mode helper: Pass mode does not itself add a changing element to the
/// coding line at `a0`'s boundary (the run simply extends); nothing to
/// record besides updating `a0`, which the caller does. Kept as a
/// documented no-op function to make the call site self-explanatory.
fn coding_line_push_run(_coding_line: &mut [i32], _b2: i32, _a0_is_black: bool) {}

/// Renders one decoded row (list of changing-element positions, starting
/// with a white run) into packed 1bpp bytes.
///
/// ISO 32000-1:2008 Table 11's `BlackIs1` (default `false`) means: `0` bits
/// represent black pixels and `1` bits represent white pixels (the "normal"
/// convention, matching plain `DeviceGray` sample semantics where `0` =
/// black); `BlackIs1 = true` reverses that (`1` = black, `0` = white). The
/// output buffer starts zero-initialised, so exactly one of "this run's
/// colour" vs. "the other colour" needs its bits explicitly set to `1` --
/// whichever colour maps to `1` under `black_is_1` (i.e. this run's bits
/// are set exactly when `is_black == black_is_1`; the other colour's runs
/// are left at the already-correct `0`).
fn render_row(coding_line: &[i32], columns: i32, row_bytes: usize, black_is_1: bool) -> Vec<u8> {
    let mut row = vec![0u8; row_bytes];
    let mut pos = 0i32;
    let mut is_black = false;

    for &change in coding_line {
        let change = change.clamp(0, columns);
        if is_black == black_is_1 {
            set_bits(&mut row, pos, change);
        }
        pos = change;
        is_black = !is_black;
        if pos >= columns {
            break;
        }
    }

    row
}

fn set_bits(row: &mut [u8], start: i32, end: i32) {
    for px in start.max(0)..end {
        let byte_idx = (px / 8) as usize;
        let bit_idx = 7 - (px % 8);
        if byte_idx < row.len() {
            row[byte_idx] |= 1 << bit_idx;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the transcribed Huffman tables form a valid prefix-free
    /// code (no code is a prefix of another within the same colour table),
    /// which would catch gross transcription errors even without external
    /// reference vectors.
    fn assert_prefix_free(tables: &[&[CodeEntry]]) {
        let mut all: Vec<(u8, u16)> = Vec::new();
        for t in tables {
            for &(len, val, _run) in *t {
                all.push((len, val));
            }
        }
        for i in 0..all.len() {
            for j in 0..all.len() {
                if i == j {
                    continue;
                }
                let (li, vi) = all[i];
                let (lj, vj) = all[j];
                if li <= lj {
                    let shifted = vj >> (lj - li);
                    assert!(
                        shifted != vi,
                        "code {:0width$b} (len {}) is a prefix of {:0width2$b} (len {})",
                        vi,
                        li,
                        vj,
                        lj,
                        width = li as usize,
                        width2 = lj as usize
                    );
                }
            }
        }
    }

    #[test]
    fn white_table_is_prefix_free() {
        assert_prefix_free(&[WHITE_TERMINATING, WHITE_MAKEUP, EXT_MAKEUP]);
    }

    #[test]
    fn black_table_is_prefix_free() {
        assert_prefix_free(&[BLACK_TERMINATING, BLACK_MAKEUP, EXT_MAKEUP]);
    }

    #[test]
    fn no_duplicate_run_lengths_within_table() {
        // Terminating codes 0-63 must each appear exactly once.
        for table in [WHITE_TERMINATING, BLACK_TERMINATING] {
            let mut runs: Vec<u32> = table.iter().map(|&(_, _, r)| r).collect();
            runs.sort_unstable();
            let expected: Vec<u32> = (0..64).collect();
            assert_eq!(runs, expected);
        }
    }

    /// Builds a Group-4 bitstream by hand for an all-white 8x2 image
    /// (V0 vertical mode reproduces "no change" from an all-white
    /// reference line, so two V0-only rows stay all-white) and checks the
    /// decoder reproduces an all-white bitmap.
    #[test]
    fn decodes_all_white_image() {
        // Row 1: reference line is imaginary all-white -> b1 = columns (8).
        // A single V0 sets a1 = b1 + 0 = 8 = columns, ending the row as
        // pure white with one transition recorded at column 8 (== columns,
        // so effectively no black run rendered).
        // Bits: V0 = "1"
        // Row 2: same reference (now equal to row1, still no transitions
        // before columns) -> V0 again.
        let mut bits = String::new();
        bits.push('1'); // row1: V0
        bits.push('1'); // row2: V0
        let data = bits_to_bytes(&bits);

        let params = CcittParams {
            k: -1,
            columns: 8,
            rows: 2,
            black_is_1: false,
            encoded_byte_align: false,
        };
        let out = decode_ccitt(&data, params).unwrap();
        // BlackIs1 = false (default) is the "normal" convention: 0 = black,
        // 1 = white (ISO 32000-1 Table 11) -- an all-white row is all 1
        // bits, i.e. 0xFF per byte.
        assert_eq!(out, vec![0xFF, 0xFF]);
    }

    /// Regression test for a real bug this phase fixed: the row renderer
    /// used to only ever set bits for *black* runs and only when
    /// `BlackIs1` was `true`, meaning a white run's pixels (which need bit
    /// `1` under the default, far more common `BlackIs1 = false`) were
    /// never actually set -- every row decoded as all-zero regardless of
    /// its real black/white content whenever `BlackIs1` was left at its
    /// default. A mixed white-then-black row makes that bug immediately
    /// visible (both halves would come out identical/all-black instead of
    /// visibly different).
    #[test]
    fn white_then_black_row_produces_distinguishable_halves_by_default() {
        // 16 columns, Horizontal mode: white run of 8, then black run of 8.
        let mut bits = String::new();
        bits.push_str("001"); // Horizontal
        bits.push_str("10011"); // white run length 8 (WHITE_TERMINATING)
        bits.push_str("000101"); // black run length 8 (BLACK_TERMINATING)
        let data = bits_to_bytes(&bits);

        let params = CcittParams {
            k: -1,
            columns: 16,
            rows: 1,
            black_is_1: false,
            encoded_byte_align: false,
        };
        let out = decode_ccitt(&data, params).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], 0xFF, "first 8 columns (white run) must be all 1 bits");
        assert_eq!(out[1], 0x00, "last 8 columns (black run) must be all 0 bits");
    }

    #[test]
    fn decodes_horizontal_mode_black_run() {
        // 8-column row, Horizontal mode: white run of 0, black run of 8.
        // Mode code "001" (Horizontal), then white-run(0) = "00110101",
        // then black-run(8) = "000101".
        let mut bits = String::new();
        bits.push_str("001"); // Horizontal
        bits.push_str("00110101"); // white run 0
        bits.push_str("000101"); // black run 8
        let data = bits_to_bytes(&bits);

        let params = CcittParams {
            k: -1,
            columns: 8,
            rows: 1,
            black_is_1: false,
            encoded_byte_align: false,
        };
        let out = decode_ccitt(&data, params).unwrap();
        // BlackIs1 = false => black pixels are 0 bits => whole byte 0x00.
        assert_eq!(out, vec![0x00]);

        let params_b1 = CcittParams {
            black_is_1: true,
            ..params
        };
        let out_b1 = decode_ccitt(&data, params_b1).unwrap();
        assert_eq!(out_b1, vec![0xFF]);
    }

    #[test]
    fn rejects_group3_k_gte_0() {
        let params = CcittParams {
            k: 0,
            ..Default::default()
        };
        let result = decode_ccitt(&[0u8; 4], params);
        assert!(result.is_err());
    }

    #[test]
    fn truncated_stream_does_not_panic() {
        let params = CcittParams {
            k: -1,
            columns: 1728,
            rows: 100,
            black_is_1: false,
            encoded_byte_align: false,
        };
        // Garbage / truncated data: must return Ok (best-effort, stops
        // early) or a clean Err - never panic.
        let _ = decode_ccitt(&[0xFF, 0x00, 0xAB], params);
    }

    fn bits_to_bytes(bits: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cur = 0u8;
        let mut n = 0u8;
        for c in bits.chars() {
            cur = (cur << 1) | if c == '1' { 1 } else { 0 };
            n += 1;
            if n == 8 {
                out.push(cur);
                cur = 0;
                n = 0;
            }
        }
        if n > 0 {
            cur <<= 8 - n;
            out.push(cur);
        }
        out
    }
}
