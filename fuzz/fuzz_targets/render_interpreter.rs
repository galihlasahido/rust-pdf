//! Fuzz target for the content-stream **interpreter itself**
//! (`render::native::render_content_stream`, ISO 32000-1:2008 7.8.2 /
//! Chapter 8/9/11), as opposed to the full-document parser
//! (`parse_pdf`).
//!
//! # Why this target exists
//!
//! Earlier phases of this crate delegated page rendering to Pdfium, a
//! mature C++ engine already fuzzed at scale by Google itself. Since this
//! crate's "Security Hardening" phase migrated to a from-scratch, pure-Rust
//! content-stream interpreter, that interpreter is **brand-new attack
//! surface with none of that prior fuzzing history** -- it did not exist
//! when the rest of this crate's inputs (whole PDF files, embedded fonts,
//! filter payloads) were first put under continuous fuzzing. This target
//! closes that specific gap by feeding the interpreter itself adversarial
//! operator streams directly, bypassing full-document parsing so libFuzzer
//! spends its whole budget mutating *content-stream bytes* rather than
//! wasting most of its time on xref/object-stream structure that
//! `parse_pdf` already covers.
//!
//! # Attack surface deliberately baited via a fixed, hostile `/Resources`
//!
//! A content stream on its own (with no `/Resources` at all) can already
//! exercise deep `q`/`Q` nesting and huge path-operator runs, but it
//! cannot reach the recursive constructs (Form XObject / Type 3 glyph
//! procedures / ExtGState soft-mask groups) without a `/Resources`
//! dictionary to resolve names against -- and building a *useful*
//! (self- or mutually-referential) resource graph out of raw fuzzer bytes
//! would mostly just waste the fuzzer's mutation budget on dictionary
//! syntax it doesn't need to explore (that syntax is already this crate's
//! `object`/`parser` modules' job, covered by `parse_pdf`). So this target
//! builds one **fixed, deliberately pathological** `/Resources` dictionary
//! by hand (not fuzzer input) containing:
//!
//! - `/XObject /RecA` and `/XObject /RecB`: two Form XObjects whose own
//!   content streams invoke each other (`/RecB Do` / `/RecA Do`), so a
//!   fuzzer-supplied `/RecA Do` or `/RecB Do` in the driving content
//!   stream immediately engages a mutually-recursive Form XObject cycle
//!   (see [`MAX_FORM_XOBJECT_DEPTH`] in `render::native::interpreter`).
//! - `/Font /T3`: a Type 3 font whose glyph procedure for `'A'` shows the
//!   text `"A"` using the very same font -- a direct self-reference (see
//!   `MAX_TYPE3_DEPTH` in `render::native::font`).
//! - `/ExtGState /GS1`: a soft mask (`/SMask`) whose transparency group is
//!   itself a Form XObject that sets the very same soft mask again via
//!   `gs`, so a fuzzer-driven `/GS1 gs` reaches nested transparency-group
//!   recursion through the soft-mask path too.
//!
//! Every fuzzer-mutated byte then becomes the *driving* top-level content
//! stream, free to combine these in any order/depth/count: `q`/`Q`
//! flooding, path-operator flooding (`m`/`l`/`c`/`v`/`y`/`re`), `Do`/`Tf`/
//! `Tj`/`gs` invoking the above recursive constructs, and arbitrary
//! operator soup interleaving all of it -- exactly the adversarial classes
//! named in this phase's task (deeply nested `q`/`Q`, huge path-operator
//! counts, self-referential Form XObject/Type3 recursion, pathological
//! nested transparency groups).
//!
//! # What "success" means here
//!
//! `render_content_stream` returning `Ok` *or* a structured `Err` are both
//! fine outcomes -- see [`rust_pdf::render::native::NativeRenderError`]'s
//! own module docs for the resource-limit errors it now returns
//! (`OperatorBudgetExceeded`, `RenderTimeBudgetExceeded`,
//! `GraphicsStateStackOverflow`) precisely so pathological input like this
//! target generates fails gracefully instead of hanging or aborting. The
//! only unacceptable outcome is a panic, a hang past the interpreter's own
//! `MAX_RENDER_DURATION` wall-clock budget, or unbounded memory growth.
#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use rust_pdf::object::{Object, PdfArray, PdfDictionary, PdfName, PdfStream};
use rust_pdf::render::native::render_content_stream;
use rust_pdf::Rectangle;

/// Deliberately tiny output raster: this target is about interpreter
/// *logic* robustness, not rasterization throughput, and a small pixmap
/// keeps each fuzzer iteration cheap so far more iterations/second fit in
/// any given fuzzing time budget.
const WIDTH: u32 = 32;
const HEIGHT: u32 = 32;

fn media_box() -> Rectangle {
    Rectangle::new(0.0, 0.0, 200.0, 200.0)
}

fn form_xobject(bbox: [f64; 4], content: &[u8]) -> Object {
    let mut dict = PdfDictionary::new();
    dict.set("Subtype", Object::Name(PdfName::new_unchecked("Form")));
    dict.set(
        "BBox",
        Object::Array(PdfArray::from_objects(bbox.iter().map(|v| Object::Real(*v)).collect())),
    );
    Object::Stream(PdfStream::with_dictionary(dict, content.to_vec()))
}

fn type3_font_dict() -> PdfDictionary {
    let mut font_dict = PdfDictionary::new();
    font_dict.set("Subtype", Object::Name(PdfName::new_unchecked("Type3")));
    let mut matrix = PdfArray::new();
    for v in [0.001, 0.0, 0.0, 0.001, 0.0, 0.0] {
        matrix.push(Object::Real(v));
    }
    font_dict.set("FontMatrix", Object::Array(matrix));

    let mut encoding = PdfDictionary::new();
    let mut diffs = PdfArray::new();
    diffs.push(Object::Integer(65));
    diffs.push(Object::Name(PdfName::new_unchecked("glyphA")));
    encoding.set("Differences", Object::Array(diffs));
    font_dict.set("Encoding", Object::Dictionary(encoding));

    let mut char_procs = PdfDictionary::new();
    // Self-referential: this glyph procedure shows "A" again using the
    // very same font resource name, directly recursing into itself.
    char_procs.set(
        "glyphA",
        Object::Stream(PdfStream::new(b"BT /T3 12 Tf 0 0 Td (A) Tj ET".to_vec())),
    );
    font_dict.set("CharProcs", Object::Dictionary(char_procs));

    font_dict.set("FirstChar", Object::Integer(65));
    let mut widths = PdfArray::new();
    widths.push(Object::Integer(1000));
    font_dict.set("Widths", Object::Array(widths));
    font_dict
}

/// One fixed, hostile `/Resources` dictionary (see the module docs for
/// why it's hand-built rather than fuzzer-derived), built once and reused
/// across every fuzzer iteration.
fn resources() -> &'static PdfDictionary {
    static RESOURCES: OnceLock<PdfDictionary> = OnceLock::new();
    RESOURCES.get_or_init(|| {
        let mut resources = PdfDictionary::new();

        // Mutually-recursive Form XObjects.
        let rec_a = form_xobject([0.0, 0.0, 200.0, 200.0], b"/RecB Do");
        let rec_b = form_xobject([0.0, 0.0, 200.0, 200.0], b"/RecA Do");
        let mut xobjects = PdfDictionary::new();
        xobjects.set("RecA", rec_a);
        xobjects.set("RecB", rec_b);
        resources.set("XObject", Object::Dictionary(xobjects));

        // Self-referential Type 3 font.
        let mut fonts = PdfDictionary::new();
        fonts.set("T3", Object::Dictionary(type3_font_dict()));
        resources.set("Font", Object::Dictionary(fonts));

        // ExtGState with a soft-mask group whose own content re-selects
        // the same ExtGState -- nested transparency-group/soft-mask
        // recursion via `gs`.
        let smask_group_content = b"/GS1 gs 0 0 200 200 re f";
        let mut smask_group_dict = PdfDictionary::new();
        smask_group_dict.set("Subtype", Object::Name(PdfName::new_unchecked("Form")));
        smask_group_dict.set(
            "BBox",
            Object::Array(PdfArray::from_objects(vec![
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(200.0),
                Object::Real(200.0),
            ])),
        );
        let mut group = PdfDictionary::new();
        group.set("S", Object::Name(PdfName::new_unchecked("Transparency")));
        smask_group_dict.set("Group", Object::Dictionary(group));
        let smask_group_stream = PdfStream::with_dictionary(smask_group_dict, smask_group_content.to_vec());

        let mut smask_dict = PdfDictionary::new();
        smask_dict.set("S", Object::Name(PdfName::new_unchecked("Luminosity")));
        smask_dict.set("G", Object::Stream(smask_group_stream));

        let mut gs1 = PdfDictionary::new();
        gs1.set("SMask", Object::Dictionary(smask_dict));
        let mut ext_gstates = PdfDictionary::new();
        ext_gstates.set("GS1", Object::Dictionary(gs1));
        resources.set("ExtGState", Object::Dictionary(ext_gstates));

        resources
    })
}

fuzz_target!(|data: &[u8]| {
    // The fuzzer's bytes are the *driving* content stream: free to
    // combine deep `q`/`Q` nesting, path-operator floods, and `Do`/`Tf`/
    // `gs` invocations of the hostile resources above in any order,
    // depth, or repetition count.
    let _ = render_content_stream(data, WIDTH, HEIGHT, media_box(), Some(resources()));
});
