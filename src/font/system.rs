//! OS-level font *discovery* for non-embedded simple-font substitution
//! (feature `system-fonts`) - see this crate's `native-render`/
//! `system-fonts` feature doc comments in `Cargo.toml` for the licensing
//! rationale for why this reads a font already installed on the host
//! rather than bundling a substitute of this crate's own.
//!
//! `font-kit` is used ONLY to *locate* a matching font file already on
//! the host OS (Core Text on macOS, DirectWrite on Windows, fontconfig
//! on Linux); the bytes it returns are parsed and rasterized through
//! this crate's own [`crate::font::truetype::TrueTypeFont`]
//! (`ttf-parser`)-based pipeline exactly like an embedded `FontFile2`
//! would be - `font-kit` never touches glyph outlines or pixels itself.
//!
//! # Why this exists
//!
//! A non-embedded simple font referencing a well-known family by name
//! (`/BaseFont /Arial-BoldMT` with no `FontFile2` in its
//! `/FontDescriptor`, or even no `/FontDescriptor` at all) is one of the
//! most common patterns in real-world business PDFs - Word/Excel/
//! PowerPoint exporters routinely skip embedding "Arial"/"Times New
//! Roman"/"Courier New" to save file size, since every mainstream reader
//! is expected to substitute the OS's own copy. Before this module
//! existed, this crate's native renderer had no substitution mechanism
//! at all (an explicit, documented gap) and such text rendered nothing.

#[cfg(feature = "system-fonts")]
use font_kit::family_name::FamilyName;
#[cfg(feature = "system-fonts")]
use font_kit::properties::{Properties, Style, Weight};
#[cfg(feature = "system-fonts")]
use font_kit::source::SystemSource;

/// Parses a PDF `/BaseFont` name into a bare family name plus bold/italic
/// flags, stripping the naming conventions real-world PDF producers
/// (and this crate's own writer) actually emit:
///
/// - A 6-uppercase-letter subset tag (`"ABCDEF+Arial-BoldMT"`, ISO
///   32000-1 9.6.4.3).
/// - ISO 32000-1 9.6.6.2's own `"BaseName,Style"` convention (e.g.
///   `"Arial,BoldItalic"`).
/// - Hyphenated/concatenated style words a font's own PostScript name
///   commonly carries (`"Arial-BoldMT"`, `"TimesNewRoman-Italic"`,
///   `"Verdana-Bold"`).
/// - Trailing `MT`/`PSMT`/`PS` naming artifacts common in Microsoft's own
///   font PostScript names (`"ArialMT"`, `"TimesNewRomanPSMT"`).
///
/// Not exhaustive of every font-naming convention in the wild - a family
/// this can't fully untangle just falls through to
/// [`load_system_font_bytes`]'s own `FamilyName::SansSerif` fallback
/// rather than failing outright.
pub(crate) fn parse_base_font_name(base_font: &str) -> (String, bool, bool) {
    let name = match base_font.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.chars().all(|c| c.is_ascii_uppercase()) => rest,
        _ => base_font,
    };

    let mut bold = false;
    let mut italic = false;
    let mut family = name.to_string();

    if let Some((base, style)) = family.clone().split_once(',') {
        let style_lower = style.to_ascii_lowercase();
        bold |= style_lower.contains("bold");
        italic |= style_lower.contains("italic") || style_lower.contains("oblique");
        family = base.to_string();
    }

    for (needle, is_bold, is_italic) in [
        ("bolditalic", true, true),
        ("boldoblique", true, true),
        ("bold", true, false),
        ("italic", false, true),
        ("oblique", false, true),
    ] {
        let lower = family.to_ascii_lowercase();
        let Some(pos) = lower.find(needle) else {
            continue;
        };
        bold |= is_bold;
        italic |= is_italic;

        let mut end = pos + needle.len();
        for trailing in ["psmt", "mt", "ps"] {
            if lower[end..].starts_with(trailing) {
                end += trailing.len();
                break;
            }
        }
        let mut start = pos;
        if start > 0 && matches!(family.as_bytes()[start - 1], b'-' | b',' | b'_') {
            start -= 1;
        }
        family = format!("{}{}", &family[..start], &family[end..]);
        break;
    }

    for trailing in ["PSMT", "MT", "PS"] {
        if let Some(stripped) = family.strip_suffix(trailing) {
            family = stripped.to_string();
            break;
        }
    }

    (family.trim_end_matches(['-', ',']).to_string(), bold, italic)
}

/// Looks up `family` (with `bold`/`italic` as parsed by
/// [`parse_base_font_name`]) among fonts already installed on the host OS
/// and returns its raw font program bytes, or `None` if no reasonable
/// match exists (a minimal install with no matching family, an
/// unsupported/headless OS backend, `font-kit` itself failing to load a
/// file it found, ...). Callers treat `None` exactly like "not embedded"
/// was already treated before this feature existed - fail closed, no
/// glyphs painted, a [`super::super::render::native::RenderWarning`]
/// recorded, never a panic.
#[cfg(feature = "system-fonts")]
pub(crate) fn load_system_font_bytes(family: &str, bold: bool, italic: bool) -> Option<Vec<u8>> {
    let mut properties = Properties::new();
    properties.style(if italic { Style::Italic } else { Style::Normal });
    properties.weight(if bold { Weight::BOLD } else { Weight::NORMAL });

    let families = [FamilyName::Title(family.to_string()), FamilyName::SansSerif];
    // Any `SelectionError` (no match found, or the host's font source
    // being unreachable e.g. in a headless/sandboxed environment) is
    // treated identically: fall back to "no substitute", not a panic or
    // a hard error.
    let handle = SystemSource::new().select_best_match(&families, &properties).ok()?;
    let font = handle.load().ok()?;
    let data = font.copy_font_data()?;
    Some((*data).clone())
}

#[cfg(not(feature = "system-fonts"))]
pub(crate) fn load_system_font_bytes(_family: &str, _bold: bool, _italic: bool) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hyphenated_postscript_names() {
        assert_eq!(parse_base_font_name("Arial-BoldMT"), ("Arial".to_string(), true, false));
        assert_eq!(parse_base_font_name("ArialMT"), ("Arial".to_string(), false, false));
        assert_eq!(parse_base_font_name("Arial-ItalicMT"), ("Arial".to_string(), false, true));
        assert_eq!(
            parse_base_font_name("Arial-BoldItalicMT"),
            ("Arial".to_string(), true, true)
        );
    }

    #[test]
    fn parses_subset_tag_prefix() {
        assert_eq!(parse_base_font_name("ABCDEF+Arial-BoldMT"), ("Arial".to_string(), true, false));
        // A `+` that isn't a valid 6-upper-letter subset tag is part of
        // the name, not stripped.
        assert_eq!(parse_base_font_name("A+Arial"), ("A+Arial".to_string(), false, false));
    }

    #[test]
    fn parses_comma_style_convention() {
        assert_eq!(parse_base_font_name("Arial,Bold"), ("Arial".to_string(), true, false));
        assert_eq!(parse_base_font_name("Arial,BoldItalic"), ("Arial".to_string(), true, true));
        assert_eq!(parse_base_font_name("TimesNewRoman,Italic"), ("TimesNewRoman".to_string(), false, true));
    }

    #[test]
    fn parses_times_new_roman_ps_suffix() {
        assert_eq!(
            parse_base_font_name("TimesNewRomanPSMT"),
            ("TimesNewRoman".to_string(), false, false)
        );
        assert_eq!(
            parse_base_font_name("TimesNewRomanPS-BoldMT"),
            ("TimesNewRoman".to_string(), true, false)
        );
    }

    #[test]
    fn leaves_plain_family_names_untouched() {
        assert_eq!(parse_base_font_name("Verdana"), ("Verdana".to_string(), false, false));
        assert_eq!(parse_base_font_name("Courier New"), ("Courier New".to_string(), false, false));
    }

    // `load_system_font_bytes` itself is deliberately NOT unit-tested here
    // with hard assertions on a specific family being found: which
    // families are actually installed is host-machine-dependent (CI
    // containers commonly have zero fonts). See
    // `render::native::font::tests` for a live, environment-tolerant
    // integration test that skips gracefully when nothing is found.
}
