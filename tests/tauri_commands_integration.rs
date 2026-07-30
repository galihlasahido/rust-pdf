//! End-to-end integration test for the "Tauri Integration" phase
//! (`rust_pdf::tauri_commands`), covering its Definition of Done:
//!
//! - open a (large-ish) document, render a page, extract/search its
//!   text, edit it, and save it -- all through the same `..._impl`
//!   functions a `#[tauri::command]` wrapper calls -- **without blocking
//!   a single-threaded async executor** (standing in for a Tauri app's
//!   own async-command executor/UI thread), and
//! - every command surfaces structured errors for invalid input rather
//!   than panicking.
//!
//! # What "large" means here
//!
//! This does not generate a literal 1GB+ file: doing so in an automated
//! test suite (run routinely, including in CI) would be slow and I/O
//! heavy for marginal additional proof. Instead it builds a
//! several-hundred-page, multi-megabyte document -- large enough that
//! `extract_text`/`save_document` take a measurable amount of wall-clock
//! time -- and uses that window to prove the concurrency property
//! itself (see [`no_blocking_of_a_single_threaded_executor`] below),
//! which does not get stronger with a bigger file, only slower to run.
//! Manually opening/rendering an actual 1GB+ real-world PDF was part of
//! this phase's task report instead of this automated suite; see that
//! report for the result.

#![cfg(feature = "tauri")]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rust_pdf::prelude::*;
use rust_pdf::tauri_commands::commands::{
    apply_edit_impl, extract_text_impl, open_document_impl, render_page_impl, save_document_impl,
    search_text_impl, ApplyEditRequest, EditOperation, ExtractTextRequest, OpenDocumentRequest,
    RenderPageRequest, SaveDocumentRequest, SaveMode, SearchTextRequest,
};
use rust_pdf::tauri_commands::progress::no_progress;
use rust_pdf::tauri_commands::state::AppState;

const PAGE_COUNT: usize = 400;

/// Builds a multi-hundred-page PDF with enough per-page text that
/// `extract_text`/`search_text` over the whole document, and a
/// full-rewrite `save_document`, each take a measurable amount of time.
fn build_large_document() -> Vec<u8> {
    let paragraph = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
                      Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. "
        .repeat(20);

    let mut builder = DocumentBuilder::new().title("rust-pdf tauri_commands integration fixture");
    for i in 0..PAGE_COUNT {
        let content = ContentBuilder::new()
            .text("F1", 10.0, 72.0, 760.0, &format!("Page {i} of {PAGE_COUNT} -- {paragraph}"));
        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(content)
            .build();
        builder = builder.page(page);
    }
    builder
        .build()
        .expect("building the large fixture PDF must not fail")
        .save_to_bytes()
        .expect("serializing the large fixture PDF must not fail")
}

/// DoD: "buka file besar dari command, render, edit, save tanpa freeze UI
/// thread".
///
/// Uses `#[tokio::test]`'s default **single-threaded** (`current_thread`)
/// runtime deliberately: that is the strictest possible environment for
/// this property. If `open_document`/`render_page`/`extract_text`/
/// `search_text`/`apply_edit`/`save_document` did any of their real work
/// (parsing, rasterizing, text extraction, saving) directly on the
/// calling task instead of via `WorkerPool`/`RenderActor`'s own OS
/// threads, a concurrently-running "UI heartbeat" task on this same
/// single executor thread would be completely unable to make progress
/// for the whole duration of that work (since nothing else could run
/// until it returned) -- exactly the freeze this phase's DoD prohibits.
/// With a multi-thread runtime this same bug could hide (a second tokio
/// worker thread could happen to run the heartbeat while the first one
/// blocks), so this intentionally does not use `flavor = "multi_thread"`.
#[tokio::test]
async fn no_blocking_of_a_single_threaded_executor() {
    let path = std::env::temp_dir().join(format!(
        "rust_pdf_tauri_integration_large_{}.pdf",
        std::process::id()
    ));
    std::fs::write(&path, build_large_document()).expect("writing the large fixture PDF");

    let state = AppState::with_worker_threads(2);

    // "UI heartbeat": ticks a counter on a fixed cadence for as long as
    // `keep_running` is set, standing in for a native UI event loop (or
    // any other concurrent async task on the same executor) that must
    // keep making progress while PDF work happens in the background.
    let keep_running = Arc::new(AtomicBool::new(true));
    let ticks = Arc::new(AtomicU64::new(0));
    const TICK_INTERVAL: Duration = Duration::from_millis(2);
    let heartbeat = {
        let keep_running = Arc::clone(&keep_running);
        let ticks = Arc::clone(&ticks);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(TICK_INTERVAL);
            while keep_running.load(Ordering::Relaxed) {
                interval.tick().await;
                ticks.fetch_add(1, Ordering::Relaxed);
            }
        })
    };
    // Give the heartbeat a moment to actually start ticking before the
    // PDF work window begins, so the "ticks per window" comparison below
    // isn't skewed by tokio's own task-startup latency.
    tokio::time::sleep(Duration::from_millis(20)).await;

    let ticks_before = ticks.load(Ordering::Relaxed);
    let window_start = Instant::now();

    // ---- The actual command sequence: open -> render -> extract ->
    // search -> edit -> save, all on the *same* (single) executor
    // thread as the heartbeat task above. ----

    let opened = open_document_impl(
        &state,
        OpenDocumentRequest {
            path: path.to_string_lossy().into_owned(),
            password: None,
        },
    )
    .await
    .expect("open_document must succeed on a valid, freshly-written PDF");
    assert_eq!(opened.page_count, PAGE_COUNT);

    // Unlike the previous FFI-backed rendering engine, this pure-Rust
    // pipeline has no native-library-availability precondition (see
    // `rust_pdf::render`'s module docs), so there is nothing to skip here.
    let page = render_page_impl(
        &state,
        RenderPageRequest {
            handle: opened.handle,
            page_index: 0,
            dpi: 96.0,
            viewport: None,
        },
    )
    .await
    .expect("render_page must succeed on a valid, freshly-written PDF");
    assert!(page.width > 0 && page.height > 0);

    let pages = extract_text_impl(
        &state,
        ExtractTextRequest {
            handle: opened.handle,
            page_index: None,
        },
        no_progress(),
    )
    .await
    .expect("extract_text must succeed across every page");
    assert_eq!(pages.len(), PAGE_COUNT);
    assert!(pages[0].text.contains("Lorem ipsum"));

    let matches = search_text_impl(
        &state,
        SearchTextRequest {
            handle: opened.handle,
            query: "Lorem ipsum".to_string(),
            case_sensitive: false,
        },
        no_progress(),
    )
    .await
    .expect("search_text must succeed");
    // Each page's paragraph repeats the sentence containing "Lorem ipsum"
    // 20 times (see `build_large_document`).
    assert_eq!(matches.len(), PAGE_COUNT * 20);

    let edit = apply_edit_impl(
        &state,
        ApplyEditRequest {
            handle: opened.handle,
            operation: EditOperation::ReplaceText {
                page_index: 0,
                find: "Lorem ipsum".to_string(),
                replace: "REDACTED".to_string(),
            },
        },
    )
    .await
    .expect("apply_edit must succeed");
    // Page 0's paragraph contains 20 occurrences of "Lorem ipsum".
    assert_eq!(edit.replacements, 20);

    let save_path = std::env::temp_dir().join(format!(
        "rust_pdf_tauri_integration_large_saved_{}.pdf",
        std::process::id()
    ));
    let saved = save_document_impl(
        &state,
        SaveDocumentRequest {
            handle: opened.handle,
            path: Some(save_path.to_string_lossy().into_owned()),
            mode: SaveMode::FullRewrite,
        },
        no_progress(),
    )
    .await
    .expect("save_document must succeed");
    assert!(saved.bytes_written > 0);

    let window_elapsed = window_start.elapsed();
    let ticks_during_window = ticks.load(Ordering::Relaxed) - ticks_before;

    keep_running.store(false, Ordering::Relaxed);
    // The heartbeat task may be parked in `interval.tick().await`; give it
    // one more tick's worth of time to observe `keep_running == false` and
    // exit, then join it so the test doesn't leak a task.
    let _ = tokio::time::timeout(TICK_INTERVAL * 4, heartbeat).await;

    let saved_bytes = std::fs::read(&save_path).expect("saved file must exist");
    assert!(saved_bytes.starts_with(b"%PDF-"));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&save_path);

    // The core assertion: the heartbeat kept ticking *throughout* the
    // whole open/render/extract/search/edit/save window. A generous
    // (not exact-cadence) threshold is used to stay robust against
    // scheduler jitter on a loaded CI machine while still failing hard
    // if the window froze the executor for any significant fraction of
    // its duration -- a fully-blocked executor would produce ~0 ticks
    // for the entire window regardless of jitter.
    let expected_ticks = (window_elapsed.as_secs_f64() / TICK_INTERVAL.as_secs_f64()) as u64;
    eprintln!(
        "PDF work window: {window_elapsed:?} elapsed, {ticks_during_window} heartbeat ticks \
         observed (~{expected_ticks} expected at a perfect {TICK_INTERVAL:?} cadence)"
    );
    assert!(
        ticks_during_window > 0,
        "UI heartbeat made zero progress while open/render/extract/search/edit/save ran: \
         the executor thread was blocked"
    );
    if expected_ticks >= 10 {
        assert!(
            ticks_during_window * 3 >= expected_ticks,
            "UI heartbeat ticked only {ticks_during_window} times over a window where ~{expected_ticks} \
             were expected -- the executor thread looks like it was mostly blocked, not just jittery"
        );
    }
}

/// DoD: "semua command punya error handling yang mengembalikan pesan
/// jelas ke frontend (bukan panic yang crash app)" -- exercised here
/// against a battery of invalid inputs across every command, in one
/// place, as a single end-to-end regression guard (per-command
/// unit-level versions of these also live in `src/tauri_commands/commands.rs`'s
/// own test module).
#[tokio::test]
async fn every_command_reports_structured_errors_instead_of_panicking() {
    let state = AppState::with_worker_threads(1);

    let open_err = open_document_impl(
        &state,
        OpenDocumentRequest {
            path: "/no/such/file/anywhere.pdf".to_string(),
            password: None,
        },
    )
    .await
    .expect_err("opening a missing file must be a structured error, not a panic");
    assert_eq!(open_err.code, rust_pdf::tauri_commands::ErrorCode::NotFound);

    let render_err = render_page_impl(
        &state,
        RenderPageRequest {
            handle: 424_242,
            page_index: 0,
            dpi: 72.0,
            viewport: None,
        },
    )
    .await
    .expect_err("rendering an unknown handle must be a structured error, not a panic");
    assert_eq!(render_err.code, rust_pdf::tauri_commands::ErrorCode::NotFound);

    let extract_err = extract_text_impl(
        &state,
        ExtractTextRequest {
            handle: 424_242,
            page_index: None,
        },
        no_progress(),
    )
    .await
    .expect_err("extracting text from an unknown handle must be a structured error, not a panic");
    assert_eq!(extract_err.code, rust_pdf::tauri_commands::ErrorCode::NotFound);

    let search_err = search_text_impl(
        &state,
        SearchTextRequest {
            handle: 424_242,
            query: "anything".to_string(),
            case_sensitive: false,
        },
        no_progress(),
    )
    .await
    .expect_err("searching an unknown handle must be a structured error, not a panic");
    assert_eq!(search_err.code, rust_pdf::tauri_commands::ErrorCode::NotFound);

    let edit_err = apply_edit_impl(
        &state,
        ApplyEditRequest {
            handle: 424_242,
            operation: EditOperation::DeletePage { page_index: 0 },
        },
    )
    .await
    .expect_err("editing an unknown handle must be a structured error, not a panic");
    assert_eq!(edit_err.code, rust_pdf::tauri_commands::ErrorCode::NotFound);

    let save_err = save_document_impl(
        &state,
        SaveDocumentRequest {
            handle: 424_242,
            path: None,
            mode: SaveMode::Incremental,
        },
        no_progress(),
    )
    .await
    .expect_err("saving an unknown handle must be a structured error, not a panic");
    assert_eq!(save_err.code, rust_pdf::tauri_commands::ErrorCode::NotFound);
}
