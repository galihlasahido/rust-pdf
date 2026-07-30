//! Embedded TrueType/OpenType font loading.
//!
//! References:
//! - ISO 32000-1:2008 section 9.6 "Simple Fonts" (TrueType as a simple
//!   font), section 9.7 "Composite Fonts" (TrueType/OpenType wrapped as a
//!   Type 0/CIDFontType2 font, see [`crate::font::cid`]), section 9.8 "Font
//!   Descriptors", and section 9.9 "Embedded Font Programs" (`FontFile2`
//!   for TrueType outlines, `FontFile3` with `Subtype /OpenType` for
//!   OpenType-wrapped CFF outlines). Exact sub-clause/table numbers cited
//!   elsewhere in this module are to the best of the author's recollection
//!   of the spec structure and should be checked against the actual ISO
//!   32000-1 text rather than treated as authoritative on their own.
//! - Microsoft/Apple OpenType specification for the sfnt table layout being
//!   parsed (<https://learn.microsoft.com/en-us/typography/opentype/spec/>).
//!
//! This module deliberately does **not** implement its own sfnt/OpenType
//! parser: all font-file parsing goes through the `ttf-parser` crate, which
//! is battle-tested against the very large corpus of malformed real-world
//! fonts that a hand-rolled parser would be unlikely to handle safely. This
//! module only *extracts* the small set of facts the PDF writer needs
//! (glyph-to-Unicode mapping, advance widths, `FontDescriptor` metrics) and
//! stores them in an owned, `'static`-lifetime-friendly form.

use std::fmt;

/// Hard cap on the size of a font program accepted for loading.
///
/// This guards against unbounded memory allocation from a hostile input
/// (e.g. a font file supplied to an application that itself sourced it from
/// an untrusted upload); no legitimate desktop/CJK font comes close to this
/// size.
pub const MAX_FONT_SIZE_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

/// Hard cap on the number of glyphs a loaded font may declare.
///
/// Independent of the file-size cap: bounds any loop or allocation keyed on
/// `maxp.numGlyphs` (e.g. a future full glyph-table walk) against a
/// malformed/adversarial value in that field.
pub const MAX_GLYPH_COUNT: u32 = 200_000;

/// Errors that can occur while loading a TrueType/OpenType font program.
#[derive(Debug, thiserror::Error)]
pub enum FontLoadError {
    /// The font data exceeds [`MAX_FONT_SIZE_BYTES`].
    #[error("font data is {0} bytes, exceeding the {MAX_FONT_SIZE_BYTES} byte safety limit")]
    TooLarge(usize),
    /// The font declares more glyphs than [`MAX_GLYPH_COUNT`].
    #[error("font declares {0} glyphs, exceeding the safety limit of {MAX_GLYPH_COUNT}")]
    TooManyGlyphs(u32),
    /// `ttf-parser` rejected the font data as malformed.
    #[error("not a valid TrueType/OpenType font: {0}")]
    Malformed(#[from] ttf_parser::FaceParsingError),
    /// The font collection index requested does not exist.
    #[error("font collection does not contain face index {0}")]
    NoSuchFace(u32),
    /// `ttf-parser` panicked while parsing the font data instead of
    /// returning an error.
    ///
    /// `ttf-parser` is now unmaintained (RUSTSEC-2026-0192; see
    /// `docs/THREAT_MODEL.md`) and this crate's `font_load` fuzz target has
    /// found malformed inputs (a `ttcf` collection header with an
    /// out-of-range table offset) that trip an internal `debug_assert!`
    /// inside `ttf-parser` (`parser::Stream::read_bytes`) rather than
    /// failing gracefully. Since embedded font programs are fully
    /// attacker-controlled untrusted input, [`load`] isolates the parse
    /// call with `catch_unwind` and surfaces any panic as this variant
    /// instead of letting it unwind past this crate's API.
    ///
    /// **This is defense-in-depth only, not the primary fix**: `catch_unwind`
    /// only helps in ordinary (`panic = "unwind"`) binaries. `cargo fuzz`
    /// builds link `libfuzzer-sys`, whose panic hook calls
    /// `std::process::abort()` *inside the hook itself*, i.e. before
    /// unwinding ever starts — so under the `font_load` fuzz target this
    /// variant is never actually produced for the known `ttcf` crashers;
    /// the process aborts regardless of `catch_unwind`. The real fix for
    /// those inputs is [`load`] rejecting the malformed structure *before*
    /// ever calling into `ttf-parser` (see the `ttcf` header validation at
    /// the top of [`load`]); `catch_unwind` here only guards against some
    /// *other*, not-yet-fuzz-discovered panic inside `ttf-parser` in a
    /// normal (non-fuzz) host application.
    ///
    /// [`load`]: TrueTypeFont::load
    #[error("ttf-parser panicked while parsing font data (malformed/adversarial input)")]
    Panicked,
}

/// The outline flavor of a loaded font, which determines whether it must be
/// embedded via `FontFile2` (TrueType, ISO 32000-1 9.9 Table 126) or
/// `FontFile3`/`OpenType` (CFF-flavored OpenType).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontFlavor {
    /// `glyf`/`loca`-based outlines.
    TrueType,
    /// `CFF `-based outlines (as found in most OpenType fonts).
    Cff,
}

/// An owned, parsed TrueType/OpenType font program, ready to be embedded in
/// a PDF as a CID-keyed (Type 0) or simple TrueType font.
///
/// All facts needed by the PDF writer are extracted eagerly at [`load`]
/// time into owned fields, so this type does not borrow from (or keep
/// re-parsing) the underlying font bytes for metric queries; only glyph
/// lookups ([`glyph_id`], [`glyph_advance`]) re-run a (cheap, zero-alloc)
/// `ttf_parser::Face::parse` over the stored bytes.
///
/// [`load`]: TrueTypeFont::load
/// [`glyph_id`]: TrueTypeFont::glyph_id
/// [`glyph_advance`]: TrueTypeFont::glyph_advance
pub struct TrueTypeFont {
    data: Vec<u8>,
    face_index: u32,
    flavor: FontFlavor,
    units_per_em: u16,
    num_glyphs: u16,
    ascender: i16,
    descender: i16,
    cap_height: i16,
    italic_angle: f32,
    is_bold: bool,
    is_italic: bool,
    is_fixed_pitch: bool,
    is_serif: bool,
    bbox: (i16, i16, i16, i16),
    postscript_name: Option<String>,
}

impl fmt::Debug for TrueTypeFont {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrueTypeFont")
            .field("postscript_name", &self.postscript_name)
            .field("flavor", &self.flavor)
            .field("units_per_em", &self.units_per_em)
            .field("num_glyphs", &self.num_glyphs)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// Length in bytes of a `ttcf` (TrueType/OpenType Collection) header up to
/// and including `numFonts`: `TTCTag` (4) + `majorVersion`+`minorVersion`
/// (4, read together as a single skipped `u32` by `ttf-parser`) +
/// `numFonts` (4). See the OpenType spec, "Font Collections"
/// (<https://learn.microsoft.com/en-us/typography/opentype/spec/otff#font-collections>).
const TTCF_HEADER_LEN: usize = 12;

/// Size in bytes of a single `Offset32` entry in a `ttcf` header's
/// `offsetTable` (one per face in the collection).
const TTCF_OFFSET_ENTRY_SIZE: u64 = 4;

/// Rejects `ttcf` (font collection) headers that would make `ttf-parser`
/// 0.25.1's `Face::parse` trip an internal `debug_assert!` instead of
/// returning an `Err`.
///
/// `ttf-parser`'s `RawFace::parse` reads a `ttcf` header's `numFonts: u32`
/// field straight from the file and immediately requests
/// `numFonts * size_of::<Offset32>()` bytes from its internal `Stream`
/// (`Stream::read_array32::<Offset32>`) *before* checking that count
/// against the amount of data actually remaining. `Stream::read_bytes`
/// itself then hits `debug_assert!(self.offset as u64 + len as u64 <=
/// u32::MAX as u64)` — a sanity check the crate's own doc comment
/// characterizes as guarding against "an incorrect parsing logic from the
/// caller side" — before it ever reaches the real (non-debug) bounds
/// check a few lines later. A small file with a garbage-but-huge
/// `numFonts` (e.g. `0xFF000000`) makes `offset + len` overflow
/// `u32::MAX` and trips that assertion.
///
/// In an ordinary binary this only aborts when compiled with debug
/// assertions on (`cargo test`/dev builds), where `catch_unwind` in
/// [`TrueTypeFont::load`] already turns it into `Err`. But `cargo fuzz`
/// binaries link `libfuzzer-sys`, whose panic hook calls
/// `std::process::abort()` from *within the hook*, before any unwinding
/// (and therefore before `catch_unwind`'s landing pad) ever runs — so
/// relying on `catch_unwind` alone does not actually protect the
/// `font_load` fuzz target, and three crash artifacts
/// (`fuzz/artifacts/font_load/crash-*`, now covered by
/// `tests::rejects_the_three_font_load_fuzz_crashers` below)
/// demonstrated exactly that. This function re-derives the same
/// overflow condition ourselves, as a plain comparison rather than an
/// assertion, and rejects the input *before* it ever reaches
/// `ttf_parser::Face::parse` — so the debug assertion can never fire, no
/// matter how the caller's binary was compiled.
///
/// Returns `Ok(())` for anything that isn't a (potentially malformed)
/// `ttcf` header short enough to read `numFonts` from; such inputs are
/// left to `ttf-parser` to accept or reject in the ordinary way.
fn reject_ttcf_offset_table_overflow(data: &[u8]) -> Result<(), FontLoadError> {
    const TTCF_MAGIC: [u8; 4] = *b"ttcf";

    if data.len() < TTCF_HEADER_LEN || data[0..4] != TTCF_MAGIC {
        return Ok(());
    }

    let num_fonts = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    let offset_table_len = u64::from(num_fonts) * TTCF_OFFSET_ENTRY_SIZE;
    if (TTCF_HEADER_LEN as u64) + offset_table_len > u64::from(u32::MAX) {
        return Err(FontLoadError::Malformed(
            ttf_parser::FaceParsingError::MalformedFont,
        ));
    }

    Ok(())
}

impl TrueTypeFont {
    /// Loads and validates a TrueType/OpenType font program from raw bytes.
    ///
    /// `face_index` selects a face within a font collection (`.ttc`/`.otc`);
    /// pass `0` for a regular single-face font file.
    ///
    /// # Errors
    /// Returns an error if `data` exceeds the configured safety limits, or
    /// if `ttf-parser` cannot parse it as a valid font.
    pub fn load(data: Vec<u8>, face_index: u32) -> Result<Self, FontLoadError> {
        if data.len() > MAX_FONT_SIZE_BYTES {
            return Err(FontLoadError::TooLarge(data.len()));
        }

        // Must run before ever calling into `ttf-parser`: see this
        // function's doc comment for why `catch_unwind` below is not
        // sufficient on its own to stop this specific input shape from
        // aborting the process under a `cargo fuzz` binary.
        reject_ttcf_offset_table_overflow(&data)?;

        // `ttf-parser`'s public API is documented as panic-free, but per
        // `FontLoadError::Panicked`'s doc comment, fuzzing found a malformed
        // `ttcf` collection header that trips an internal `assert!` instead
        // of returning `Err`. `data`/`face_index` here are fully
        // attacker-controlled (an embedded `FontFile2`/`FontFile3` stream
        // from an untrusted PDF), so the parse is isolated with
        // `catch_unwind`: `AssertUnwindSafe` is sound here because `data`
        // and `face_index` are only read (never mutated) by the closure, so
        // there is no way for a panic mid-parse to leave them in an
        // observably inconsistent state that could violate an invariant
        // elsewhere.
        let parse_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ttf_parser::Face::parse(&data, face_index)
        }))
        .map_err(|_| FontLoadError::Panicked)?;

        let face = parse_result.map_err(|e| match e {
            ttf_parser::FaceParsingError::FaceIndexOutOfBounds => {
                FontLoadError::NoSuchFace(face_index)
            }
            other => FontLoadError::Malformed(other),
        })?;

        let num_glyphs = face.number_of_glyphs();
        if u32::from(num_glyphs) > MAX_GLYPH_COUNT {
            return Err(FontLoadError::TooManyGlyphs(u32::from(num_glyphs)));
        }

        let flavor = if face.tables().glyf.is_some() {
            FontFlavor::TrueType
        } else {
            FontFlavor::Cff
        };

        let bbox = {
            let r = face.global_bounding_box();
            (r.x_min, r.y_min, r.x_max, r.y_max)
        };

        let cap_height = face
            .capital_height()
            .filter(|&h| h != 0)
            .unwrap_or_else(|| face.ascender());

        let postscript_name = find_name(&face, ttf_parser::name_id::POST_SCRIPT_NAME)
            .or_else(|| find_name(&face, ttf_parser::name_id::FULL_NAME))
            .or_else(|| find_name(&face, ttf_parser::name_id::FAMILY));

        Ok(Self {
            face_index,
            flavor,
            units_per_em: face.units_per_em(),
            num_glyphs,
            ascender: face.ascender(),
            descender: face.descender(),
            cap_height,
            italic_angle: face.italic_angle(),
            is_bold: face.is_bold(),
            is_italic: face.is_italic(),
            is_fixed_pitch: face.is_monospaced(),
            is_serif: false, // ttf-parser has no direct serif flag; left to caller heuristics.
            bbox,
            postscript_name,
            data,
        })
    }

    /// Re-parses the stored font bytes into a borrowed `ttf_parser::Face`.
    ///
    /// This is intentionally cheap (table-directory lookups only, no glyph
    /// decoding) so callers may call it per glyph query without caching.
    ///
    /// `self.data`/`self.face_index` were already successfully parsed once
    /// in [`load`](Self::load), and `data` is never mutated afterwards, so
    /// in practice this cannot fail — but per this crate's rule against
    /// `unwrap`/`expect` on data derived from a file, callers still treat a
    /// `None` here as "no information available" rather than assuming it's
    /// unreachable, so a future change to this invariant fails safe
    /// instead of panicking.
    fn face(&self) -> Option<ttf_parser::Face<'_>> {
        ttf_parser::Face::parse(&self.data, self.face_index).ok()
    }

    /// Returns the raw, original (un-subset) font program bytes.
    pub fn raw_data(&self) -> &[u8] {
        &self.data
    }

    /// Returns the outline flavor (`glyf` vs `CFF`), which determines
    /// whether this font must be embedded as `FontFile2` or `FontFile3`.
    pub fn flavor(&self) -> FontFlavor {
        self.flavor
    }

    /// Returns the font's units-per-em (typically 1000 or 2048).
    pub fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    /// Returns the total number of glyphs in the font (including `.notdef`).
    pub fn num_glyphs(&self) -> u16 {
        self.num_glyphs
    }

    /// Returns the font's PostScript name, if present in the `name` table.
    pub fn postscript_name(&self) -> Option<&str> {
        self.postscript_name.as_deref()
    }

    /// Looks up the glyph ID for a Unicode scalar value via the font's
    /// `cmap` table — the glyph-selection mechanism ISO 32000-1 section
    /// 9.7 ("Composite Fonts") relies on for CIDFontType2 embedding.
    ///
    /// Returns `None` if the font has no glyph for `c` (the caller should
    /// fall back to glyph `0` (`.notdef`), which conventionally renders as
    /// a placeholder box).
    pub fn glyph_id(&self, c: char) -> Option<u16> {
        self.face()?.glyph_index(c).map(|g| g.0)
    }

    /// Returns the horizontal advance width of `gid`, scaled to 1000
    /// glyph-space units (the fixed unit PDF `W`/`Widths` arrays use,
    /// ISO 32000-1 9.7.4.3), regardless of the font's own `unitsPerEm`.
    pub fn glyph_advance(&self, gid: u16) -> u16 {
        let advance = self
            .face()
            .and_then(|f| f.glyph_hor_advance(ttf_parser::GlyphId(gid)))
            .unwrap_or(0);
        if self.units_per_em == 0 {
            return 0;
        }
        ((advance as u32 * 1000) / u32::from(self.units_per_em)) as u16
    }

    /// Returns `(ascender, descender, cap_height)` in 1000-unit glyph space,
    /// for use in the `FontDescriptor` dictionary (ISO 32000-1 Table 122).
    pub fn descriptor_metrics_1000(&self) -> (i32, i32, i32) {
        let scale = |v: i16| -> i32 {
            if self.units_per_em == 0 {
                0
            } else {
                (i32::from(v) * 1000) / i32::from(self.units_per_em)
            }
        };
        (scale(self.ascender), scale(self.descender), scale(self.cap_height))
    }

    /// Returns the font's global bounding box in 1000-unit glyph space, for
    /// `FontDescriptor`'s `/FontBBox` (ISO 32000-1 Table 122).
    pub fn font_bbox_1000(&self) -> [i32; 4] {
        let scale = |v: i16| -> i32 {
            if self.units_per_em == 0 {
                0
            } else {
                (i32::from(v) * 1000) / i32::from(self.units_per_em)
            }
        };
        let (x0, y0, x1, y1) = self.bbox;
        [scale(x0), scale(y0), scale(x1), scale(y1)]
    }

    /// The italic angle in degrees (0 for upright fonts), for
    /// `FontDescriptor`'s `/ItalicAngle`.
    pub fn italic_angle(&self) -> f32 {
        self.italic_angle
    }

    /// Whether the font's `OS/2`/`head` metadata marks it bold.
    pub fn is_bold(&self) -> bool {
        self.is_bold
    }

    /// Whether the font's `OS/2`/`head` metadata marks it italic/oblique.
    pub fn is_italic(&self) -> bool {
        self.is_italic
    }

    /// Whether the font is fixed-pitch (monospace).
    pub fn is_fixed_pitch(&self) -> bool {
        self.is_fixed_pitch
    }

    /// Heuristic serif flag (best-effort; `ttf-parser`/OpenType have no
    /// direct "is serif" bit outside the non-mandatory `PANOSE` bytes in
    /// `OS/2`, which this crate does not currently decode).
    pub fn is_serif(&self) -> bool {
        self.is_serif
    }

    /// Extracts glyph `gid`'s outline (ISO 32000-1:2008 9.6.5/9.7 glyph
    /// shapes; the underlying `glyf` or `CFF` table, whichever this font
    /// actually has -- see [`flavor`](Self::flavor)) into `builder`, in
    /// unscaled font design units (divide by [`units_per_em`](Self::units_per_em)
    /// to get text-space-relative glyph-space units, ISO 32000-1 9.2.4).
    ///
    /// Returns `None` if the font could not be re-parsed (see
    /// [`face`](Self::face) -- not expected in practice, since `data` was
    /// already successfully parsed once by [`load`](Self::load)), or if
    /// `gid` has no outline (e.g. `.notdef`, a whitespace glyph, a
    /// bitmap/color-only glyph this phase doesn't rasterize, or `gid`
    /// beyond [`num_glyphs`](Self::num_glyphs)) -- callers should treat
    /// `None` as "paint nothing for this glyph", not an error.
    ///
    /// # Untrusted input
    ///
    /// `gid`/the font bytes are attacker-controlled (an embedded
    /// `FontFile2`/`FontFile3` from an untrusted PDF). `ttf-parser`'s
    /// public API is documented as panic-free, but per
    /// [`FontLoadError::Panicked`]'s doc comment this crate has already
    /// found malformed inputs that trip an internal `debug_assert!`
    /// instead of returning `Err`/`None`; the same defense-in-depth
    /// `catch_unwind` wrapper used there is applied here too. As there,
    /// `AssertUnwindSafe` is sound because the closure only reads `self`/
    /// `gid` and writes to `builder` through the `OutlineBuilder` trait
    /// object -- a panic mid-outline cannot leave any of those in an
    /// observably-inconsistent state that could violate an invariant
    /// elsewhere (the caller simply discards whatever partial path
    /// `builder` accumulated, same as for a clean `None`).
    pub fn outline_glyph(
        &self,
        gid: u16,
        builder: &mut dyn ttf_parser::OutlineBuilder,
    ) -> Option<ttf_parser::Rect> {
        let face = self.face()?;
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            face.outline_glyph(ttf_parser::GlyphId(gid), builder)
        }))
        .unwrap_or(None)
    }
}

fn find_name(face: &ttf_parser::Face<'_>, name_id: u16) -> Option<String> {
    face.names()
        .into_iter()
        .find(|n| n.name_id == name_id && n.is_unicode())
        .and_then(|n| n.to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
pub(crate) mod test_support {
    //! A minimal, hand-built sfnt/TrueType font *generator* used only by
    //! this crate's own unit/integration tests (never shipped in the
    //! library itself). Building fixture fonts this way avoids vendoring
    //! large third-party font binaries (and their licensing overhead) just
    //! to exercise the embedding/subsetting pipeline. See
    //! <https://learn.microsoft.com/en-us/typography/opentype/spec/otff>
    //! for the table layouts being emitted.

    /// Builds a minimal valid TrueType font whose `cmap` maps exactly the
    /// given `(char, glyph_id)` pairs (plus the mandatory `.notdef` glyph
    /// 0). All glyphs are empty outlines (zero contours) — sufficient to
    /// exercise cmap lookup, advance widths, subsetting and CID/ToUnicode
    /// generation, though not to visually render a real glyph shape.
    pub(crate) fn build_test_font(chars: &[(char, u16)]) -> Vec<u8> {
        let num_glyphs: u16 = chars.iter().map(|&(_, g)| g).max().unwrap_or(0) + 1;

        let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"cmap", build_cmap(chars)),
            (b"glyf", Vec::new()), // all glyphs are empty
            (b"head", build_head(num_glyphs)),
            (b"hhea", build_hhea(num_glyphs)),
            (b"hmtx", build_hmtx(num_glyphs)),
            (b"loca", build_loca(num_glyphs)),
            (b"maxp", build_maxp(num_glyphs)),
            (b"name", build_name(b"RustPdfTestFont")),
            (b"post", build_post()),
        ];

        tables.sort_by_key(|(tag, _)| **tag);

        let mut out = Vec::new();
        let num_tables = tables.len() as u16;
        out.extend_from_slice(&0x00010000u32.to_be_bytes());
        out.extend_from_slice(&num_tables.to_be_bytes());
        let entry_selector = (num_tables as f32).log2().floor() as u16;
        let search_range = 2u16.saturating_pow(entry_selector as u32) * 16;
        let range_shift = num_tables.saturating_mul(16).saturating_sub(search_range);
        out.extend_from_slice(&search_range.to_be_bytes());
        out.extend_from_slice(&entry_selector.to_be_bytes());
        out.extend_from_slice(&range_shift.to_be_bytes());

        let dir_len = 12 + tables.len() * 16;
        let mut offset = dir_len;
        let mut records = Vec::new();
        let mut body = Vec::new();
        for (tag, data) in &tables {
            let start = offset;
            body.extend_from_slice(data);
            while body.len() % 4 != 0 {
                body.push(0);
            }
            offset = dir_len + body.len();
            records.push((*tag, start, data.len()));
        }

        for (tag, start, len) in &records {
            out.extend_from_slice(*tag);
            out.extend_from_slice(&0u32.to_be_bytes()); // checksum (unchecked by ttf-parser)
            out.extend_from_slice(&(*start as u32).to_be_bytes());
            out.extend_from_slice(&(*len as u32).to_be_bytes());
        }
        out.extend_from_slice(&body);
        out
    }

    fn build_head(num_glyphs: u16) -> Vec<u8> {
        let _ = num_glyphs;
        let mut v = Vec::new();
        v.extend_from_slice(&0x00010000u32.to_be_bytes()); // version
        v.extend_from_slice(&0u32.to_be_bytes()); // fontRevision
        v.extend_from_slice(&0u32.to_be_bytes()); // checkSumAdjustment
        v.extend_from_slice(&0x5F0F3CF5u32.to_be_bytes()); // magicNumber
        v.extend_from_slice(&0u16.to_be_bytes()); // flags
        v.extend_from_slice(&1000u16.to_be_bytes()); // unitsPerEm
        v.extend_from_slice(&0u64.to_be_bytes()); // created
        v.extend_from_slice(&0u64.to_be_bytes()); // modified
        v.extend_from_slice(&0i16.to_be_bytes()); // xMin
        v.extend_from_slice(&0i16.to_be_bytes()); // yMin
        v.extend_from_slice(&1000i16.to_be_bytes()); // xMax
        v.extend_from_slice(&1000i16.to_be_bytes()); // yMax
        v.extend_from_slice(&0u16.to_be_bytes()); // macStyle
        v.extend_from_slice(&8u16.to_be_bytes()); // lowestRecPPEM
        v.extend_from_slice(&2i16.to_be_bytes()); // fontDirectionHint
        v.extend_from_slice(&0i16.to_be_bytes()); // indexToLocFormat (short)
        v.extend_from_slice(&0i16.to_be_bytes()); // glyphDataFormat
        v
    }

    fn build_hhea(num_glyphs: u16) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&0x00010000u32.to_be_bytes());
        v.extend_from_slice(&800i16.to_be_bytes()); // ascender
        v.extend_from_slice(&(-200i16).to_be_bytes()); // descender
        v.extend_from_slice(&0i16.to_be_bytes()); // lineGap
        v.extend_from_slice(&1000u16.to_be_bytes()); // advanceWidthMax
        v.extend_from_slice(&0i16.to_be_bytes()); // minLeftSideBearing
        v.extend_from_slice(&0i16.to_be_bytes()); // minRightSideBearing
        v.extend_from_slice(&1000i16.to_be_bytes()); // xMaxExtent
        v.extend_from_slice(&1i16.to_be_bytes()); // caretSlopeRise
        v.extend_from_slice(&0i16.to_be_bytes()); // caretSlopeRun
        v.extend_from_slice(&0i16.to_be_bytes()); // caretOffset
        v.extend_from_slice(&[0u8; 8]); // reserved x4
        v.extend_from_slice(&0i16.to_be_bytes()); // metricDataFormat
        v.extend_from_slice(&num_glyphs.to_be_bytes()); // numberOfHMetrics
        v
    }

    fn build_hmtx(num_glyphs: u16) -> Vec<u8> {
        let mut v = Vec::new();
        for _ in 0..num_glyphs {
            v.extend_from_slice(&600u16.to_be_bytes()); // advanceWidth
            v.extend_from_slice(&0i16.to_be_bytes()); // lsb
        }
        v
    }

    fn build_loca(num_glyphs: u16) -> Vec<u8> {
        // Short format: all glyphs are zero-length, so every offset is 0.
        let mut v = Vec::new();
        for _ in 0..=num_glyphs {
            v.extend_from_slice(&0u16.to_be_bytes());
        }
        v
    }

    fn build_maxp(num_glyphs: u16) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&0x00010000u32.to_be_bytes()); // version 1.0
        v.extend_from_slice(&num_glyphs.to_be_bytes());
        v.extend_from_slice(&[0u8; 26]); // remaining v1.0 fields, all zero
        v
    }

    fn build_post() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&0x00030000u32.to_be_bytes()); // version 3.0 (no names)
        v.extend_from_slice(&0i32.to_be_bytes()); // italicAngle
        v.extend_from_slice(&0i16.to_be_bytes()); // underlinePosition
        v.extend_from_slice(&0i16.to_be_bytes()); // underlineThickness
        v.extend_from_slice(&0u32.to_be_bytes()); // isFixedPitch
        v.extend_from_slice(&[0u8; 16]); // min/max MemType42/Type1
        v
    }

    fn build_name(postscript_name: &[u8]) -> Vec<u8> {
        let utf16: Vec<u8> = postscript_name
            .iter()
            .flat_map(|&b| (b as u16).to_be_bytes())
            .collect();
        let mut v = Vec::new();
        v.extend_from_slice(&0u16.to_be_bytes()); // format
        v.extend_from_slice(&1u16.to_be_bytes()); // count
        v.extend_from_slice(&(6 + 12u16).to_be_bytes()); // storageOffset
        // One NameRecord: Windows, Unicode BMP, en-US, PostScript name.
        v.extend_from_slice(&3u16.to_be_bytes()); // platformID
        v.extend_from_slice(&1u16.to_be_bytes()); // encodingID
        v.extend_from_slice(&0x0409u16.to_be_bytes()); // languageID
        v.extend_from_slice(&6u16.to_be_bytes()); // nameID (PostScript name)
        v.extend_from_slice(&(utf16.len() as u16).to_be_bytes()); // length
        v.extend_from_slice(&0u16.to_be_bytes()); // offset into storage
        v.extend_from_slice(&utf16);
        v
    }

    fn build_cmap(chars: &[(char, u16)]) -> Vec<u8> {
        let mut segs: Vec<(u16, u16)> = chars
            .iter()
            .map(|&(c, g)| (c as u16, g))
            .collect();
        segs.sort_by_key(|&(code, _)| code);

        let seg_count = segs.len() as u16 + 1; // + terminator
        let mut sub = Vec::new();
        sub.extend_from_slice(&4u16.to_be_bytes()); // format
        sub.extend_from_slice(&0u16.to_be_bytes()); // length (patched below)
        sub.extend_from_slice(&0u16.to_be_bytes()); // language
        let seg_count_x2 = seg_count * 2;
        sub.extend_from_slice(&seg_count_x2.to_be_bytes());
        let entry_selector = (seg_count as f32).log2().floor() as u16;
        let search_range = 2u16.saturating_pow(entry_selector as u32) * 2;
        sub.extend_from_slice(&search_range.to_be_bytes());
        sub.extend_from_slice(&entry_selector.to_be_bytes());
        sub.extend_from_slice(&(seg_count_x2.saturating_sub(search_range)).to_be_bytes());

        for &(code, _) in &segs {
            sub.extend_from_slice(&code.to_be_bytes()); // endCode == startCode
        }
        sub.extend_from_slice(&0xFFFFu16.to_be_bytes()); // terminator endCode
        sub.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
        for &(code, _) in &segs {
            sub.extend_from_slice(&code.to_be_bytes()); // startCode
        }
        sub.extend_from_slice(&0xFFFFu16.to_be_bytes()); // terminator startCode
        for &(code, gid) in &segs {
            let delta = gid.wrapping_sub(code);
            sub.extend_from_slice(&delta.to_be_bytes());
        }
        sub.extend_from_slice(&1u16.to_be_bytes()); // terminator idDelta
        for _ in 0..seg_count {
            sub.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset
        }

        let len = sub.len() as u16;
        sub[2..4].copy_from_slice(&len.to_be_bytes());

        let mut v = Vec::new();
        v.extend_from_slice(&0u16.to_be_bytes()); // version
        v.extend_from_slice(&1u16.to_be_bytes()); // numTables
        v.extend_from_slice(&3u16.to_be_bytes()); // platformID (Windows)
        v.extend_from_slice(&1u16.to_be_bytes()); // encodingID (Unicode BMP)
        v.extend_from_slice(&12u32.to_be_bytes()); // offset
        v.extend_from_slice(&sub);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::build_test_font;
    use super::*;

    fn sample_font() -> Vec<u8> {
        build_test_font(&[('A', 1), ('B', 2), ('中', 3), ('日', 4)])
    }

    #[test]
    fn loads_valid_font() {
        let font = TrueTypeFont::load(sample_font(), 0).expect("valid test font must load");
        assert_eq!(font.units_per_em(), 1000);
        assert_eq!(font.num_glyphs(), 5); // .notdef + 4
        assert_eq!(font.flavor(), FontFlavor::TrueType);
    }

    #[test]
    fn glyph_lookup_ascii_and_cjk() {
        let font = TrueTypeFont::load(sample_font(), 0).unwrap();
        assert_eq!(font.glyph_id('A'), Some(1));
        assert_eq!(font.glyph_id('B'), Some(2));
        assert_eq!(font.glyph_id('中'), Some(3));
        assert_eq!(font.glyph_id('日'), Some(4));
        assert_eq!(font.glyph_id('Z'), None); // not in cmap
    }

    #[test]
    fn glyph_advance_is_scaled_to_1000_units() {
        let font = TrueTypeFont::load(sample_font(), 0).unwrap();
        // unitsPerEm is 1000 in the fixture, so advance passes through
        // unchanged (600 font units -> 600 in glyph space).
        assert_eq!(font.glyph_advance(1), 600);
    }

    #[test]
    fn postscript_name_extracted() {
        let font = TrueTypeFont::load(sample_font(), 0).unwrap();
        assert_eq!(font.postscript_name(), Some("RustPdfTestFont"));
    }

    #[test]
    fn rejects_oversized_font() {
        let oversized = vec![0u8; MAX_FONT_SIZE_BYTES + 1];
        let err = TrueTypeFont::load(oversized, 0).unwrap_err();
        assert!(matches!(err, FontLoadError::TooLarge(_)));
    }

    #[test]
    fn rejects_garbage_data() {
        let err = TrueTypeFont::load(vec![0u8; 32], 0).unwrap_err();
        assert!(matches!(err, FontLoadError::Malformed(_)));
    }

    #[test]
    fn rejects_truncated_font() {
        let mut data = sample_font();
        data.truncate(20); // chop off most of the table directory/data
        let err = TrueTypeFont::load(data, 0).unwrap_err();
        assert!(matches!(err, FontLoadError::Malformed(_)));
    }

    /// Regression tests for the three crashes the `font_load` cargo-fuzz
    /// target found (previously committed as
    /// `fuzz/artifacts/font_load/crash-*`, now removed since they are
    /// reproduced here): each is a malformed `ttcf` (TrueType Collection)
    /// header whose garbage `numFonts` field, multiplied by the `Offset32`
    /// entry size, overflows `u32::MAX` and trips an internal
    /// `debug_assert!` inside `ttf-parser` 0.25.1
    /// (`parser::Stream::read_bytes`, `self.offset as u64 + len as u64 <=
    /// u32::MAX as u64`) instead of `ttf-parser` returning `Err`.
    ///
    /// Note this is a *stronger* guarantee than "doesn't unwind past
    /// `load`": `TrueTypeFont::load`'s `catch_unwind` wrapper around the
    /// `ttf_parser::Face::parse` call does **not** actually stop these
    /// specific inputs from aborting the process when the caller is a
    /// `cargo fuzz`/`libfuzzer-sys` binary, because that harness's panic
    /// hook calls `std::process::abort()` from inside the hook itself,
    /// before any unwinding (and therefore before `catch_unwind`'s landing
    /// pad) ever runs -- confirmed by literally reproducing all three
    /// crashes via `cargo +nightly fuzz run font_load
    /// fuzz/artifacts/font_load/crash-*` before this fix. The actual fix is
    /// `reject_ttcf_offset_table_overflow` rejecting the malformed
    /// structure *before* `load` ever calls into `ttf-parser`, so these
    /// assertions below check specifically for `FontLoadError::Malformed`
    /// (the proactive-validation path), not `FontLoadError::Panicked` (the
    /// `catch_unwind` fallback path), to prove the real fix -- not just the
    /// unwind-safety net -- is what's rejecting these inputs.
    #[test]
    fn rejects_the_three_font_load_fuzz_crashers() {
        // fuzz/artifacts/font_load/crash-30b0cd3a693fc48b90c680dbb6857cfd09505a6a
        let crash_1: &[u8] = &[
            0x74, 0x74, 0x63, 0x66, 0xe3, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x04, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0x60,
        ];
        // fuzz/artifacts/font_load/crash-4a98e9e6b19576b075447f7db3230247c43c2fd1
        let crash_2: &[u8] = &[
            0x74, 0x74, 0x63, 0x66, 0xff, 0xff, 0xff, 0xfd, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0x00, 0x00, 0x00, 0x00, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b,
            0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b,
            0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b,
            0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b,
            0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x01, 0x00, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b,
            0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x74,
            0x74, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x63, 0x6b, 0x6b,
        ];
        // fuzz/artifacts/font_load/crash-8bd76a58989e17683c849737aef872fa2038bdfd
        let crash_3: &[u8] = &[
            0x74, 0x74, 0x63, 0x66, 0xff, 0xff, 0x3f, 0xff, 0xff, 0x00, 0x00, 0x00, 0x04, 0xff,
            0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b,
            0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b,
            0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x74, 0x63, 0x66, 0xff, 0xff, 0xff,
            0xff, 0x00, 0xff, 0xff, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x6b, 0x6b,
            0x6b, 0x6b, 0x67, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b,
            0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x6b, 0x63, 0x6b, 0x6b,
        ];

        for crash in [crash_1, crash_2, crash_3] {
            let err = TrueTypeFont::load(crash.to_vec(), 0).unwrap_err();
            assert!(
                matches!(err, FontLoadError::Malformed(_)),
                "expected proactive ttcf validation to reject {crash:x?} with \
                 FontLoadError::Malformed, got {err:?}"
            );
            // Face index 1 (the second face index the fuzz harness also
            // exercises) must be rejected identically.
            let err = TrueTypeFont::load(crash.to_vec(), 1).unwrap_err();
            assert!(matches!(err, FontLoadError::Malformed(_)));
        }
    }

    #[test]
    fn ttcf_offset_table_within_bounds_is_not_rejected_by_prevalidation() {
        // A `ttcf` header with a small, plausible `numFonts` must not be
        // rejected by `reject_ttcf_offset_table_overflow` itself (it should
        // still fail later, in `ttf-parser`, for being otherwise
        // malformed/truncated -- just not via the overflow guard).
        let mut data = vec![0u8; TTCF_HEADER_LEN];
        data[0..4].copy_from_slice(b"ttcf");
        data[8..12].copy_from_slice(&1u32.to_be_bytes()); // numFonts = 1
        assert!(reject_ttcf_offset_table_overflow(&data).is_ok());
    }

    #[test]
    fn font_bbox_and_descriptor_metrics_present() {
        let font = TrueTypeFont::load(sample_font(), 0).unwrap();
        let (ascender, descender, cap_height) = font.descriptor_metrics_1000();
        assert!(ascender > 0);
        assert!(descender < 0);
        assert!(cap_height > 0);
        let bbox = font.font_bbox_1000();
        assert_eq!(bbox, [0, 0, 1000, 1000]);
    }
}
