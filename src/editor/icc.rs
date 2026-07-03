//! ICC output-intent embedding (ISO 32000-1:2008 14.11.5 "Output
//! Intents", Table 394/395) - used by PDF/A (ISO 19005-x, which mandates
//! at least one output intent, see [`crate::editor::pdfa`]) and PDF/X
//! (which mandates one whose profile's colour space matches the
//! document's process colour space, see [`crate::editor::pdfx`]).
//!
//! This module does **not** implement an ICC colour management engine
//! (no colour transforms, no rendering-intent math) and does not build
//! ICC profiles from scratch - profile *bytes* are always supplied by the
//! caller (e.g. a vendored `sRGB2014.icc` shipped by the embedding
//! desktop application), matching the crate's existing policy of not
//! reimplementing mature C/C++-grade functionality (see
//! `src/render/native/mod.rs`'s ICCBased-colour-space-approximation gap
//! for the same policy applied to rasterization). What this module
//! *does* do is:
//! - validate that the supplied bytes look like a structurally sane ICC
//!   profile before trusting them (the ICC.1 header layout referenced
//!   below), rejecting obvious garbage rather than embedding it silently;
//! - build the `/OutputIntent` dictionary and `/DestOutputProfile` stream
//!   object graph ISO 32000-1 Table 394/395 describes, and attach it to
//!   the catalog's `/OutputIntents` array (Table 29);
//! - read that graph back for the validators.
//!
//! # ICC header layout referenced here
//!
//! The ICC.1 profile header is a fixed 128-byte structure (this is
//! long-standing, extremely stable public specification structure -
//! unchanged since ICC.1:2001 through ICC.1:2010/2022 for every field
//! used here): bytes 0..4 profile size (big-endian `u32`), bytes
//! 12..16 device class, bytes 16..20 data colour space signature (e.g.
//! `b"RGB "`, `b"CMYK"`, `b"GRAY"`), bytes 36..40 the fixed signature
//! `b"acsp"` ("ICC profile file signature"). This module only reads those
//! fields - it does not parse the tag table or any tag's contents.
use super::graph::EditableDocument;
use crate::error::{EditorError, PdfResult};
use crate::object::{Object, PdfArray, PdfDictionary, PdfName, PdfStream};
use crate::types::ObjectId;

/// Minimum size of a syntactically plausible ICC profile: the fixed
/// 128-byte header alone (a real profile always has at least one tag on
/// top of that, but this module doesn't need to look at the tag table).
const ICC_HEADER_LEN: usize = 128;

/// Hard cap on the ICC profile size this crate will embed or read back.
/// Real display/print ICC profiles are typically a few hundred KB to a
/// few MB (large ones embed detailed 3D lookup tables); this generously
/// bounds that while still rejecting an absurd claimed size from a
/// crafted/corrupt file (untrusted-input rule).
const MAX_ICC_PROFILE_BYTES: usize = 32 * 1024 * 1024;

/// The ICC "data colour space" a profile declares (ICC.1 header bytes
/// 16..20), and which `/N` (ISO 32000-1 Table 95, "the number of colour
/// components") an `/OutputIntent`'s `/DestOutputProfile` stream must
/// declare to match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IccColorSpace {
    /// `b"GRAY"`: 1 component.
    Gray,
    /// `b"RGB "`: 3 components.
    Rgb,
    /// `b"CMYK"`: 4 components.
    Cmyk,
}

impl IccColorSpace {
    /// The `/N` value (ISO 32000-1 Table 95) for this colour space.
    pub fn component_count(self) -> i64 {
        match self {
            IccColorSpace::Gray => 1,
            IccColorSpace::Rgb => 3,
            IccColorSpace::Cmyk => 4,
        }
    }

    fn alternate_device_space(self) -> &'static str {
        match self {
            IccColorSpace::Gray => "DeviceGray",
            IccColorSpace::Rgb => "DeviceRGB",
            IccColorSpace::Cmyk => "DeviceCMYK",
        }
    }

    fn from_signature(sig: &[u8]) -> Option<Self> {
        match sig {
            b"GRAY" => Some(IccColorSpace::Gray),
            b"RGB " => Some(IccColorSpace::Rgb),
            b"CMYK" => Some(IccColorSpace::Cmyk),
            _ => None,
        }
    }
}

/// Why a caller-supplied byte string was rejected as an ICC profile by
/// [`validate_icc_header`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IccError {
    /// Fewer than [`ICC_HEADER_LEN`] bytes - too short to even hold the
    /// fixed header.
    #[error("ICC profile is smaller than the 128-byte fixed header")]
    TooShort,
    /// Longer than [`MAX_ICC_PROFILE_BYTES`].
    #[error("ICC profile exceeds the {MAX_ICC_PROFILE_BYTES}-byte safety limit")]
    TooLarge,
    /// Bytes 36..40 are not `b"acsp"`.
    #[error("missing ICC profile file signature ('acsp') at byte offset 36")]
    BadSignature,
    /// Bytes 16..20 are not a data colour space this crate recognizes
    /// (`GRAY`/`RGB `/`CMYK`) - other ICC colour spaces (Lab, XYZ, ...)
    /// exist but are not valid output-intent process colour spaces for
    /// PDF/A or PDF/X (ISO 32000-1 14.11.5).
    #[error("unrecognized/unsupported ICC data colour space signature")]
    UnsupportedColorSpace,
    /// The header's own declared profile size (bytes 0..4) is larger than
    /// the number of bytes actually supplied - internally inconsistent,
    /// and exactly the kind of claim that must never be trusted for
    /// allocation sizing (untrusted-input rule).
    #[error("ICC header declares a profile size larger than the supplied data")]
    DeclaredSizeExceedsData,
}

/// Validates that `data` has a structurally plausible ICC.1 header (see
/// the [module docs](self)) and returns its declared data colour space.
///
/// This is a structural sanity check, not full ICC.1 conformance
/// validation (it does not walk the tag table, verify per-tag checksums,
/// or validate any tag's contents) - sufficient to catch "this obviously
/// isn't an ICC profile" (empty file, truncated download, wrong file
/// entirely) before it gets embedded into a PDF/A/PDF/X output intent and
/// silently produces a file no real reader can colour-manage.
pub fn validate_icc_header(data: &[u8]) -> Result<IccColorSpace, IccError> {
    if data.len() > MAX_ICC_PROFILE_BYTES {
        return Err(IccError::TooLarge);
    }
    if data.len() < ICC_HEADER_LEN {
        return Err(IccError::TooShort);
    }
    let declared_size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if declared_size > data.len() {
        return Err(IccError::DeclaredSizeExceedsData);
    }
    if &data[36..40] != b"acsp" {
        return Err(IccError::BadSignature);
    }
    IccColorSpace::from_signature(&data[16..20]).ok_or(IccError::UnsupportedColorSpace)
}

/// Which `/S` (output-intent subtype, ISO 32000-1 Table 394) value to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputIntentSubtype {
    /// `/GTS_PDFA1` - the conventional value used by PDF/A output
    /// intents for every part (1/2/3), not just PDF/A-1; this is the
    /// value the PDF/A Competence Center's own reference files and
    /// veraPDF's test corpus use across parts, and is treated as the
    /// de-facto standard by every PDF/A producer this crate's author is
    /// aware of, though the author has not independently re-verified
    /// this specific string is normatively mandated by ISO 19005-2/3's
    /// own text (as opposed to just universal practice).
    PdfA,
    /// `/GTS_PDFX` - PDF/X output intents (ISO 15930-x).
    PdfX,
}

impl OutputIntentSubtype {
    fn as_name(self) -> &'static str {
        match self {
            OutputIntentSubtype::PdfA => "GTS_PDFA1",
            OutputIntentSubtype::PdfX => "GTS_PDFX",
        }
    }
}

/// An output intent read back from a document's `/OutputIntents` array.
#[derive(Debug, Clone)]
pub struct OutputIntentInfo {
    /// The `/S` subtype name (e.g. `"GTS_PDFA1"`).
    pub subtype: String,
    /// `/OutputConditionIdentifier`.
    pub identifier: Option<String>,
    /// Whether `/DestOutputProfile` is present and resolves to a stream
    /// whose bytes pass [`validate_icc_header`].
    pub has_valid_icc_profile: bool,
    /// The embedded profile's declared colour space, if it validated.
    pub color_space: Option<IccColorSpace>,
}

impl EditableDocument {
    /// Embeds `icc_profile` as a new `/OutputIntent` (ISO 32000-1
    /// 14.11.5) and appends it to the catalog's `/OutputIntents` array,
    /// creating that array if this is the first one.
    ///
    /// `identifier` is `/OutputConditionIdentifier` (e.g.
    /// `"sRGB IEC61966-2.1"` or a registered CMYK condition name like
    /// `"Coated FOGRA39"`); `condition` is the free-text
    /// `/OutputCondition`. Returns the new `/OutputIntent` dictionary's
    /// object id.
    pub fn add_output_intent(
        &mut self,
        icc_profile: &[u8],
        subtype: OutputIntentSubtype,
        identifier: &str,
        condition: &str,
    ) -> PdfResult<ObjectId> {
        let color_space = validate_icc_header(icc_profile)
            .map_err(|e| EditorError::InvalidArgument(format!("invalid ICC profile: {e}")))?;

        let mut profile_dict = PdfDictionary::new();
        profile_dict.set("N", Object::Integer(color_space.component_count()));
        profile_dict.set("Alternate", Object::Name(PdfName::new_unchecked(color_space.alternate_device_space())));
        let profile_stream = PdfStream::with_dictionary(profile_dict, icc_profile.to_vec());
        #[cfg(feature = "compression")]
        let profile_stream = profile_stream.with_compression()?;
        let profile_id = self.allocate_id();
        self.set_object(profile_id, Object::Stream(profile_stream));

        let mut oi = PdfDictionary::new();
        oi.set("Type", Object::Name(PdfName::new_unchecked("OutputIntent")));
        oi.set("S", Object::Name(PdfName::new_unchecked(subtype.as_name())));
        oi.set("OutputConditionIdentifier", Object::String(super::util::to_pdf_text_string(identifier)));
        oi.set("OutputCondition", Object::String(super::util::to_pdf_text_string(condition)));
        oi.set("DestOutputProfile", Object::Reference(profile_id));
        let oi_id = self.allocate_id();
        self.set_object(oi_id, Object::Dictionary(oi));

        let mut catalog = self.catalog()?;
        let mut intents = match catalog.get("OutputIntents") {
            Some(Object::Array(a)) => a.clone(),
            _ => PdfArray::new(),
        };
        intents.push(Object::Reference(oi_id));
        catalog.set("OutputIntents", Object::Array(intents));
        let cat_id = self.catalog_id();
        self.set_object(cat_id, Object::Dictionary(catalog));

        Ok(oi_id)
    }

    /// Reads back every entry of the catalog's `/OutputIntents` array.
    pub fn output_intents(&self) -> PdfResult<Vec<OutputIntentInfo>> {
        let catalog = self.catalog()?;
        let Some(Object::Array(intents)) = catalog.get("OutputIntents") else { return Ok(Vec::new()) };

        let mut out = Vec::new();
        for entry in intents.iter() {
            let Object::Reference(id) = entry else { continue };
            let Ok(dict) = self.get_dictionary(*id) else { continue };
            let subtype = match dict.get("S") {
                Some(Object::Name(n)) => n.as_str().to_string(),
                _ => String::new(),
            };
            let identifier = match dict.get("OutputConditionIdentifier") {
                Some(Object::String(s)) => Some(super::util::from_pdf_text_string(s)),
                _ => None,
            };
            let (has_valid_icc_profile, color_space) = match dict.get("DestOutputProfile") {
                Some(Object::Reference(profile_id)) => match self.get_object(*profile_id) {
                    Some(Object::Stream(s)) => match s.decode_all() {
                        Ok(decoded) => match validate_icc_header(&decoded) {
                            Ok(cs) => (true, Some(cs)),
                            Err(_) => (false, None),
                        },
                        Err(_) => (false, None),
                    },
                    _ => (false, None),
                },
                _ => (false, None),
            };
            out.push(OutputIntentInfo { subtype, identifier, has_valid_icc_profile, color_space });
        }
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! A minimal, hand-built *syntactically valid* ICC.1 header used only
    //! by this crate's own unit/integration tests (never shipped in the
    //! library itself) - see [`super`]'s module docs for why real ICC
    //! profile bytes always come from the caller rather than this crate.
    //! Only the header fields this module actually reads are filled in;
    //! everything else (tag table, tag data) is zeroed, which is enough
    //! to exercise the embedding/validation mechanics but does **not**
    //! produce a profile any real colour management engine could use.

    use super::IccColorSpace;

    pub(crate) fn fake_icc_profile(space: IccColorSpace) -> Vec<u8> {
        let mut data = vec![0u8; 200]; // header + a little slack
        let size = (data.len() as u32).to_be_bytes();
        data[0..4].copy_from_slice(&size);
        data[12..16].copy_from_slice(b"mntr"); // device class: display
        let sig: &[u8; 4] = match space {
            IccColorSpace::Gray => b"GRAY",
            IccColorSpace::Rgb => b"RGB ",
            IccColorSpace::Cmyk => b"CMYK",
        };
        data[16..20].copy_from_slice(sig);
        data[20..24].copy_from_slice(b"XYZ "); // PCS
        data[36..40].copy_from_slice(b"acsp");
        data
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::fake_icc_profile;
    use super::*;
    use crate::prelude::*;

    fn doc_with_one_page() -> EditableDocument {
        let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(ContentBuilder::new()).build();
        let bytes = DocumentBuilder::new().page(page).build().unwrap().save_to_bytes().unwrap();
        EditableDocument::from_bytes(bytes).unwrap()
    }

    #[test]
    fn test_validate_icc_header_accepts_well_formed_rgb_profile() {
        let profile = fake_icc_profile(IccColorSpace::Rgb);
        assert_eq!(validate_icc_header(&profile), Ok(IccColorSpace::Rgb));
    }

    #[test]
    fn test_validate_icc_header_rejects_too_short() {
        assert_eq!(validate_icc_header(&[0u8; 10]), Err(IccError::TooShort));
    }

    #[test]
    fn test_validate_icc_header_rejects_missing_signature() {
        let mut profile = fake_icc_profile(IccColorSpace::Cmyk);
        profile[36..40].copy_from_slice(b"XXXX");
        assert_eq!(validate_icc_header(&profile), Err(IccError::BadSignature));
    }

    #[test]
    fn test_validate_icc_header_rejects_declared_size_larger_than_data() {
        let mut profile = fake_icc_profile(IccColorSpace::Gray);
        profile[0..4].copy_from_slice(&(u32::MAX).to_be_bytes());
        assert_eq!(validate_icc_header(&profile), Err(IccError::DeclaredSizeExceedsData));
    }

    #[test]
    fn test_validate_icc_header_rejects_unknown_color_space() {
        let mut profile = fake_icc_profile(IccColorSpace::Rgb);
        profile[16..20].copy_from_slice(b"Lab ");
        assert_eq!(validate_icc_header(&profile), Err(IccError::UnsupportedColorSpace));
    }

    #[test]
    fn test_add_output_intent_round_trips() {
        let mut doc = doc_with_one_page();
        let profile = fake_icc_profile(IccColorSpace::Rgb);
        doc.add_output_intent(&profile, OutputIntentSubtype::PdfA, "sRGB IEC61966-2.1", "sRGB").unwrap();

        let intents = doc.output_intents().unwrap();
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].subtype, "GTS_PDFA1");
        assert_eq!(intents[0].identifier.as_deref(), Some("sRGB IEC61966-2.1"));
        assert!(intents[0].has_valid_icc_profile);
        assert_eq!(intents[0].color_space, Some(IccColorSpace::Rgb));
    }

    #[test]
    fn test_add_output_intent_rejects_garbage_profile() {
        let mut doc = doc_with_one_page();
        let result = doc.add_output_intent(b"not an icc profile", OutputIntentSubtype::PdfA, "x", "y");
        assert!(result.is_err());
    }

    #[test]
    fn test_output_intent_survives_full_rewrite() {
        let mut doc = doc_with_one_page();
        let profile = fake_icc_profile(IccColorSpace::Cmyk);
        doc.add_output_intent(&profile, OutputIntentSubtype::PdfX, "FOGRA39", "Coated").unwrap();
        let saved = doc.save_full_rewrite_to_bytes().unwrap();
        let reopened = EditableDocument::from_bytes(saved).unwrap();
        let intents = reopened.output_intents().unwrap();
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].subtype, "GTS_PDFX");
        assert_eq!(intents[0].color_space, Some(IccColorSpace::Cmyk));
    }

    #[test]
    fn test_no_output_intents_returns_empty() {
        let doc = doc_with_one_page();
        assert!(doc.output_intents().unwrap().is_empty());
    }
}
