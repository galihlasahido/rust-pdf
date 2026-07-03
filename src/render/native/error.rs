//! Error and warning types for the native (pure-Rust) content-stream
//! interpreter.
//!
//! Per this crate's mandatory rules, content streams are untrusted input:
//! this module distinguishes between
//!
//! - [`NativeRenderError`]: a *hard* failure that aborts the render before
//!   (or instead of) producing a pixel buffer -- always a structured,
//!   `Display`-able error, never a panic; and
//! - [`RenderWarning`]: a *soft*, recoverable condition (an operator or
//!   color space this phase doesn't implement, a malformed operand, a
//!   dangling resource reference, ...) that is recorded and skipped so the
//!   rest of the content stream still renders, rather than the whole page
//!   silently coming out blank or the interpreter panicking.

use thiserror::Error;

/// Hard failures that abort content-stream interpretation.
#[derive(Debug, Error, PartialEq)]
pub enum NativeRenderError {
    /// The requested output raster has a zero or otherwise invalid
    /// dimension.
    #[error("invalid output dimensions: {width}x{height}")]
    InvalidDimensions {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
    },

    /// `tiny_skia::Pixmap::new` failed to allocate the requested raster
    /// (e.g. `width * height` overflowed or the allocator refused).
    #[error("failed to allocate a {width}x{height} px raster")]
    PixmapAllocationFailed {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
    },

    /// The page's `/MediaBox` (ISO 32000-1 7.7.3.3) has zero or negative
    /// width/height, so no page-space-to-device-space mapping exists.
    #[error(
        "media box has zero or negative extent: [{llx} {lly} {urx} {ury}]"
    )]
    DegenerateMediaBox {
        /// Lower-left X.
        llx: f64,
        /// Lower-left Y.
        lly: f64,
        /// Upper-right X.
        urx: f64,
        /// Upper-right Y.
        ury: f64,
    },

    /// The `q` (save graphics state) operator nesting depth exceeded
    /// [`super::MAX_GRAPHICS_STATE_DEPTH`]. A well-formed content stream
    /// never needs anywhere near this many nested saves; this bounds a
    /// crafted content stream's ability to force unbounded `Vec` growth
    /// (a memory-exhaustion attempt via a `q` flood), per this crate's
    /// mandatory untrusted-input rules.
    #[error(
        "graphics state stack depth exceeded {max} (crafted/corrupt content stream?)"
    )]
    GraphicsStateStackOverflow {
        /// The configured maximum depth.
        max: usize,
    },

    /// The total number of content-stream operators/inline-images
    /// executed by this render -- summed across the top-level content
    /// stream *and* every Form XObject / Type 3 glyph procedure /
    /// transparency-group recursion it triggers (they all share one
    /// running counter, see [`super::interpreter::RenderBudget`]) --
    /// exceeded [`super::interpreter::MAX_OPERATOR_COUNT`]. A well-formed
    /// page's content stream, even a genuinely complex one, has no
    /// business anywhere near this many operators; this bounds a crafted
    /// content stream's ability to hang the interpreter with an
    /// operator-count-based attack that never overflows any single stack
    /// depth (e.g. a very long, but never deeply-nested, flood of `q Q`
    /// or path-construction operators).
    #[error(
        "content-stream operator budget exceeded {max} (crafted/corrupt content stream?)"
    )]
    OperatorBudgetExceeded {
        /// The configured maximum operator count.
        max: usize,
    },

    /// Wall-clock time spent interpreting this render exceeded
    /// [`super::interpreter::MAX_RENDER_DURATION`]. This is the backstop
    /// for pathological inputs that are *not* well-described by any of the
    /// other bounded counters above -- e.g. a content stream that is
    /// legal and under every other limit, but combines several expensive
    /// operations (large paths, many glyphs, deep-but-legal transparency
    /// nesting) in a way whose *wall-clock cost*, not any single count, is
    /// what actually matters to a caller with a render-time SLA.
    #[error("render exceeded its {max_millis}ms time budget (crafted/corrupt or pathologically expensive content stream?)")]
    RenderTimeBudgetExceeded {
        /// The configured maximum render duration, in milliseconds.
        max_millis: u64,
    },
}

/// A recoverable condition encountered while interpreting a content
/// stream: something this phase of the native renderer does not (yet)
/// implement, or a malformed-but-not-fatal construct in the input. The
/// affected operator/construct is skipped (treated as a no-op) and
/// interpretation continues with the rest of the stream.
///
/// This type exists so callers/tests can assert *why* a render might not
/// look like a full-fidelity PDF viewer's output, instead of the gap being
/// silent. See `src/render/native/mod.rs` module docs for the full list of
/// gaps this phase is known to have (text, images/XObjects, non-Device
/// color spaces, shading, JBIG2/JPX/Type1-CFF, ICC color management).
#[derive(Debug, Clone, PartialEq)]
pub enum RenderWarning {
    /// A content-stream operator this phase of the interpreter does not
    /// implement (e.g. text-showing, `Do`, `sh`, non-Device color space
    /// selection). Its operands are discarded and it is treated as a
    /// no-op; painting/graphics-state operators elsewhere in the stream
    /// are unaffected.
    UnsupportedOperator {
        /// The operator keyword (e.g. `"Tj"`, `"Do"`, `"scn"`).
        operator: String,
    },
    /// A colour space named by `cs`/`CS`, or an image's `/ColorSpace`,
    /// could not be resolved into something this phase can paint (an
    /// unrecognised family such as `/Lab`, a malformed colour space
    /// array/object, an unresolvable `/Resources /ColorSpace` name, or a
    /// Separation/DeviceN whose tint-transform function
    /// [`super::function`] could not parse -- see
    /// [`super::colorspace::ColorSpace::Unsupported`]). The current
    /// fill/stroke colour is left unchanged (for `cs`/`CS`/`sc`/`scn`) or
    /// the image is skipped (for an image's `/ColorSpace`) rather than an
    /// approximate/guessed colour being produced.
    UnsupportedColorSpace {
        /// Human-readable reason (see
        /// `colorspace::ColorSpace::description`).
        reason: String,
    },
    /// `scn`/`SCN` named a Pattern colour (ISO 32000-1 §8.7) -- Patterns
    /// are out of scope this phase (see [`super`] module docs). The
    /// current colour is left unchanged.
    PatternColorUnsupported,
    /// An `ICCBased` colour space (ISO 32000-1 §8.6.5.5) was resolved by
    /// *approximation* (its `/Alternate` entry, or a heuristic guess from
    /// `/N`) rather than true ICC colour management -- see
    /// [`super::colorspace`]'s module docs for why. Recorded (at most
    /// [`super::interpreter::MAX_WARNINGS`] times, like every other
    /// warning) so callers can tell "accurate" from "approximated" colour
    /// output rather than this being silently indistinguishable.
    IccColorApproximated,
    /// `Do` named an XObject resource not present in `/Resources
    /// /XObject` (or no `resources` dictionary was supplied at all).
    /// Nothing is painted for it.
    MissingXObjectResource {
        /// The resource name that could not be resolved.
        name: String,
    },
    /// `Do` named an XObject resource whose `/Subtype` is something this
    /// phase does not paint at all: anything other than `/Image` or
    /// `/Form` (e.g. a PostScript `/PS` XObject). Form XObjects *are*
    /// painted this phase (see [`super::interpreter::Interpreter::do_xobject`]);
    /// this variant is only for subtypes beyond that.
    UnsupportedXObjectSubtype {
        /// The resource name this applies to.
        name: String,
        /// The XObject's declared `/Subtype` (or `"(missing)"`).
        subtype: String,
    },
    /// An image XObject or inline image's filter chain includes
    /// `JBIG2Decode` or `JPXDecode` -- **there is no mature pure-Rust
    /// decoder for either in the ecosystem today** (a hard, structural
    /// gap, not a "didn't get to it yet" one; see [`super::image`]'s
    /// module docs). The image area is left unpainted (a documented
    /// placeholder, never silently blank without this warning, never a
    /// panic).
    UnsupportedImageFilter {
        /// The resource name (or `"(inline)"` for an inline image) this
        /// applies to.
        name: String,
        /// The unsupported filter name (`"JBIG2Decode"` or
        /// `"JPXDecode"`).
        filter: String,
    },
    /// An image XObject or inline image could not be decoded for a reason
    /// *other* than the JBIG2/JPX gap above: a missing/invalid
    /// `/Width`/`/Height`/`/BitsPerComponent`, a filter decode error (e.g.
    /// corrupt JPEG data, or `CCITTFaxDecode` with the unimplemented
    /// `K >= 0` variant -- see `crate::filter::ccitt`'s own documented
    /// limitation), or decoded byte count that doesn't match the declared
    /// geometry. The image area is left unpainted.
    ImageDecodeFailed {
        /// The resource name (or `"(inline)"` for an inline image) this
        /// applies to.
        name: String,
        /// Human-readable failure reason.
        reason: String,
    },
    /// A `gs` operator named an `ExtGState` resource that was not found in
    /// `/Resources/ExtGState` (or no `resources` dictionary was supplied
    /// at all). Treated as a no-op: the graphics state is left unchanged.
    MissingExtGState {
        /// The resource name that could not be resolved.
        name: String,
    },
    /// A `d` (set dash pattern) operator supplied an array tiny-skia
    /// rejects (e.g. all-zero lengths, a negative length, or a
    /// non-finite phase). Falls back to a solid (non-dashed) stroke.
    InvalidDashPattern,
    /// The content stream ended partway through a syntactically malformed
    /// statement (e.g. an unterminated string or array). Bytes from that
    /// point to the end of the stream are not interpreted; everything
    /// parsed before it still rendered normally.
    TruncatedContentStream,
    /// The graphics-state-stack "restore" operator (`Q`) was invoked with
    /// no matching `q` (stack already at its initial depth). Ignored
    /// rather than treated as an error, since unbalanced `q`/`Q` is a
    /// (surprisingly common) real-world producer bug and not something
    /// that should block an otherwise-renderable page.
    UnbalancedRestore,
    /// `Tf` named a resource not present in `/Resources /Font` (or no
    /// `resources` dictionary was supplied at all, or the entry wasn't a
    /// dereferenced dictionary -- see `native`'s module docs on the
    /// pre-resolved-`Resources` assumption). Text shown before the next
    /// (valid) `Tf` renders nothing.
    MissingFontResource {
        /// The resource name that could not be resolved.
        name: String,
    },
    /// A text-showing operator ran with no active font at all (`Tf` was
    /// never called, or only ever named a missing resource). The string
    /// operand is discarded; no glyphs are painted.
    MissingActiveFont,
    /// `Tf` selected a font resource this phase cannot rasterize --
    /// see `crate::render::native::font`'s module docs for the full list
    /// of reasons (no embedded program, Type1/bare-CFF, or an otherwise
    /// unparseable program) and which of those is the documented,
    /// structural gap versus an unexpected/adversarial input. Recorded
    /// once per resource name; subsequent text shown with it renders
    /// nothing (but still advances the pen using its declared widths).
    UnsupportedFontProgram {
        /// The resource name (`/Resources /Font /<name>`) this applies to.
        resource_name: String,
        /// Human-readable reason (see
        /// `font::UnsupportedFontReason`'s `Display` impl).
        reason: String,
    },
    /// A Type 3 glyph procedure (ISO 32000-1:2008 9.6.5) recursed past
    /// [`super::font::MAX_TYPE3_DEPTH`] -- a self-referential or
    /// mutually-recursive set of Type 3 fonts (untrusted/adversarial
    /// input). The glyph is skipped (nothing painted for it) rather than
    /// recursing further.
    Type3RecursionLimitExceeded,
    /// A Form XObject (directly, or as an ExtGState `/SMask` group, or a
    /// transparency-group `/Group` used via `Do`) recursed past
    /// [`super::interpreter::MAX_FORM_XOBJECT_DEPTH`] -- a
    /// self-referential or mutually-recursive set of Form XObjects
    /// (untrusted/adversarial input, e.g. a Form whose own content stream
    /// paints itself via `Do`, or an ExtGState `/SMask` group that selects
    /// an ExtGState referencing itself). Rendering stops for that branch
    /// (an empty/transparent result for the offending group) rather than
    /// recursing further.
    FormXObjectRecursionLimitExceeded,
    /// An ExtGState `/BM` (blend mode, ISO 32000-1 §11.3.5) named (as a
    /// bare name, or every entry of an array) something other than one of
    /// the 16 standard blend modes this phase maps 1:1 onto
    /// `tiny_skia::BlendMode` (`Normal`/`Compatible`, `Multiply`,
    /// `Screen`, `Overlay`, `Darken`, `Lighten`, `ColorDodge`,
    /// `ColorBurn`, `HardLight`, `SoftLight`, `Difference`, `Exclusion`,
    /// `Hue`, `Saturation`, `Color`, `Luminosity`). Falls back to `Normal`
    /// per ISO 32000-1 §11.3.5's "if the viewer does not recognise any of
    /// the requested blend modes, it shall use `Normal`" rule.
    UnsupportedBlendMode {
        /// The unrecognised name (the bare name, or the first array
        /// element, whichever was encountered).
        name: String,
    },
    /// An ExtGState `/SMask` entry named something other than `/None` or a
    /// well-formed soft-mask dictionary with a readable `/G` (transparency
    /// group Form XObject stream) -- e.g. `/G` missing, not a stream, or
    /// the dictionary itself malformed. Treated as `/SMask /None` (no soft
    /// mask restriction) for this and subsequent `gs` invocations of the
    /// same (or any) `ExtGState` until a valid one is set.
    InvalidSoftMaskGroup,
    /// An ExtGState `/SMask` soft-mask dictionary carried a `/TR` (transfer
    /// function) and/or `/BC` (backdrop colour) entry other than the
    /// identity default -- both are ignored (identity transfer function,
    /// default backdrop per `/S`'s Luminosity-is-black/Alpha-is-transparent
    /// convention) rather than applied, a documented simplification (ISO
    /// 32000-1 §11.6.5.2).
    SoftMaskParameterIgnored {
        /// Which entry was ignored: `"TR"` or `"BC"`.
        parameter: &'static str,
    },
    /// An image XObject's `/SMask` (ISO 32000-1 §11.6.5.3: a per-image
    /// soft-mask, distinct from the ExtGState group soft mask above) could
    /// not be decoded (missing/invalid geometry, an unsupported filter
    /// such as `JBIG2Decode`/`JPXDecode`, or a colour space other than
    /// DeviceGray). The base image still paints, but **fully opaque**
    /// (the soft mask is skipped entirely) rather than the render failing
    /// or the base image itself going unpainted.
    ImageSoftMaskDecodeFailed {
        /// The resource name (or `"(inline)"`) this applies to.
        name: String,
        /// Human-readable failure reason.
        reason: String,
    },
    /// A single path object (ISO 32000-1 8.5.2: everything since the last
    /// path-painting operator) accumulated more points than
    /// [`super::path::MAX_PATH_POINTS_PER_PATH`] -- e.g. a content stream
    /// consisting of an enormous run of `l`/`c` operators with no
    /// intervening painting operator (untrusted/adversarial input; a
    /// well-formed page's single path object never needs anywhere near
    /// this many points). Recorded once per offending path object; every
    /// further construction call against that same path object is
    /// silently dropped (the path keeps whatever geometry it had
    /// accumulated up to the limit and is still painted/clipped normally
    /// with that truncated geometry when a painting operator eventually
    /// runs, rather than the whole render aborting).
    PathPointBudgetExceeded {
        /// The configured maximum point count for one path object.
        max: usize,
    },
}

impl std::fmt::Display for RenderWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderWarning::UnsupportedOperator { operator } => {
                write!(f, "unsupported operator: {operator}")
            }
            RenderWarning::UnsupportedColorSpace { reason } => {
                write!(f, "unsupported colour space: {reason}")
            }
            RenderWarning::PatternColorUnsupported => {
                write!(f, "Pattern colour space unsupported, colour left unchanged")
            }
            RenderWarning::IccColorApproximated => {
                write!(f, "ICCBased colour space approximated (no ICC colour management), not colour-accurate")
            }
            RenderWarning::MissingXObjectResource { name } => {
                write!(f, "Do referenced a missing XObject resource: /{name}")
            }
            RenderWarning::UnsupportedXObjectSubtype { name, subtype } => {
                write!(f, "XObject /{name} has unsupported /Subtype {subtype}, not painted")
            }
            RenderWarning::UnsupportedImageFilter { name, filter } => {
                write!(f, "image /{name} uses unsupported filter {filter} (no pure-Rust decoder exists), painted as a placeholder")
            }
            RenderWarning::ImageDecodeFailed { name, reason } => {
                write!(f, "image /{name} could not be decoded: {reason}")
            }
            RenderWarning::MissingExtGState { name } => {
                write!(f, "ExtGState resource not found: /{name}")
            }
            RenderWarning::InvalidDashPattern => {
                write!(f, "invalid dash pattern, falling back to solid stroke")
            }
            RenderWarning::TruncatedContentStream => {
                write!(f, "content stream truncated at a malformed statement")
            }
            RenderWarning::UnbalancedRestore => {
                write!(f, "Q with no matching q, ignored")
            }
            RenderWarning::MissingFontResource { name } => {
                write!(f, "Tf referenced a missing font resource: /{name}")
            }
            RenderWarning::MissingActiveFont => {
                write!(f, "text shown with no active font (Tf never succeeded)")
            }
            RenderWarning::UnsupportedFontProgram { resource_name, reason } => {
                write!(f, "font /{resource_name} cannot be rendered: {reason}")
            }
            RenderWarning::Type3RecursionLimitExceeded => {
                write!(f, "Type 3 glyph procedure recursion limit exceeded, glyph skipped")
            }
            RenderWarning::FormXObjectRecursionLimitExceeded => {
                write!(f, "Form XObject recursion limit exceeded, group rendering stopped")
            }
            RenderWarning::UnsupportedBlendMode { name } => {
                write!(f, "unrecognised blend mode /{name}, falling back to Normal")
            }
            RenderWarning::InvalidSoftMaskGroup => {
                write!(f, "ExtGState /SMask malformed or /G unreadable, treated as /None")
            }
            RenderWarning::SoftMaskParameterIgnored { parameter } => {
                write!(f, "ExtGState /SMask /{parameter} ignored (not applied)")
            }
            RenderWarning::ImageSoftMaskDecodeFailed { name, reason } => {
                write!(f, "image /{name} /SMask could not be decoded ({reason}), base image painted fully opaque")
            }
            RenderWarning::PathPointBudgetExceeded { max } => {
                write!(f, "path object exceeded {max} points, further construction on it dropped")
            }
        }
    }
}
