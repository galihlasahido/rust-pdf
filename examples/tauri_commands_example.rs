//! Demonstrates the `tauri_commands` layer (`src/tauri_commands/`) end to
//! end, calling the plain `..._impl` async functions directly -- exactly
//! what a real `#[tauri::command]` wrapper does internally (see
//! `src/tauri_commands/commands.rs`) -- without needing a running Tauri
//! application/window at all.
//!
//! Covered in order:
//! 1. [`open_document_impl`] -- opens a freshly-generated PDF and returns
//!    a [`DocumentHandle`].
//! 2. [`render_page_impl`] -- rasterizes page 0 to RGBA.
//! 3. [`extract_text_impl`] -- pulls text out of every page, with a
//!    progress callback.
//! 4. [`search_text_impl`] -- finds a substring across every page, also
//!    with progress.
//! 5. [`apply_edit_impl`] -- an in-memory structural edit
//!    (`EditOperation::ReplaceText`).
//! 6. [`add_annotation_impl`] -- adds a highlight annotation.
//! 7. [`save_document_impl`] -- persists the edited document to disk.
//! 8. Structured-error handling: two calls that are *expected* to fail
//!    (an unknown document handle, and an out-of-range page index),
//!    showing how a Tauri frontend would branch on
//!    [`rust_pdf::tauri_commands::ErrorCode`] rather than getting a panic
//!    or an opaque string.
//!
//! Run with:
//! ```text
//! cargo run --example tauri_commands_example --features full,tauri
//! ```

use rust_pdf::prelude::*;
use rust_pdf::tauri_commands::commands::{
    self, ApplyEditRequest, EditOperation, ExtractTextRequest, OpenDocumentRequest,
    RenderPageRequest, SaveDocumentRequest, SaveMode, SearchTextRequest,
};
use rust_pdf::tauri_commands::progress::no_progress;
use rust_pdf::tauri_commands::{AppState, DocumentHandle, ErrorCode, ProgressEvent};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("tests/output")?;

    // -----------------------------------------------------------------
    // 0. Build a small source PDF with the plain document-building API,
    //    to give the commands below something real to open. A Tauri
    //    frontend would instead point `open_document` at a path the user
    //    picked via a native file dialog.
    // -----------------------------------------------------------------
    let content = ContentBuilder::new()
        .text("F1", 18.0, 72.0, 760.0, "Hello, Tauri commands!")
        .text("F1", 12.0, 72.0, 700.0, "Second line for search/extract demo.");
    let page = PageBuilder::a4()
        .font("F1", Standard14Font::Helvetica)
        .content(content)
        .build();
    let bytes = DocumentBuilder::new()
        .title("tauri_commands_example source")
        .page(page)
        .build()?
        .save_to_bytes()?;
    let source_path = std::env::current_dir()?.join("tests/output/tauri_commands_example_source.pdf");
    std::fs::write(&source_path, &bytes)?;
    println!("wrote {}", source_path.display());

    // `AppState` is what a real Tauri app registers once at startup via
    // `tauri::Builder::manage(AppState::new())` (see the "Tauri
    // desktop-app integration" section of README.md); every command
    // receives it as `tauri::State<'_, AppState>`. Here we own it
    // directly and call the `..._impl` functions the `#[tauri::command]`
    // wrappers forward to.
    let state = AppState::new();

    // -----------------------------------------------------------------
    // 1. open_document
    // -----------------------------------------------------------------
    let opened = commands::open_document_impl(
        &state,
        OpenDocumentRequest {
            path: source_path.to_string_lossy().into_owned(),
            password: None,
        },
    )
    .await?;
    let handle = DocumentHandle(opened.handle);
    println!(
        "open_document: handle={} page_count={}",
        opened.handle, opened.page_count
    );

    // -----------------------------------------------------------------
    // 2. render_page
    // -----------------------------------------------------------------
    let rendered = commands::render_page_impl(
        &state,
        RenderPageRequest {
            handle: handle.0,
            page_index: 0,
            dpi: 96.0,
            viewport: None,
        },
    )
    .await?;
    println!(
        "render_page: {}x{} RGBA raster ({} bytes)",
        rendered.width,
        rendered.height,
        rendered.rgba.len()
    );

    // -----------------------------------------------------------------
    // 3. extract_text (with a progress callback, like a Tauri wrapper's
    //    `emitting_progress_reporter` would build -- except this one just
    //    prints instead of emitting a Tauri IPC event).
    // -----------------------------------------------------------------
    let print_progress: rust_pdf::tauri_commands::ProgressReporter =
        std::sync::Arc::new(|event: ProgressEvent| {
            println!(
                "  [progress] {} {}/{:?} - {:?}",
                event.operation, event.current, event.total, event.message
            );
        });

    let pages = commands::extract_text_impl(
        &state,
        ExtractTextRequest {
            handle: handle.0,
            page_index: None,
        },
        print_progress.clone(),
    )
    .await?;
    for page in &pages {
        println!(
            "extract_text: page {} -> {:?}",
            page.page_index, page.text
        );
    }

    // -----------------------------------------------------------------
    // 4. search_text
    // -----------------------------------------------------------------
    let matches = commands::search_text_impl(
        &state,
        SearchTextRequest {
            handle: handle.0,
            query: "search/extract".to_string(),
            case_sensitive: false,
        },
        print_progress,
    )
    .await?;
    for m in &matches {
        println!(
            "search_text: hit on page {} at offset {} -> {:?}",
            m.page_index, m.offset, m.snippet
        );
    }

    // -----------------------------------------------------------------
    // 5. apply_edit (structural edit, in memory only until save_document)
    // -----------------------------------------------------------------
    let edit_result = commands::apply_edit_impl(
        &state,
        ApplyEditRequest {
            handle: handle.0,
            operation: EditOperation::ReplaceText {
                page_index: 0,
                find: "Hello, Tauri commands!".to_string(),
                replace: "Hello, edited Tauri commands!".to_string(),
            },
        },
    )
    .await?;
    println!(
        "apply_edit: ReplaceText replaced {} occurrence(s)",
        edit_result.replacements
    );

    // -----------------------------------------------------------------
    // 6. add_annotation (highlight the (now-edited) first line)
    // -----------------------------------------------------------------
    let annotation_result = commands::add_annotation_impl(
        &state,
        commands::AddAnnotationRequest {
            handle: handle.0,
            annotation: commands::AnnotationRequest::Highlight {
                page_index: 0,
                quads: vec![(72.0, 755.0, 320.0, 775.0)],
                color: commands::ColorRequest {
                    r: 1.0,
                    g: 1.0,
                    b: 0.0,
                },
            },
        },
    )
    .await?;
    println!(
        "add_annotation: created highlight object {}:{}",
        annotation_result.annotation.number, annotation_result.annotation.generation
    );

    // -----------------------------------------------------------------
    // 7. save_document
    // -----------------------------------------------------------------
    let output_path = std::env::current_dir()?.join("tests/output/tauri_commands_example_edited.pdf");
    let saved = commands::save_document_impl(
        &state,
        SaveDocumentRequest {
            handle: handle.0,
            path: Some(output_path.to_string_lossy().into_owned()),
            mode: SaveMode::FullRewrite,
        },
        no_progress(),
    )
    .await?;
    println!(
        "save_document: wrote {} bytes to {}",
        saved.bytes_written, saved.path
    );

    // -----------------------------------------------------------------
    // 8. Structured error handling.
    //
    // Every command returns `Result<T, CommandError>` -- never a panic --
    // so a Tauri frontend can branch on `CommandError::code`
    // (`rust_pdf::tauri_commands::ErrorCode`) to decide what to show the
    // user, instead of parsing a free-form message string.
    // -----------------------------------------------------------------

    // 8a. An unknown/stale document handle -> `ErrorCode::NotFound`.
    match commands::render_page_impl(
        &state,
        RenderPageRequest {
            handle: 999_999,
            page_index: 0,
            dpi: 96.0,
            viewport: None,
        },
    )
    .await
    {
        Ok(_) => panic!("expected render_page on an unknown handle to fail"),
        Err(err) => {
            assert_eq!(err.code, ErrorCode::NotFound);
            println!(
                "structured error (expected): render_page on unknown handle -> code={:?} message={:?}",
                err.code, err.message
            );
        }
    }

    // 8b. An out-of-range page index on a real, open document ->
    // `ErrorCode::InvalidArgument`.
    match commands::extract_text_impl(
        &state,
        ExtractTextRequest {
            handle: handle.0,
            page_index: Some(42),
        },
        no_progress(),
    )
    .await
    {
        Ok(_) => panic!("expected extract_text on an out-of-range page index to fail"),
        Err(err) => {
            assert_eq!(err.code, ErrorCode::InvalidArgument);
            println!(
                "structured error (expected): extract_text with out-of-range page -> code={:?} message={:?}",
                err.code, err.message
            );
        }
    }

    Ok(())
}
