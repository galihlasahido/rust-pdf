//! PDF object parsing.

use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfStream, PdfString};
use crate::parser::lexer::*;
use nom::{
    branch::alt,
    combinator::map,
    IResult,
};

/// Parse any PDF object.
pub fn parse_object(input: &[u8]) -> IResult<&[u8], Object> {
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
        parse_array_object,
        parse_dictionary_or_stream,
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
fn parse_array_object(input: &[u8]) -> IResult<&[u8], Object> {
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
        let (input, obj) = parse_object(input)?;
        array.push(obj);
        remaining = input;
    }
}

/// Parse a dictionary or stream.
fn parse_dictionary_or_stream(input: &[u8]) -> IResult<&[u8], Object> {
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
        let (input, value) = parse_object(input)?;
        dict.set(key, value);
        remaining = input;
    }

    // Check if this is followed by a stream
    // Use a separate variable to avoid consuming whitespace if not a stream
    let (after_ws, _) = skip_whitespace(remaining)?;

    if let Ok((stream_input, _)) = parse_stream(after_ws) {
        // This is a stream object
        // Get the length from the dictionary
        let length = match dict.get("Length") {
            Some(Object::Integer(len)) => *len as usize,
            _ => {
                // Length might be a reference, which we can't resolve here
                // Return error for now
                return Err(nom::Err::Error(nom::error::Error::new(
                    stream_input,
                    nom::error::ErrorKind::Verify,
                )));
            }
        };

        // Read stream data
        if stream_input.len() < length {
            return Err(nom::Err::Error(nom::error::Error::new(
                stream_input,
                nom::error::ErrorKind::Eof,
            )));
        }

        let stream_data = stream_input[..length].to_vec();
        let after_data = &stream_input[length..];

        // Skip whitespace and endstream
        let (final_input, _) = skip_whitespace(after_data)?;
        let (final_input, _) = parse_endstream(final_input)?;

        let stream = PdfStream::with_dictionary(dict, stream_data);
        Ok((final_input, Object::Stream(stream)))
    } else {
        // Just a dictionary - return without consuming trailing whitespace
        Ok((remaining, Object::Dictionary(dict)))
    }
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
        let (_, obj) = parse_object(b"3.14").unwrap();
        assert_eq!(obj, Object::Real(3.14));
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
}
