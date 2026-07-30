//! Inline image parsing (ISO 32000-1:2008 Section 8.9.7).
//!
//! Inline images are embedded directly in a content stream using the
//! `BI` (Begin Image) ... `ID` (Image Data) ... `EI` (End Image) operators,
//! e.g.:
//!
//! ```text
//! BI
//!   /W 16 /H 16 /BPC 8 /CS /RGB /F /AHx
//! ID
//!   <...hex-encoded image data...>
//! EI
//! ```
//!
//! Unlike a regular stream object, the dictionary entries are written
//! directly as key/value pairs (no enclosing `<< >>`), commonly using the
//! abbreviated key/filter names from Table 93 (`W`, `H`, `BPC`, `CS`, `F`,
//! `DP`, `D`, `IM`, `I`, or their unabbreviated equivalents). The image
//! data's length is almost never declared explicitly (the optional `/L`
//! key was only added in ISO 32000-2:2020 7.8.5), so - like every other
//! real-world-tolerant PDF reader - this parser locates the end of the
//! data by scanning for a whitespace-delimited `EI` token.

use crate::object::{Object, PdfDictionary};
use crate::parser::lexer::{parse_name, skip_whitespace};
use crate::parser::objects::parse_object;
use nom::bytes::complete::tag;
use nom::IResult;

/// A parsed inline image: its (possibly abbreviated-key) dictionary and
/// raw, still-filtered image data.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineImage {
    /// The inline image's parameter dictionary (e.g. `/W`, `/H`, `/F`).
    pub dictionary: PdfDictionary,
    /// The raw image data between `ID` and `EI`, filtered but not yet
    /// decoded (see [`crate::filter::decode_filter`], which understands
    /// the abbreviated filter names used in inline images).
    pub data: Vec<u8>,
}

/// Maximum number of key/value pairs allowed in an inline image
/// dictionary, bounding work on adversarial input that never produces an
/// `ID` token.
const MAX_INLINE_DICT_ENTRIES: usize = 256;

/// Parses one inline image, starting at `BI` and ending just after `EI`.
pub fn parse_inline_image(input: &[u8]) -> IResult<&[u8], InlineImage> {
    let (input, _) = skip_whitespace(input)?;
    let (mut input, _) = tag(b"BI")(input)?;

    let mut dictionary = PdfDictionary::new();
    loop {
        let (after_ws, _) = skip_whitespace(input)?;

        if after_ws.starts_with(b"ID") {
            input = &after_ws[2..];
            break;
        }

        if dictionary.len() >= MAX_INLINE_DICT_ENTRIES {
            return Err(nom::Err::Error(nom::error::Error::new(
                after_ws,
                nom::error::ErrorKind::TooLarge,
            )));
        }

        let (after_key, key) = parse_name(after_ws)?;
        let (after_key_ws, _) = skip_whitespace(after_key)?;
        let (after_value, value) = parse_object(after_key_ws)?;
        dictionary.set(key, value);
        input = after_value;
    }

    // Per 8.9.7, exactly one whitespace character follows ID before the
    // data begins; consume it if present (be lenient if it's missing).
    if let Some(&b) = input.first() {
        if is_ws(b) {
            input = &input[1..];
        }
    }

    let declared_len = match dictionary.get("L").or_else(|| dictionary.get("Length")) {
        Some(Object::Integer(n)) if *n >= 0 => Some(*n as usize),
        _ => None,
    };

    let (data, remaining) = match declared_len {
        Some(len) if len <= input.len() => {
            let data = input[..len].to_vec();
            let after_data = &input[len..];
            let (after_ws, _) = skip_whitespace(after_data)?;
            match tag::<_, _, nom::error::Error<&[u8]>>(b"EI")(after_ws) {
                Ok((rest, _)) => (data, rest),
                Err(_) => scan_for_ei(input)?,
            }
        }
        _ => scan_for_ei(input)?,
    };

    Ok((remaining, InlineImage { dictionary, data }))
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0x0c | 0x00)
}

/// Result of [`scan_for_ei`]: the image data found, and the remaining
/// input immediately after the `EI` terminator.
type EiScanResult<'a> = Result<(Vec<u8>, &'a [u8]), nom::Err<nom::error::Error<&'a [u8]>>>;

/// Scans for a whitespace-delimited `EI` token (the end-of-image marker),
/// returning the data preceding it (with the single separating whitespace
/// byte stripped) and the remaining input after `EI`.
fn scan_for_ei(input: &[u8]) -> EiScanResult<'_> {
    let mut i = 0usize;
    while i + 1 < input.len() {
        if input[i] == b'E' && input[i + 1] == b'I' {
            let preceded_by_ws = i == 0 || is_ws(input[i - 1]);
            let followed_by_ws_or_eof = i + 2 >= input.len() || is_ws(input[i + 2]);
            if preceded_by_ws && followed_by_ws_or_eof {
                let data_end = if i > 0 && is_ws(input[i - 1]) { i - 1 } else { i };
                let data = input[..data_end].to_vec();
                let remaining = &input[(i + 2).min(input.len())..];
                return Ok((data, remaining));
            }
        }
        i += 1;
    }

    Err(nom::Err::Error(nom::error::Error::new(
        input,
        nom::error::ErrorKind::Eof,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_inline_image() {
        let input = b"BI /W 2 /H 2 /BPC 8 /CS /G /F /AHx ID FFFF0000> EI Q";
        let (remaining, img) = parse_inline_image(input).unwrap();
        assert_eq!(img.dictionary.get("W"), Some(&Object::Integer(2)));
        assert_eq!(img.data, b"FFFF0000>");
        assert_eq!(remaining, b" Q");
    }

    #[test]
    fn parses_binary_data_containing_ei_bytes() {
        // Raw binary data that happens to contain the byte sequence "EI"
        // *without* whitespace around it must not be treated as the
        // terminator.
        let mut data = vec![0x01, 0x02, b'E', b'I', 0x03, 0x04];
        let mut input = b"BI /W 1 /H 1 /BPC 8 /CS /G ID ".to_vec();
        input.extend_from_slice(&data);
        input.extend_from_slice(b" EI");

        let (remaining, img) = parse_inline_image(&input).unwrap();
        assert!(remaining.is_empty());
        // The false "EI" inside the data must be preserved verbatim.
        let expected = std::mem::take(&mut data);
        assert_eq!(img.data, expected);
    }

    #[test]
    fn parses_with_explicit_length_key() {
        let input = b"BI /W 1 /H 1 /BPC 8 /CS /G /L 4 ID \x00\x01\x02\x03 EI";
        let (remaining, img) = parse_inline_image(input).unwrap();
        assert!(remaining.is_empty());
        assert_eq!(img.data, vec![0x00, 0x01, 0x02, 0x03]);
    }

    #[test]
    fn missing_ei_is_rejected_not_panicking() {
        let input = b"BI /W 1 /H 1 ID \x00\x01\x02\x03 no terminator here";
        let result = parse_inline_image(input);
        assert!(result.is_err());
    }

    #[test]
    fn missing_id_is_rejected_not_panicking() {
        let input = b"BI /W 1 /H 1 /BPC 8 no ID keyword at all, just garbage until eof";
        let result = parse_inline_image(input);
        assert!(result.is_err());
    }

    #[test]
    fn oversized_dictionary_is_rejected_not_looping_forever() {
        let mut input = b"BI ".to_vec();
        for i in 0..10_000 {
            input.extend_from_slice(format!("/K{} {} ", i, i).as_bytes());
        }
        input.extend_from_slice(b"ID data EI");
        let result = parse_inline_image(&input);
        assert!(result.is_err());
    }

    #[test]
    fn abbreviated_filter_name_is_preserved_for_later_decoding() {
        let input = b"BI /W 1 /H 1 /BPC 8 /CS /G /F /Fl ID \x78\x9c\x03\x00\x00\x00\x00\x01 EI";
        let (_, img) = parse_inline_image(input).unwrap();
        match img.dictionary.get("F") {
            Some(Object::Name(n)) => assert_eq!(n.as_str(), "Fl"),
            other => panic!("expected /F /Fl, got {:?}", other),
        }
    }
}
