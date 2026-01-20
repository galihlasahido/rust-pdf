//! PDF cross-reference table parsing.

use crate::error::ParserError;
use crate::parser::lexer::*;
use nom::{
    bytes::complete::take_while1,
    character::complete::{multispace1, one_of},
    combinator::{map_res, opt},
    IResult,
};
use std::collections::HashMap;

/// An entry in the cross-reference table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum XrefEntry {
    /// Object is in use at the given byte offset.
    InUse {
        offset: u64,
        generation: u16,
    },
    /// Object is free (deleted).
    Free {
        next_free: u32,
        generation: u16,
    },
    /// Object is stored in an object stream (PDF 1.5+).
    Compressed {
        object_stream: u32,
        index: u32,
    },
}

impl XrefEntry {
    /// Returns the byte offset if this entry is in use.
    pub fn offset(&self) -> Option<u64> {
        match self {
            XrefEntry::InUse { offset, .. } => Some(*offset),
            _ => None,
        }
    }

    /// Returns true if this entry is in use.
    pub fn is_in_use(&self) -> bool {
        matches!(self, XrefEntry::InUse { .. })
    }

    /// Returns true if this entry is free.
    pub fn is_free(&self) -> bool {
        matches!(self, XrefEntry::Free { .. })
    }

    /// Returns true if this entry is compressed.
    pub fn is_compressed(&self) -> bool {
        matches!(self, XrefEntry::Compressed { .. })
    }
}

/// The cross-reference table.
#[derive(Debug, Default)]
pub struct XrefTable {
    /// Entries indexed by object number.
    entries: HashMap<u32, XrefEntry>,
}

impl XrefTable {
    /// Creates a new empty xref table.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Gets an entry by object number.
    pub fn get(&self, obj_num: u32) -> Option<&XrefEntry> {
        self.entries.get(&obj_num)
    }

    /// Inserts an entry.
    pub fn insert(&mut self, obj_num: u32, entry: XrefEntry) {
        self.entries.insert(obj_num, entry);
    }

    /// Returns the number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the table is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Merges another xref table into this one.
    /// Newer entries (from this table) take precedence.
    pub fn merge(&mut self, other: XrefTable) {
        for (obj_num, entry) in other.entries {
            self.entries.entry(obj_num).or_insert(entry);
        }
    }

    /// Returns an iterator over all entries.
    pub fn iter(&self) -> impl Iterator<Item = (&u32, &XrefEntry)> {
        self.entries.iter()
    }
}

/// Parse a traditional xref table.
pub fn parse_xref_table(input: &[u8]) -> IResult<&[u8], XrefTable> {
    let (input, _) = parse_xref(input)?;
    let (input, _) = skip_whitespace(input)?;

    let mut table = XrefTable::new();
    let mut remaining = input;

    // Parse subsections until we hit trailer
    loop {
        let (input, _) = skip_whitespace(remaining)?;

        // Check if we've reached the trailer
        if input.starts_with(b"trailer") {
            return Ok((input, table));
        }

        // Parse subsection header: first_obj_num count
        let (input, first_obj) = parse_xref_integer(input)?;
        let (input, _) = skip_whitespace(input)?;
        let (input, count) = parse_xref_integer(input)?;
        let (input, _) = skip_whitespace(input)?;

        // Parse entries
        let mut current_input = input;
        for i in 0..count {
            let obj_num = first_obj + i as u32;
            let (input, entry) = parse_xref_entry(current_input)?;
            table.insert(obj_num, entry);
            current_input = input;
        }

        remaining = current_input;
    }
}

/// Parse a single xref entry (20 bytes: offset generation n/f).
fn parse_xref_entry(input: &[u8]) -> IResult<&[u8], XrefEntry> {
    // Format: nnnnnnnnnn ggggg n/f (10 digits, space, 5 digits, space, n/f, EOL)
    let (input, _) = skip_whitespace(input)?;

    let (input, offset_str) = take_while1(|c: u8| c.is_ascii_digit())(input)?;
    let (input, _) = multispace1(input)?;

    let (input, gen_str) = take_while1(|c: u8| c.is_ascii_digit())(input)?;
    let (input, _) = multispace1(input)?;

    let (input, flag) = one_of("nf")(input)?;

    let offset: u64 = std::str::from_utf8(offset_str)
        .map_err(|_| nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Digit)))?
        .parse()
        .map_err(|_| nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Digit)))?;

    let generation: u16 = std::str::from_utf8(gen_str)
        .map_err(|_| nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Digit)))?
        .parse()
        .map_err(|_| nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Digit)))?;

    let entry = if flag == 'n' {
        XrefEntry::InUse { offset, generation }
    } else {
        XrefEntry::Free {
            next_free: offset as u32,
            generation,
        }
    };

    // Skip any trailing whitespace/EOL
    let (input, _) = opt(multispace1)(input)?;

    Ok((input, entry))
}

/// Parse an integer in xref context.
fn parse_xref_integer(input: &[u8]) -> IResult<&[u8], u32> {
    map_res(
        take_while1(|c: u8| c.is_ascii_digit()),
        |s: &[u8]| {
            std::str::from_utf8(s)
                .map_err(|_| "invalid utf8")
                .and_then(|s| s.parse::<u32>().map_err(|_| "invalid integer"))
        },
    )(input)
}

/// Find the startxref offset by scanning from the end of the file.
pub fn find_startxref(data: &[u8]) -> Result<u64, ParserError> {
    // Search backwards for startxref
    let search_start = if data.len() > 1024 {
        data.len() - 1024
    } else {
        0
    };

    let search_data = &data[search_start..];
    let startxref_pos = search_data
        .windows(9)
        .rposition(|w| w == b"startxref")
        .ok_or(ParserError::InvalidTrailer)?;

    let after_startxref = &search_data[startxref_pos + 9..];

    // Skip whitespace and parse the offset
    let (_, _) = skip_whitespace(after_startxref)
        .map_err(|_| ParserError::InvalidTrailer)?;

    let offset_start = after_startxref
        .iter()
        .position(|&c| c.is_ascii_digit())
        .ok_or(ParserError::InvalidTrailer)?;

    let offset_end = after_startxref[offset_start..]
        .iter()
        .position(|&c| !c.is_ascii_digit())
        .map(|p| offset_start + p)
        .unwrap_or(after_startxref.len());

    let offset_str = std::str::from_utf8(&after_startxref[offset_start..offset_end])
        .map_err(|_| ParserError::InvalidTrailer)?;

    offset_str
        .parse::<u64>()
        .map_err(|_| ParserError::InvalidTrailer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xref_entry_in_use() {
        let entry = XrefEntry::InUse {
            offset: 1234,
            generation: 0,
        };
        assert!(entry.is_in_use());
        assert_eq!(entry.offset(), Some(1234));
    }

    #[test]
    fn test_xref_entry_free() {
        let entry = XrefEntry::Free {
            next_free: 0,
            generation: 65535,
        };
        assert!(entry.is_free());
        assert_eq!(entry.offset(), None);
    }

    #[test]
    fn test_parse_xref_entry_in_use() {
        let input = b"0000000015 00000 n \n";
        let (_, entry) = parse_xref_entry(input).unwrap();
        match entry {
            XrefEntry::InUse { offset, generation } => {
                assert_eq!(offset, 15);
                assert_eq!(generation, 0);
            }
            _ => panic!("Expected InUse entry"),
        }
    }

    #[test]
    fn test_parse_xref_entry_free() {
        let input = b"0000000000 65535 f \n";
        let (_, entry) = parse_xref_entry(input).unwrap();
        match entry {
            XrefEntry::Free { next_free, generation } => {
                assert_eq!(next_free, 0);
                assert_eq!(generation, 65535);
            }
            _ => panic!("Expected Free entry"),
        }
    }

    #[test]
    fn test_parse_xref_table() {
        let input = b"xref\n0 3\n0000000000 65535 f \n0000000015 00000 n \n0000000100 00000 n \ntrailer";
        let (remaining, table) = parse_xref_table(input).unwrap();
        assert!(remaining.starts_with(b"trailer"));
        assert_eq!(table.len(), 3);
        assert!(table.get(0).unwrap().is_free());
        assert!(table.get(1).unwrap().is_in_use());
        assert!(table.get(2).unwrap().is_in_use());
    }

    #[test]
    fn test_find_startxref() {
        let data = b"%PDF-1.7\nsome content\nstartxref\n12345\n%%EOF";
        let offset = find_startxref(data).unwrap();
        assert_eq!(offset, 12345);
    }
}
