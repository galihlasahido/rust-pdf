//! Demonstrates opening a document lazily/memory-mapped
//! ([`rust_pdf::parser::PdfReader::from_file`], `parser` feature's
//! `memmap2` backing -- see `src/parser/mod.rs`'s "Large-file streaming"
//! module docs) and random-access page lookup that never parses the whole
//! document sequentially.
//!
//! # Why this example builds a few hundred pages, not 2GB/10,000 pages
//!
//! The point this example demonstrates -- lazy, mmap-backed opening plus
//! `O(depth)` random page lookup via [`PdfReader::page_ref`]/
//! [`PdfReader::get_page`] -- doesn't need a multi-gigabyte file to show:
//! the *shape* of the API and the access pattern are identical regardless
//! of file size. Building a real ~2GB/10,000-page fixture takes minutes,
//! which would make this example impractical for anyone to just
//! `cargo run` to see how the API is used. The **real** large-scale
//! numbers (a ~2GB, 10,000-page fixture, opened and rendered through the
//! production `PdfRenderer` path) are measured in
//! `tests/large_file_render_bench.rs` (`#[ignore]`d by default -- run
//! explicitly per that file's module docs) and reported in
//! `ARCHITECTURE.md` (search "Large-File Render Benchmark").
//!
//! Run with:
//! ```text
//! cargo run --example large_file_streaming_example --features full
//! ```

use rust_pdf::prelude::*;
use std::time::Instant;

/// Page count for this example's fixture. Big enough that "jump straight
/// to a page deep in the document" is a meaningful demonstration, small
/// enough to build in well under a second. Override with the
/// `RUST_PDF_EXAMPLE_PAGE_COUNT` env var if you want to see the same
/// pattern at a larger (but still practical) scale.
fn page_count() -> usize {
    std::env::var("RUST_PDF_EXAMPLE_PAGE_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n: &usize| n >= 2)
        .unwrap_or(400)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("tests/output")?;
    let fixture_path = std::path::Path::new("tests/output/large_file_streaming_example.pdf");
    let page_count = page_count();

    build_fixture(fixture_path, page_count)?;
    let fixture_bytes = std::fs::metadata(fixture_path)?.len();
    println!(
        "built a {page_count}-page fixture at {} ({fixture_bytes} bytes)",
        fixture_path.display()
    );

    // -----------------------------------------------------------------
    // 1. Open lazily/memory-mapped via `PdfReader::from_file`.
    // -----------------------------------------------------------------
    //
    // This memory-maps the file (via `memmap2`) rather than reading it
    // into a `Vec<u8>` heap buffer, and only parses the header, the
    // cross-reference table/stream, and the trailer up front -- every
    // actual page/content-stream object is parsed lazily, on first
    // access. For this example's modest fixture the wall-clock
    // difference from an eager read isn't observable, but the same open
    // call is what makes opening a real multi-gigabyte PDF not require
    // multi-gigabyte process memory up front (see the module docs above
    // for where that's actually measured).
    let open_start = Instant::now();
    let reader = PdfReader::from_file(fixture_path)?;
    let open_elapsed = open_start.elapsed();
    println!(
        "opened via PdfReader::from_file in {open_elapsed:?} (mmap-backed, no full-document parse yet)"
    );
    println!("reader reports {} page(s)", reader.page_count());

    // -----------------------------------------------------------------
    // 2. Random-access page lookup: jump straight to pages out of
    //    order, never resolving/parsing the pages in between.
    // -----------------------------------------------------------------
    //
    // `PdfReader::page_ref`/`get_page` locate the Nth page by descending
    // the page tree using each `/Pages` node's `/Count` (ISO 32000-1
    // 7.7.3.2) to skip whole sibling subtrees -- not by walking pages
    // `0..index`. Deliberately visit indices out of ascending order
    // (last, then middle, then first) to make it visible that this
    // isn't secretly a sequential scan.
    let targets = [page_count - 1, page_count / 2, 0];
    for &index in &targets {
        let lookup_start = Instant::now();
        let page_ref = reader
            .page_ref(index)
            .unwrap_or_else(|| panic!("page {index} must exist in a {page_count}-page fixture"));
        let page_dict = reader
            .get_page(index)
            .unwrap_or_else(|| panic!("page {index} dictionary must resolve"));
        let lookup_elapsed = lookup_start.elapsed();

        // Prove this is really page `index`'s own content (not, say,
        // page 0's) by resolving its `/Contents` stream directly and
        // reading the marker text this fixture wrote into it -- without
        // ever touching any other page's content stream.
        let marker = read_page_marker(&reader, &page_dict)
            .unwrap_or_else(|| panic!("page {index}'s content stream must decode"));
        let expected_marker = format!("this is page {index} of {page_count}");
        assert_eq!(
            marker, expected_marker,
            "random-accessed page {index} did not contain its own content"
        );

        println!(
            "random-accessed page {index} (object {} {} R) in {lookup_elapsed:?} -> {marker:?}",
            page_ref.number, page_ref.generation
        );
    }

    println!(
        "\nall {} random-access lookups above resolved only the objects on the page tree \
         path down to the requested page (plus that page's own content stream) -- none of \
         them required parsing pages 0..index first.",
        targets.len()
    );

    Ok(())
}

/// Resolves `page_dict`'s `/Contents` stream and returns the marker line
/// [`build_fixture`] wrote into it (`"this is page N of TOTAL"`), or
/// `None` if the content stream doesn't decode/contain it.
fn read_page_marker(reader: &PdfReader, page_dict: &PdfDictionary) -> Option<String> {
    let contents_ref = match page_dict.get("Contents")? {
        Object::Reference(id) => *id,
        _ => return None,
    };
    let stream = match reader.resolve_reference(contents_ref)? {
        Object::Stream(s) => s,
        _ => return None,
    };
    let decoded = stream.decode_all().ok()?;
    let text = String::from_utf8_lossy(&decoded);
    // The fixture's content stream shows the marker text as a single
    // `Tj` string literal, e.g. `(this is page 12 of 400) Tj`.
    let start = text.find("(this is page")?;
    let rest = &text[start + 1..];
    let end = rest.find(')')?;
    Some(rest[..end].to_string())
}

/// Builds a `page_count`-page fixture, each page carrying a distinct
/// marker string (`"this is page N of TOTAL"`) so random access can be
/// verified to have actually landed on the right page. Written directly
/// to disk via [`Document::save_to_file`].
fn build_fixture(path: &std::path::Path, page_count: usize) -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = DocumentBuilder::new().title("rust-pdf large-file streaming example fixture");
    for index in 0..page_count {
        let marker = format!("this is page {index} of {page_count}");
        let content = ContentBuilder::new().text("F1", 14.0, 72.0, 700.0, &marker);
        let page = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(content)
            .build();
        builder = builder.page(page);
    }
    let document = builder.build()?;
    document.save_to_file(path)?;
    Ok(())
}
