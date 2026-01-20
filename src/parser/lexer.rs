//! PDF lexer - token definitions and basic tokenization.

use nom::{
    branch::alt,
    bytes::complete::{tag, take_while},
    character::complete::{char, digit1, multispace1, one_of},
    combinator::{map_res, opt, recognize, value},
    multi::many0,
    sequence::{pair, tuple},
    IResult,
};

/// A PDF token.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum Token<'a> {
    /// Boolean value
    Boolean(bool),
    /// Integer value
    Integer(i64),
    /// Real (floating point) value
    Real(f64),
    /// Literal string (parentheses)
    LiteralString(Vec<u8>),
    /// Hexadecimal string
    HexString(Vec<u8>),
    /// Name (starts with /)
    Name(&'a str),
    /// Array start [
    ArrayStart,
    /// Array end ]
    ArrayEnd,
    /// Dictionary start <<
    DictStart,
    /// Dictionary end >>
    DictEnd,
    /// Null value
    Null,
    /// Object reference (obj_num gen_num R)
    Reference(u32, u16),
    /// Object definition start
    ObjStart(u32, u16),
    /// Object definition end
    ObjEnd,
    /// Stream keyword
    Stream,
    /// Endstream keyword
    EndStream,
    /// Xref keyword
    Xref,
    /// Trailer keyword
    Trailer,
    /// StartXref keyword
    StartXref,
    /// EOF marker
    Eof,
    /// Raw keyword (for unrecognized keywords)
    Keyword(&'a str),
}

/// Skip whitespace and comments.
pub fn skip_whitespace(input: &[u8]) -> IResult<&[u8], ()> {
    let (input, _) = many0(alt((
        value((), multispace1),
        value((), comment),
    )))(input)?;
    Ok((input, ()))
}

/// Parse a PDF comment (% to end of line).
fn comment(input: &[u8]) -> IResult<&[u8], &[u8]> {
    let (input, _) = char('%')(input)?;
    let (input, content) = take_while(|c| c != b'\n' && c != b'\r')(input)?;
    let (input, _) = opt(alt((tag(b"\r\n"), tag(b"\n"), tag(b"\r"))))(input)?;
    Ok((input, content))
}

/// Parse a boolean value.
pub fn parse_boolean(input: &[u8]) -> IResult<&[u8], bool> {
    alt((
        value(true, tag(b"true")),
        value(false, tag(b"false")),
    ))(input)
}

/// Parse an integer.
pub fn parse_integer(input: &[u8]) -> IResult<&[u8], i64> {
    map_res(
        recognize(pair(
            opt(one_of("+-")),
            digit1,
        )),
        |s: &[u8]| {
            std::str::from_utf8(s)
                .map_err(|_| "invalid utf8")
                .and_then(|s| s.parse::<i64>().map_err(|_| "invalid integer"))
        },
    )(input)
}

/// Parse a real number.
#[allow(dead_code)]
pub fn parse_real(input: &[u8]) -> IResult<&[u8], f64> {
    map_res(
        recognize(tuple((
            opt(one_of("+-")),
            alt((
                // .123 or 123.456 or 123.
                recognize(tuple((
                    opt(digit1),
                    char('.'),
                    opt(digit1),
                ))),
                // 123 (integer part only, will be parsed as integer first)
                digit1,
            )),
        ))),
        |s: &[u8]| {
            std::str::from_utf8(s)
                .map_err(|_| "invalid utf8")
                .and_then(|s| s.parse::<f64>().map_err(|_| "invalid real"))
        },
    )(input)
}

/// Parse a PDF name (starts with /).
pub fn parse_name(input: &[u8]) -> IResult<&[u8], &str> {
    let (input, _) = char('/')(input)?;
    let (input, name_bytes) = take_while(is_name_char)(input)?;
    let name = std::str::from_utf8(name_bytes).map_err(|_| {
        nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Char))
    })?;
    Ok((input, name))
}

/// Check if a character is valid in a PDF name.
fn is_name_char(c: u8) -> bool {
    // Name characters are any character except whitespace and delimiters
    !matches!(c,
        b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'\0' | // whitespace
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%' // delimiters
    )
}

/// Parse a literal string (delimited by parentheses).
pub fn parse_literal_string(input: &[u8]) -> IResult<&[u8], Vec<u8>> {
    let (mut input, _) = char('(')(input)?;
    let mut result = Vec::new();
    let mut paren_depth = 1;

    while paren_depth > 0 {
        if input.is_empty() {
            return Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Eof,
            )));
        }

        let c = input[0];
        input = &input[1..];

        match c {
            b'(' => {
                paren_depth += 1;
                result.push(c);
            }
            b')' => {
                paren_depth -= 1;
                if paren_depth > 0 {
                    result.push(c);
                }
            }
            b'\\' => {
                // Escape sequence
                if input.is_empty() {
                    return Err(nom::Err::Error(nom::error::Error::new(
                        input,
                        nom::error::ErrorKind::Eof,
                    )));
                }
                let escaped = input[0];
                input = &input[1..];
                match escaped {
                    b'n' => result.push(b'\n'),
                    b'r' => result.push(b'\r'),
                    b't' => result.push(b'\t'),
                    b'b' => result.push(0x08), // backspace
                    b'f' => result.push(0x0c), // form feed
                    b'(' => result.push(b'('),
                    b')' => result.push(b')'),
                    b'\\' => result.push(b'\\'),
                    b'\r' | b'\n' => {
                        // Line continuation - skip
                        if escaped == b'\r' && !input.is_empty() && input[0] == b'\n' {
                            input = &input[1..];
                        }
                    }
                    b'0'..=b'7' => {
                        // Octal escape
                        let mut octal_val = (escaped - b'0') as u8;
                        for _ in 0..2 {
                            if !input.is_empty() && input[0] >= b'0' && input[0] <= b'7' {
                                octal_val = octal_val * 8 + (input[0] - b'0');
                                input = &input[1..];
                            } else {
                                break;
                            }
                        }
                        result.push(octal_val);
                    }
                    _ => {
                        // Unknown escape, just use the character
                        result.push(escaped);
                    }
                }
            }
            _ => result.push(c),
        }
    }

    Ok((input, result))
}

/// Parse a hexadecimal string (delimited by angle brackets).
pub fn parse_hex_string(input: &[u8]) -> IResult<&[u8], Vec<u8>> {
    let (input, _) = char('<')(input)?;
    let (input, hex_chars) = take_while(|c: u8| c.is_ascii_hexdigit() || c.is_ascii_whitespace())(input)?;
    let (input, _) = char('>')(input)?;

    // Remove whitespace and convert hex to bytes
    let hex_str: String = hex_chars
        .iter()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|&c| c as char)
        .collect();

    // Pad with 0 if odd number of digits
    let hex_str = if hex_str.len() % 2 == 1 {
        format!("{}0", hex_str)
    } else {
        hex_str
    };

    let bytes: Vec<u8> = (0..hex_str.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex_str[i..i + 2], 16).unwrap_or(0))
        .collect();

    Ok((input, bytes))
}

/// Parse null.
pub fn parse_null(input: &[u8]) -> IResult<&[u8], ()> {
    value((), tag(b"null"))(input)
}

/// Parse array start.
pub fn parse_array_start(input: &[u8]) -> IResult<&[u8], ()> {
    value((), char('['))(input)
}

/// Parse array end.
pub fn parse_array_end(input: &[u8]) -> IResult<&[u8], ()> {
    value((), char(']'))(input)
}

/// Parse dictionary start.
pub fn parse_dict_start(input: &[u8]) -> IResult<&[u8], ()> {
    value((), tag(b"<<"))(input)
}

/// Parse dictionary end.
pub fn parse_dict_end(input: &[u8]) -> IResult<&[u8], ()> {
    value((), tag(b">>"))(input)
}

/// Parse stream keyword.
pub fn parse_stream(input: &[u8]) -> IResult<&[u8], ()> {
    let (input, _) = tag(b"stream")(input)?;
    // Stream keyword must be followed by EOL
    let (input, _) = alt((tag(b"\r\n"), tag(b"\n"), tag(b"\r")))(input)?;
    Ok((input, ()))
}

/// Parse endstream keyword.
pub fn parse_endstream(input: &[u8]) -> IResult<&[u8], ()> {
    value((), tag(b"endstream"))(input)
}

/// Parse xref keyword.
pub fn parse_xref(input: &[u8]) -> IResult<&[u8], ()> {
    value((), tag(b"xref"))(input)
}

/// Parse trailer keyword.
pub fn parse_trailer(input: &[u8]) -> IResult<&[u8], ()> {
    value((), tag(b"trailer"))(input)
}

/// Parse startxref keyword.
#[allow(dead_code)]
pub fn parse_startxref(input: &[u8]) -> IResult<&[u8], ()> {
    value((), tag(b"startxref"))(input)
}

/// Parse EOF marker.
#[allow(dead_code)]
pub fn parse_eof(input: &[u8]) -> IResult<&[u8], ()> {
    value((), tag(b"%%EOF"))(input)
}

/// Parse obj keyword (object definition start).
pub fn parse_obj(input: &[u8]) -> IResult<&[u8], ()> {
    value((), tag(b"obj"))(input)
}

/// Parse endobj keyword.
pub fn parse_endobj(input: &[u8]) -> IResult<&[u8], ()> {
    value((), tag(b"endobj"))(input)
}

/// Parse R keyword (reference).
pub fn parse_r(input: &[u8]) -> IResult<&[u8], ()> {
    value((), char('R'))(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_boolean() {
        assert_eq!(parse_boolean(b"true"), Ok((&b""[..], true)));
        assert_eq!(parse_boolean(b"false"), Ok((&b""[..], false)));
    }

    #[test]
    fn test_parse_integer() {
        assert_eq!(parse_integer(b"123"), Ok((&b""[..], 123)));
        assert_eq!(parse_integer(b"-456"), Ok((&b""[..], -456)));
        assert_eq!(parse_integer(b"+789"), Ok((&b""[..], 789)));
    }

    #[test]
    fn test_parse_real() {
        assert_eq!(parse_real(b"3.14"), Ok((&b""[..], 3.14)));
        assert_eq!(parse_real(b"-1.5"), Ok((&b""[..], -1.5)));
        assert_eq!(parse_real(b".5"), Ok((&b""[..], 0.5)));
    }

    #[test]
    fn test_parse_name() {
        assert_eq!(parse_name(b"/Type"), Ok((&b""[..], "Type")));
        assert_eq!(parse_name(b"/Font"), Ok((&b""[..], "Font")));
    }

    #[test]
    fn test_parse_literal_string() {
        assert_eq!(
            parse_literal_string(b"(Hello)"),
            Ok((&b""[..], b"Hello".to_vec()))
        );
        assert_eq!(
            parse_literal_string(b"(Hello\\nWorld)"),
            Ok((&b""[..], b"Hello\nWorld".to_vec()))
        );
        assert_eq!(
            parse_literal_string(b"(Nested (parens) here)"),
            Ok((&b""[..], b"Nested (parens) here".to_vec()))
        );
    }

    #[test]
    fn test_parse_hex_string() {
        assert_eq!(
            parse_hex_string(b"<48656C6C6F>"),
            Ok((&b""[..], b"Hello".to_vec()))
        );
        assert_eq!(
            parse_hex_string(b"<48 65 6C 6C 6F>"),
            Ok((&b""[..], b"Hello".to_vec()))
        );
    }
}
