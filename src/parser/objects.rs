//! PDF object parsing (ISO 32000-1:2008 Section 7.3 "Objects").

use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfStream, PdfString};
use crate::parser::lexer::*;
use nom::{branch::alt, combinator::map, IResult};

/// Maximum nesting depth allowed for arrays/dictionaries while parsing a
/// single object. PDF does not mandate a limit, but an attacker can craft
/// a tiny file consisting of thousands of nested `[[[[...]]]]` or
/// `<<something...>>` tokens; because this parser is a straightforward
/// recursive-descent parser, unbounded nesting would recurse the host
/// stack and crash the process (a denial-of-service on untrusted input).
/// 64 levels is far beyond anything a legitimate PDF content structure
/// needs (Resources/Font/Encoding graphs are typically < 10 levels deep).
const MAX_NESTING_DEPTH: u32 = 64;

fn nesting_error(input: &[u8]) -> nom::Err<nom::error::Error<&[u8]>> {
    nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::TooLarge))
}

/// Parse any PDF object.
pub fn parse_object(input: &[u8]) -> IResult<&[u8], Object> {
    parse_object_at(input, 0)
}

fn parse_object_at(input: &[u8], depth: u32) -> IResult<&[u8], Object> {
    let (input, _) = skip_whitespace(input)?;

    alt((
        // Try reference first (n n R)
        parse_reference_object,
        // Then try other objects
        map(parse_boolean, Object::Boolean),
        parse_number_object, // handles both Integer and Real
        map(parse_literal_string, |s| Object::String(PdfString::Literal(s))),
        map(parse_hex_string, |s| Object::String(PdfString::Hex(s))),
        map(parse_name, |s| Object::Name(PdfName::new_unchecked(s))),
        move |i| parse_array_object(i, depth),
        move |i| parse_dictionary_or_stream(i, depth),
        map(parse_null, |_| Object::Null),
    ))(input)
}

/// Parse a reference (n n R).
fn parse_reference_object(input: &[u8]) -> IResult<&[u8], Object> {
    let (input, obj_num) = parse_integer(input)?;
    let (input, _) = skip_whitespace(input)?;
    let (input, gen_num) = parse_integer(input)?;
    let (input, _) = skip_whitespace(input)?;
    let (input, _) = parse_r(input)?;

    if obj_num < 0 || gen_num < 0 {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Verify,
        )));
    }

    Ok((input, Object::Reference((obj_num as u32, gen_num as u16).into())))
}

/// Parse a number (integer or real).
fn parse_number_object(input: &[u8]) -> IResult<&[u8], Object> {
    // Try to determine if it's an integer or real
    // A real has a decimal point
    let (remaining, num_str) = nom::bytes::complete::take_while1(|c: u8| {
        c.is_ascii_digit() || c == b'+' || c == b'-' || c == b'.'
    })(input)?;

    let s = std::str::from_utf8(num_str).map_err(|_| {
        nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Char))
    })?;

    if s.contains('.') {
        // Parse as real
        let val: f64 = s.parse().map_err(|_| {
            nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Float))
        })?;
        Ok((remaining, Object::Real(val)))
    } else {
        // Parse as integer
        let val: i64 = s.parse().map_err(|_| {
            nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Digit))
        })?;
        Ok((remaining, Object::Integer(val)))
    }
}

/// Parse an array.
fn parse_array_object(input: &[u8], depth: u32) -> IResult<&[u8], Object> {
    if depth >= MAX_NESTING_DEPTH {
        return Err(nesting_error(input));
    }
    let (input, _) = parse_array_start(input)?;
    let (input, _) = skip_whitespace(input)?;

    let mut array = PdfArray::new();
    let mut remaining = input;

    loop {
        let (input, _) = skip_whitespace(remaining)?;

        // Check for array end
        if let Ok((input, _)) = parse_array_end(input) {
            return Ok((input, Object::Array(array)));
        }

        // Parse next element
        let (input, obj) = parse_object_at(input, depth + 1)?;
        array.push(obj);
        remaining = input;
    }
}

/// Parse a dictionary or stream.
fn parse_dictionary_or_stream(input: &[u8], depth: u32) -> IResult<&[u8], Object> {
    if depth >= MAX_NESTING_DEPTH {
        return Err(nesting_error(input));
    }
    let (input, _) = parse_dict_start(input)?;
    let (input, _) = skip_whitespace(input)?;

    let mut dict = PdfDictionary::new();
    let mut remaining = input;

    // Parse dictionary entries
    loop {
        let (input, _) = skip_whitespace(remaining)?;

        // Check for dictionary end
        if let Ok((input, _)) = parse_dict_end(input) {
            remaining = input;
            break;
        }

        // Parse key (name)
        let (input, key) = parse_name(input)?;
        let (input, _) = skip_whitespace(input)?;

        // Parse value
        let (input, value) = parse_object_at(input, depth + 1)?;
        dict.set(key, value);
        remaining = input;
    }

    // Check if this is followed by a stream
    // Use a separate variable to avoid consuming whitespace if not a stream
    let (after_ws, _) = skip_whitespace(remaining)?;

    if let Ok((stream_input, _)) = parse_stream(after_ws) {
        // This is a stream object. /Length is normally a direct integer,
        // but real-world producers frequently emit an indirect reference
        // instead (ISO 32000-1 7.3.8.2 permits this); we cannot resolve
        // indirect references from inside the object grammar (no xref
        // table is available here), so fall back to scanning for the next
        // `endstream` keyword, which is what most lenient real-world
        // readers do.
        let by_declared_length = match dict.get("Length") {
            Some(Object::Integer(len)) if *len >= 0 && (*len as usize) <= stream_input.len() => {
                let length = *len as usize;
                let data = stream_input[..length].to_vec();
                let after_data = &stream_input[length..];
                skip_whitespace(after_data)
                    .ok()
                    .and_then(|(after_ws, _)| parse_endstream(after_ws).ok())
                    .map(|(final_input, _)| (data, final_input))
            }
            _ => None,
        };

        let (stream_data, final_input) = match by_declared_length {
            Some(result) => result,
            // The declared Length was missing, an unresolved indirect
            // reference, or didn't line up with "endstream"; fall back to
            // scanning, which tolerates a wrong/stale Length value in
            // corrupt files.
            None => scan_for_endstream(stream_input)?,
        };

        let stream = PdfStream::with_dictionary(dict, stream_data);
        Ok((final_input, Object::Stream(stream)))
    } else {
        // Just a dictionary - return without consuming trailing whitespace
        Ok((remaining, Object::Dictionary(dict)))
    }
}

/// Result of [`scan_for_endstream`]: the stream data found, and the
/// remaining input immediately after the `endstream` keyword.
type EndstreamScanResult<'a> = Result<(Vec<u8>, &'a [u8]), nom::Err<nom::error::Error<&'a [u8]>>>;

/// Recovery-oriented stream-length fallback: scans forward for the next
/// `endstream` keyword and treats everything before it (minus a single
/// trailing EOL, if present) as the stream's data.
fn scan_for_endstream(stream_input: &[u8]) -> EndstreamScanResult<'_> {
    let pos = find_subsequence(stream_input, b"endstream").ok_or_else(|| {
        nom::Err::Error(nom::error::Error::new(stream_input, nom::error::ErrorKind::Eof))
    })?;

    let mut data_end = pos;
    // Strip a single trailing EOL (CRLF, LF, or CR) that belongs to the
    // stream syntax rather than the data itself.
    if data_end >= 2 && &stream_input[data_end - 2..data_end] == b"\r\n" {
        data_end -= 2;
    } else if data_end >= 1 && (stream_input[data_end - 1] == b'\n' || stream_input[data_end - 1] == b'\r') {
        data_end -= 1;
    }

    let data = stream_input[..data_end].to_vec();
    let final_input = &stream_input[pos + b"endstream".len()..];
    Ok((data, final_input))
}

/// Finds the first occurrence of `needle` in `haystack`, if any.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Parse an indirect object definition (n n obj ... endobj).
pub fn parse_indirect_object(input: &[u8]) -> IResult<&[u8], (u32, u16, Object)> {
    let (input, _) = skip_whitespace(input)?;
    let (input, obj_num) = parse_integer(input)?;
    let (input, _) = skip_whitespace(input)?;
    let (input, gen_num) = parse_integer(input)?;
    let (input, _) = skip_whitespace(input)?;
    let (input, _) = parse_obj(input)?;
    let (input, _) = skip_whitespace(input)?;

    let (input, obj) = parse_object(input)?;

    let (input, _) = skip_whitespace(input)?;
    let (input, _) = parse_endobj(input)?;

    if obj_num < 0 || gen_num < 0 {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Verify,
        )));
    }

    Ok((input, (obj_num as u32, gen_num as u16, obj)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_integer_object() {
        let (_, obj) = parse_object(b"42").unwrap();
        assert_eq!(obj, Object::Integer(42));
    }

    #[test]
    fn test_parse_real_object() {
        let (_, obj) = parse_object(b"3.25").unwrap();
        assert_eq!(obj, Object::Real(3.25));
    }

    #[test]
    fn test_parse_name_object() {
        let (_, obj) = parse_object(b"/Type").unwrap();
        assert_eq!(obj, Object::Name(PdfName::new_unchecked("Type")));
    }

    #[test]
    fn test_parse_string_object() {
        let (_, obj) = parse_object(b"(Hello)").unwrap();
        match obj {
            Object::String(PdfString::Literal(s)) => assert_eq!(s, b"Hello"),
            _ => panic!("Expected literal string"),
        }
    }

    #[test]
    fn test_parse_array_object() {
        let (_, obj) = parse_object(b"[1 2 3]").unwrap();
        match obj {
            Object::Array(arr) => {
                assert_eq!(arr.len(), 3);
            }
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_parse_dictionary_object() {
        let (_, obj) = parse_object(b"<< /Type /Page /Count 5 >>").unwrap();
        match obj {
            Object::Dictionary(dict) => {
                assert!(dict.get("Type").is_some());
                assert!(dict.get("Count").is_some());
            }
            _ => panic!("Expected dictionary"),
        }
    }

    #[test]
    fn test_parse_reference_object() {
        let (_, obj) = parse_object(b"10 0 R").unwrap();
        match obj {
            Object::Reference(id) => {
                assert_eq!(id.number, 10);
                assert_eq!(id.generation, 0);
            }
            _ => panic!("Expected reference"),
        }
    }

    #[test]
    fn test_parse_indirect_object() {
        let input = b"1 0 obj\n<< /Type /Catalog >>\nendobj";
        let (_, (num, gen, obj)) = parse_indirect_object(input).unwrap();
        assert_eq!(num, 1);
        assert_eq!(gen, 0);
        match obj {
            Object::Dictionary(dict) => {
                assert!(dict.get("Type").is_some());
            }
            _ => panic!("Expected dictionary"),
        }
    }

    #[test]
    fn test_parse_stream_with_indirect_length_falls_back_to_scan() {
        // /Length refers to object 5 0 R, which cannot be resolved here;
        // the parser must fall back to scanning for "endstream".
        let input = b"1 0 obj\n<< /Length 5 0 R >>\nstream\nHello World\nendstream\nendobj";
        let (_, (_, _, obj)) = parse_indirect_object(input).unwrap();
        match obj {
            Object::Stream(s) => assert_eq!(s.data(), b"Hello World"),
            _ => panic!("Expected stream"),
        }
    }

    #[test]
    fn test_parse_stream_with_wrong_length_falls_back_to_scan() {
        // Length is a direct integer but deliberately wrong (corrupt file);
        // the parser should still recover via the endstream scan.
        let input = b"1 0 obj\n<< /Length 999 >>\nstream\nHello World\nendstream\nendobj";
        let (_, (_, _, obj)) = parse_indirect_object(input).unwrap();
        match obj {
            Object::Stream(s) => assert_eq!(s.data(), b"Hello World"),
            _ => panic!("Expected stream"),
        }
    }

    #[test]
    fn test_deeply_nested_array_is_rejected_not_stack_overflow() {
        let input = vec![b'['; 10_000];
        // Deliberately no closing brackets / no valid inner object: this
        // must return a parse error rather than overflowing the stack.
        let result = parse_object(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_reasonably_nested_array_still_parses() {
        let depth = 10;
        let mut input = String::new();
        for _ in 0..depth {
            input.push('[');
        }
        input.push('1');
        for _ in 0..depth {
            input.push(']');
        }
        let result = parse_object(input.as_bytes());
        assert!(result.is_ok());
    }

    #[test]
    fn test_missing_endstream_is_rejected_not_panicking() {
        let input = b"1 0 obj\n<< /Length 5 0 R >>\nstream\nno end marker here\nendobj";
        let result = parse_indirect_object(input);
        assert!(result.is_err());
    }
}
