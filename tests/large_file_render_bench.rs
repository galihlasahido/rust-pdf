//! Large-file **render** benchmark (remediation of the "Large-File Render
//! Benchmark" gap).
//!
//! # The gap this closes
//!
//! A previous "Large File Streaming" phase reported a number like "588ms
//! / 8-thread" for opening/accessing a large document, but that number
//! only ever exercised [`rust_pdf::parser::PdfReader::get_page`] --
//! structural access to a page *dictionary* (ISO 32000-1 7.7.3.3), which
//! is exactly what [`PdfReader`]'s memory-mapped, lazy-parsing design (see
//! `src/parser/mod.rs` module docs) is built to make cheap. It never
//! actually **rasterized pixels**
//! ([`rust_pdf::render::PdfRenderer::render_page`]), which is a
//! completely different code path (document-structure resolution plus
//! content-stream interpretation/rasterization). No prior committed test
//! or benchmark built a realistic ~2GB / 10,000-page fixture and measured
//! *that* path's wall-clock time.
//!
//! This test does exactly that: it builds (or reuses a cached, previously
//! built) real ~2GB / 10,000-page PDF on disk, opens it through
//! [`PdfRenderer::open_file`] (the same entry point
//! `src/tauri_commands/commands.rs`'s `render_page` command uses), calls
//! the real [`PdfRenderer::render_page`] for page 0, and asserts + prints
//! the actual measured wall-clock time.
//!
//! # Why page 0 has modest (not inflated) content
//!
//! Pages `1..PAGE_COUNT` carry large filler text so the *file* reaches
//! the ~2GB target; page 0 -- the one this benchmark actually renders --
//! has ordinary, modest content (matching the style of the fixtures in
//! `tests/render_tests.rs`). That is deliberate: the property under test
//! is "rendering one ordinary page of a huge file is fast regardless of
//! the file's total size/page count", not "rendering an artificially
//! huge single page is fast". If `render_page` (or document loading)
//! scaled with total file size/page count instead of the requested
//! page's own content, this test's timing would regress sharply.
//!
//! # Running
//!
//! This test is `#[ignore]`d by default: building a ~2GB fixture is far
//! too slow/heavy for a routine `cargo test`. This pure-Rust rendering
//! pipeline needs no native library to fetch/bind beforehand, so running
//! it explicitly is just:
//!
//! ```sh
//! cargo test --release --features render --test large_file_render_bench \
//!   -- --ignored --nocapture
//! ```
//!
//! The fixture is cached at
//! `<temp dir>/rust_pdf_bench_10000pages_2gb.pdf` and reused (by size
//! check) across runs so repeated benchmark runs during development don't
//! pay the ~2GB build/write cost every time. Delete that file (or set
//! `RUST_PDF_BENCH_FIXTURE_PATH` to a fresh path) to force a rebuild.
//!
//! Set `RUST_PDF_BENCH_PAGE_COUNT` / `RUST_PDF_BENCH_TARGET_BYTES` to
//! smaller values for a quick smoke test of this file's *logic* (not the
//! real ~2GB/10,000-page claim -- only the committed defaults below prove
//! that).

#![cfg(feature = "render")]

use rust_pdf::prelude::*;
use rust_pdf::render::PdfRenderer;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Total pages in the fixture -- matches the "~10,000 page" claim under
/// audit. Overridable via `RUST_PDF_BENCH_PAGE_COUNT` for a cheap smoke
/// test of this file's logic; the committed default is what actually
/// proves the audited claim.
fn page_count() -> usize {
    env_usize("RUST_PDF_BENCH_PAGE_COUNT", 10_000)
}

/// Target total on-disk fixture size -- matches the "~2GB" claim under
/// audit. Overridable via `RUST_PDF_BENCH_TARGET_BYTES` (see
/// [`page_count`]).
fn target_file_bytes() -> u64 {
    env_usize("RUST_PDF_BENCH_TARGET_BYTES", 2_100_000_000) as u64
}

fn env_usize(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v| v > 0)
        .unwrap_or(default)
}

fn fixture_path() -> PathBuf {
    match std::env::var("RUST_PDF_BENCH_FIXTURE_PATH") {
        Ok(p) => PathBuf::from(p),
        Err(_) => std::env::temp_dir().join("rust_pdf_bench_10000pages_2gb.pdf"),
    }
}

/// Builds page 0: the page this benchmark actually renders. Modest,
/// representative content -- see the module docs' "Why page 0 has modest
/// content" section.
fn build_benchmark_page(page_count: usize) -> Page {
    let content = ContentBuilder::new()
        .save_state()
        .fill_color(Color::rgb(0.10, 0.20, 0.40))
        .rect(0.0, 780.0, 595.0, 62.0)
        .fill()
        .fill_color(Color::WHITE)
        .text(
            "F1",
            20.0,
            40.0,
            800.0,
            "Large-file render benchmark fixture",
        )
        .restore_state()
        .text_block(
            TextBuilder::new()
                .font("F1", 12.0)
                .position(40.0, 720.0)
                .leading(16.0)
                .show("This is page 0 of a large PDF fixture generated for the")
                .next_line()
                .show("\"Large-File Render Benchmark\" remediation task")
                .next_line()
                .show(format!(
                    "(rust-pdf/tests/large_file_render_bench.rs, {page_count} total pages)."
                )),
        );
    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::Helvetica));
    page
}

/// Builds one filler page: large enough plain-ASCII text content (chosen
/// to need no PDF string escaping) so the whole document reaches the
/// ~2GB target. These pages are never rendered by this benchmark -- only
/// page 0 is -- they exist purely to make the *file* (and page count)
/// realistically large, matching the audited "~2GB / 10,000 pages" claim.
fn build_filler_page(filler_text: &str) -> Page {
    let content = ContentBuilder::new().text("F1", 8.0, 36.0, 806.0, filler_text);
    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::Helvetica));
    page
}

/// Builds the ~2GB / 10,000-page fixture and writes it directly to
/// `path` (streaming via [`Document::save_to_file`]'s `BufWriter<File>`,
/// not through an intermediate `Vec<u8>` the size of the whole file).
fn build_fixture(path: &std::path::Path) {
    let page_count = page_count();
    let target_bytes = target_file_bytes();

    // One filler page's worth of plain-ASCII text, sized so
    // `(page_count - 1) * filler_len ~= target_bytes`.
    let unit = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod \
                tempor incididunt ut labore et dolore magna aliqua ";
    let filler_len_target = (target_bytes as usize) / (page_count.max(2) - 1);
    let repeats = filler_len_target / unit.len() + 1;
    let filler_text = unit.repeat(repeats);

    eprintln!(
        "large_file_render_bench: building {page_count}-page fixture (~{:.2} GB target) at {}",
        target_bytes as f64 / 1e9,
        path.display()
    );
    let build_start = Instant::now();

    let mut builder = DocumentBuilder::new().title("rust-pdf large-file render benchmark fixture");
    builder = builder.page(build_benchmark_page(page_count));
    for i in 1..page_count {
        builder = builder.page(build_filler_page(&filler_text));
        if i % 2000 == 0 {
            eprintln!("large_file_render_bench: constructed {i}/{page_count} pages");
        }
    }

    let document = builder
        .build()
        .expect("building the large fixture document must not fail");
    document
        .save_to_file(path)
        .expect("writing the large fixture to disk must not fail");

    let bytes_written = std::fs::metadata(path)
        .expect("fixture file must exist after save_to_file")
        .len();
    eprintln!(
        "large_file_render_bench: fixture built in {:?}, {} pages, {:.2} GB on disk",
        build_start.elapsed(),
        page_count,
        bytes_written as f64 / 1e9
    );
}

/// Ensures the fixture exists at `path` and is at least
/// `target_file_bytes() * 0.9` bytes (a cached fixture from a previous
/// run with the same size target is reused rather than rebuilt; anything
/// smaller -- absent, truncated, or built with a smaller target -- is
/// rebuilt).
fn ensure_fixture(path: &std::path::Path) {
    let min_acceptable = (target_file_bytes() as f64 * 0.9) as u64;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() >= min_acceptable {
            eprintln!(
                "large_file_render_bench: reusing cached fixture at {} ({:.2} GB)",
                path.display(),
                meta.len() as f64 / 1e9
            );
            return;
        }
    }
    build_fixture(path);
}

/// Generous upper bound on real-path render time for page 0, chosen to be
/// far above any plausible legitimate render latency on slow CI hardware
/// while still catching a regression where opening/rendering scales with
/// total file size or page count instead of the single requested page's
/// own content (which would blow well past this on a ~2GB/10,000-page
/// file -- see the module docs).
const MAX_ACCEPTABLE_RENDER_TIME: Duration = Duration::from_secs(15);

#[test]
#[ignore = "builds/reads a ~2GB, 10,000-page fixture; run explicitly, see module docs"]
fn render_page_zero_of_a_2gb_10000_page_fixture() {
    let path = fixture_path();
    ensure_fixture(&path);

    let file_bytes = std::fs::metadata(&path)
        .expect("fixture file must exist")
        .len();

    // ---- The actual measurement: open + render page 0 via the real
    // production path (`PdfRenderer::open_file` / `PdfRenderer::render_page`),
    // exactly as `src/tauri_commands/commands.rs`'s `render_page` command
    // uses it. ----
    let start = Instant::now();
    let renderer =
        PdfRenderer::open_file(&path).expect("opening the large fixture must succeed");
    let page_count_reported = renderer.page_count();
    let image = renderer
        .render_page(0, 96.0, None)
        .expect("rendering page 0 of the large fixture must succeed");
    let elapsed = start.elapsed();

    eprintln!(
        "large_file_render_bench: opened + rendered page 0 of a {:.2} GB / {}-page fixture \
         (reported page count: {page_count_reported}) in {elapsed:?} \
         ({}x{} px raster)",
        file_bytes as f64 / 1e9,
        page_count(),
        image.width(),
        image.height(),
    );

    // A4 at 96 DPI: 595.28pt * 96/72 ~= 793px wide, 841.89pt * 96/72 ~=
    // 1123px tall (ISO 32000-1 7.7.3.3 MediaBox, points -> pixels at the
    // requested DPI). Confirms this rendered real page geometry, not a
    // stub/placeholder image.
    assert!(
        image.width() > 700 && image.width() < 900,
        "unexpected raster width {}",
        image.width()
    );
    assert!(
        image.height() > 1000 && image.height() < 1200,
        "unexpected raster height {}",
        image.height()
    );
    assert_eq!(page_count_reported, page_count());

    assert!(
        elapsed < MAX_ACCEPTABLE_RENDER_TIME,
        "opening + rendering page 0 of a {:.2} GB / {}-page fixture took {elapsed:?}, expected \
         well under {MAX_ACCEPTABLE_RENDER_TIME:?} -- render_page's cost should track the \
         requested page's own content, not the whole file/page count",
        file_bytes as f64 / 1e9,
        page_count(),
    );
}
