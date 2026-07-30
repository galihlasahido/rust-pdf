//! Demonstrates permanent content redaction and its audit trail
//! (`src/editor/redact.rs`, `src/editor/audit.rs`).
//!
//! Walks the full "redact before external distribution" workflow:
//!
//! 1. Build a small document containing an obviously sensitive line
//!    ("CONFIDENTIAL-SSN-123-45-6789").
//! 2. Sanity-check that the sensitive text is, as expected, sitting in
//!    the file as literal, greppable raw bytes before any redaction
//!    (this crate's `DocumentBuilder` writes content streams
//!    uncompressed by default, so this is a meaningful, not vacuous,
//!    check - see the comment at the grep site below).
//! 3. [`EditableDocument::apply_redaction`] the rectangle the sensitive
//!    line sits in - this actually deletes the underlying content-stream
//!    text-showing operator, it does not just draw a black box over it.
//! 4. Show `save_incremental_to_bytes` now (correctly) refuses to run:
//!    an incremental update only appends bytes, so the pre-redaction
//!    text would still be sitting, fully intact, earlier in the file.
//! 5. `save_full_rewrite_to_bytes` instead - the only save mode that
//!    only ever serializes objects still reachable from `/Root`, so the
//!    orphaned pre-redaction content is dropped rather than retained.
//! 6. Prove removal two ways against the saved output:
//!    a) raw file bytes: the literal sensitive string is gone;
//!    b) decoded content stream (reopen the saved file and decode the
//!    page's content bytes): still gone. (b) is the rigorous proof - a
//!    full rewrite Flate-compresses content streams (`with_compression`,
//!    `src/editor/save.rs`), so on its own a raw-byte scan of
//!    *compressed* output can't tell "actually removed" apart from
//!    "merely compressed"; doing (a) first against the
//!    known-uncompressed original, then (b) against the decoded
//!    rewritten stream, closes that gap.
//! 7. Read back [`EditableDocument::audit_log`] (from the *reopened*,
//!    saved file - proving the log itself survives the full-rewrite
//!    round trip, not just living in the in-memory session) and print
//!    every recorded field.
//!
//! Known limitation surfaced by this workflow (see the module docs in
//! `src/editor/redact.rs` for the full list): redaction here works at
//! whole-text-run granularity, is geometry-approximate (font `/Widths`-
//! derived, not full glyph shaping), and does not descend into Form
//! XObjects. None of that applies to this example's flat, single-run
//! page, but it matters for real-world documents with partial-overlap
//! or nested content.
//!
//! Run with:
//! ```text
//! cargo run --example redaction_example --features full
//! ```

use rust_pdf::prelude::*;

const SENSITIVE_TEXT: &str = "CONFIDENTIAL-SSN-123-45-6789";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("tests/output")?;

    // 1. Build a document with one clearly sensitive line plus one
    //    unrelated line that must survive redaction untouched.
    let content = ContentBuilder::new()
        .text("F1", 12.0, 72.0, 760.0, "Quarterly customer statement")
        .text("F1", 24.0, 72.0, 700.0, SENSITIVE_TEXT);
    let page = PageBuilder::a4().font("F1", Standard14Font::Helvetica).content(content).build();
    let original_bytes = DocumentBuilder::new().title("Customer Statement").page(page).build()?.save_to_bytes()?;

    // 2. Sanity check: before any redaction, the sensitive text is
    //    sitting in the file as literal raw bytes (DocumentBuilder does
    //    not compress content streams by default), i.e. it would be
    //    trivially recoverable by anyone who greps the file. This is
    //    the baseline the rest of this example proves goes away.
    assert!(
        String::from_utf8_lossy(&original_bytes).contains(SENSITIVE_TEXT),
        "expected the pre-redaction file to contain the sensitive text in the clear"
    );
    println!("[1] pre-redaction file ({} bytes) contains the sensitive text in the clear.", original_bytes.len());

    // 3. Reopen for editing and permanently redact the area the
    //    sensitive line occupies (page default user space, generously
    //    covering the estimated text-run bounding box - see
    //    src/editor/redact.rs's "Known limitations" on why the estimate
    //    is deliberately on the generous side).
    let mut doc = EditableDocument::from_bytes(original_bytes)?;
    let redact_rect = Rectangle::new(50.0, 680.0, 600.0, 730.0);
    let entry = doc.apply_redaction(0, redact_rect, "compliance-bot@example.com", "PII removal - SSN before external distribution")?;
    println!(
        "[2] apply_redaction: removed {} text run(s), {} image(s); {} /ToUnicode entr(y/ies) pruned.",
        entry.text_runs_removed, entry.images_removed, entry.tounicode_entries_pruned
    );
    assert_eq!(entry.text_runs_removed, 1, "expected exactly the sensitive line's text run to be removed");

    // 4. An incremental save is correctly refused now: it only ever
    //    appends bytes, so the pre-redaction content would still be
    //    fully intact earlier in the file - not actually a redaction.
    match doc.save_incremental_to_bytes() {
        Err(PdfError::Editor(EditorError::RedactionRequiresFullRewrite)) => {
            println!("[3] save_incremental_to_bytes correctly refused: redaction requires a full rewrite.");
        }
        other => panic!("expected RedactionRequiresFullRewrite, got {other:?}"),
    }

    // 5. A full rewrite only serializes objects still reachable from
    //    /Root, so the orphaned pre-redaction content stream body is
    //    actually dropped, not just superseded.
    let redacted_bytes = doc.save_full_rewrite_to_bytes()?;
    std::fs::write("tests/output/redacted_statement.pdf", &redacted_bytes)?;
    println!("[4] wrote tests/output/redacted_statement.pdf ({} bytes).", redacted_bytes.len());

    // 6a. Raw-byte forensic check against the saved file: the literal
    //     sensitive string is gone. (A full rewrite also Flate-
    //     compresses content streams, so by itself this check can't
    //     distinguish "actually removed" from "merely compressed" -
    //     step 6b below closes that gap by decoding first.)
    assert!(
        !String::from_utf8_lossy(&redacted_bytes).contains(SENSITIVE_TEXT),
        "sensitive text must not appear anywhere in the redacted file's raw bytes"
    );
    assert!(
        !redacted_bytes.windows(SENSITIVE_TEXT.len()).any(|w| w == SENSITIVE_TEXT.as_bytes()),
        "sensitive text's raw byte sequence must not occur anywhere in the redacted file"
    );
    println!("[5] raw bytes of the redacted file do not contain the sensitive text.");

    // 6b. Rigorous check: reopen the saved file and decode the page's
    //     content stream (undoing Flate compression) - exactly what a
    //     real forensic tool (e.g. `qpdf --decompress`) would do -
    //     and confirm the sensitive text is gone at that level too,
    //     while the unrelated line survived untouched.
    let reopened = EditableDocument::from_bytes(redacted_bytes)?;
    let page_bytes = reopened.page_content_bytes(reopened.page_id_at(0)?)?;
    let decoded_content = String::from_utf8_lossy(&page_bytes);
    assert!(!decoded_content.contains(SENSITIVE_TEXT), "decoded content stream must not contain the sensitive text");
    assert!(
        decoded_content.contains("Quarterly customer statement"),
        "the unrelated, non-redacted line must survive untouched"
    );
    println!("[6] decoded (post-Flate) content stream confirms the sensitive text is gone; the unrelated line remains.");

    // 7. Read the audit log back from the *reopened* file, proving it
    //    survives the full-rewrite save/reload round trip rather than
    //    only existing in this process's in-memory session.
    let log = reopened.audit_log();
    assert_eq!(log.len(), 1, "expected exactly the one redaction recorded above");
    println!("[7] audit log ({} entr{}):", log.len(), if log.len() == 1 { "y" } else { "ies" });
    for e in log {
        println!(
            "    actor={} reason=\"{}\" timestamp={} page_index={:?} area={:?} text_runs_removed={} images_removed={} tounicode_entries_pruned={}",
            e.actor, e.reason, e.timestamp, e.page_index, e.area, e.text_runs_removed, e.images_removed, e.tounicode_entries_pruned
        );
    }
    assert_eq!(log[0].actor, "compliance-bot@example.com");
    assert_eq!(log[0].page_index, Some(0));
    assert_eq!(log[0].area, Some(redact_rect));

    println!("done: redaction is permanent (raw bytes + decoded content stream), and the audit trail round-trips.");
    Ok(())
}
