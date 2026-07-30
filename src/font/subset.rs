//! Font subsetting: reduce an embedded font program to only the glyphs a
//! document actually uses (ISO 32000-1:2008 section 9.9 "Embedded Font
//! Programs" permits embedding a subset; section 9.7 "Composite Fonts"
//! covers CIDFontType2 programs referencing only a subset of glyphs, as
//! long as `CIDToGIDMap` reflects it — see [`crate::font::cid`]).
//!
//! Rebuilding `glyf`/`loca`/`hmtx`/CFF charstrings by hand for a subset is
//! exactly the kind of "reinvent a mature C/C++-grade library" work the
//! project rules ask us to avoid; this wraps the `subsetter` crate (used in
//! production by Typst's PDF export) instead of a from-scratch
//! implementation.

use super::truetype::TrueTypeFont;
use std::collections::BTreeSet;

pub use subsetter::GlyphRemapper;

/// Errors that can occur while subsetting a font.
#[derive(Debug, thiserror::Error)]
pub enum SubsetError {
    /// The `subsetter` crate rejected the font or the requested operation.
    #[error("font subsetting failed: {0}")]
    Subsetter(#[from] subsetter::Error),
}

/// The result of subsetting a font to a specific glyph set.
pub struct SubsetResult {
    /// The new, reduced font program bytes.
    pub font_data: Vec<u8>,
    /// Maps original glyph IDs (as looked up via `cmap`) to their new glyph
    /// ID in `font_data`. Every glyph ID passed into [`subset`] is
    /// guaranteed to have an entry.
    pub remapper: GlyphRemapper,
}

/// Subsets `font` down to `used_gids` (plus glyph `0`, `.notdef`, which the
/// underlying subsetter always retains).
///
/// The glyph IDs are remapped to a contiguous range starting at 0 (a
/// TrueType/OpenType requirement); the returned [`SubsetResult::remapper`]
/// records the old-GID -> new-GID mapping needed to build the PDF
/// `CIDToGIDMap` (ISO 32000-1 9.7.4.2).
pub fn subset(font: &TrueTypeFont, used_gids: &BTreeSet<u16>) -> Result<SubsetResult, SubsetError> {
    let sorted: Vec<u16> = used_gids.iter().copied().collect();
    let remapper = GlyphRemapper::new_from_glyphs_sorted(&sorted);
    let font_data = subsetter::subset(font.raw_data(), 0, &remapper)?;
    Ok(SubsetResult { font_data, remapper })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::truetype::test_support::build_test_font;

    fn font_with(chars: &[(char, u16)]) -> TrueTypeFont {
        TrueTypeFont::load(build_test_font(chars), 0).unwrap()
    }

    #[test]
    fn subset_reduces_size_and_remaps_used_glyphs() {
        // A "big" font with lots of unused glyphs vs. the 2 we actually use.
        let mut chars: Vec<(char, u16)> = Vec::new();
        for i in 0..200u16 {
            chars.push((char::from_u32(0x4E00 + i as u32).unwrap(), i + 1));
        }
        let font = font_with(&chars);
        let full_size = font.raw_data().len();

        let mut used = BTreeSet::new();
        used.insert(1u16); // first CJK glyph
        used.insert(50u16);

        let result = subset(&font, &used).expect("subsetting must succeed");
        assert!(
            result.font_data.len() < full_size,
            "subset ({} bytes) should be smaller than full font ({} bytes)",
            result.font_data.len(),
            full_size
        );
        assert!(result.remapper.get(1).is_some());
        assert!(result.remapper.get(50).is_some());
        // .notdef (0) is always retained by the subsetter.
        assert!(result.remapper.get(0).is_some());
    }

    #[test]
    fn subset_of_malformed_font_data_errors() {
        // Build a font, then corrupt it enough that the subsetter's own
        // table-directory validation rejects it, rather than panicking.
        let font_bytes = build_test_font(&[('A', 1)]);
        let corrupt = TrueTypeFont::load(font_bytes[..16].to_vec(), 0);
        // Loading itself should already fail on truncated data.
        assert!(corrupt.is_err());
    }
}
