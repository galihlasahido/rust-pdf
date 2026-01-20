//! PDF trailer parsing.

use crate::error::ParserError;
use crate::object::{Object, PdfDictionary};
use crate::parser::lexer::parse_trailer as parse_trailer_keyword;
use crate::parser::lexer::skip_whitespace;
use crate::parser::objects::parse_object;
use crate::types::ObjectId;
use nom::IResult;

/// Parsed PDF trailer information.
#[derive(Debug)]
pub struct Trailer {
    /// The trailer dictionary.
    pub dict: PdfDictionary,
    /// Reference to the catalog (root) object.
    pub root: ObjectId,
    /// Reference to the info dictionary (optional).
    pub info: Option<ObjectId>,
    /// Reference to the encryption dictionary (optional).
    pub encrypt: Option<ObjectId>,
    /// File ID array (optional).
    pub id: Option<(Vec<u8>, Vec<u8>)>,
    /// Previous xref offset (for incremental updates).
    pub prev: Option<u64>,
    /// Size of the xref table.
    pub size: u32,
}

impl Trailer {
    /// Creates a Trailer from a dictionary.
    pub fn from_dictionary(dict: PdfDictionary) -> Result<Self, ParserError> {
        // Get required Root reference
        let root = match dict.get("Root") {
            Some(Object::Reference(id)) => *id,
            _ => return Err(ParserError::InvalidTrailer),
        };

        // Get required Size
        let size = match dict.get("Size") {
            Some(Object::Integer(n)) => *n as u32,
            _ => return Err(ParserError::InvalidTrailer),
        };

        // Get optional Info reference
        let info = match dict.get("Info") {
            Some(Object::Reference(id)) => Some(*id),
            _ => None,
        };

        // Get optional Encrypt reference
        let encrypt = match dict.get("Encrypt") {
            Some(Object::Reference(id)) => Some(*id),
            Some(Object::Dictionary(_)) => {
                // Inline encryption dictionary - not supported
                return Err(ParserError::UnsupportedFeature(
                    "inline encryption dictionary".to_string(),
                ));
            }
            _ => None,
        };

        // Get optional ID array
        let id = match dict.get("ID") {
            Some(Object::Array(arr)) if arr.len() >= 2 => {
                let id1 = match arr.get(0) {
                    Some(Object::String(s)) => s.as_bytes().to_vec(),
                    _ => return Err(ParserError::InvalidTrailer),
                };
                let id2 = match arr.get(1) {
                    Some(Object::String(s)) => s.as_bytes().to_vec(),
                    _ => return Err(ParserError::InvalidTrailer),
                };
                Some((id1, id2))
            }
            _ => None,
        };

        // Get optional Prev (previous xref offset)
        let prev = match dict.get("Prev") {
            Some(Object::Integer(n)) => Some(*n as u64),
            _ => None,
        };

        Ok(Self {
            dict,
            root,
            info,
            encrypt,
            id,
            prev,
            size,
        })
    }
}

/// Parse the trailer section.
pub fn parse_trailer(input: &[u8]) -> IResult<&[u8], PdfDictionary> {
    let (input, _) = parse_trailer_keyword(input)?;
    let (input, _) = skip_whitespace(input)?;

    let (input, obj) = parse_object(input)?;

    match obj {
        Object::Dictionary(dict) => Ok((input, dict)),
        _ => Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_trailer() {
        let input = b"trailer\n<< /Root 1 0 R /Size 10 >>\nstartxref";
        let (remaining, dict) = parse_trailer(input).unwrap();
        assert!(remaining.starts_with(b"\nstartxref"));
        assert!(dict.get("Root").is_some());
        assert!(dict.get("Size").is_some());
    }

    #[test]
    fn test_trailer_from_dictionary() {
        let mut dict = PdfDictionary::new();
        dict.set("Root", Object::Reference((1, 0).into()));
        dict.set("Size", Object::Integer(10));
        dict.set("Info", Object::Reference((2, 0).into()));

        let trailer = Trailer::from_dictionary(dict).unwrap();
        assert_eq!(trailer.root.number, 1);
        assert_eq!(trailer.size, 10);
        assert!(trailer.info.is_some());
    }

    #[test]
    fn test_trailer_missing_root() {
        let mut dict = PdfDictionary::new();
        dict.set("Size", Object::Integer(10));

        let result = Trailer::from_dictionary(dict);
        assert!(result.is_err());
    }
}
