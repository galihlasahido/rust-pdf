//! Large-file **RSS (resident set size)** measurement (remediation of the
//! "Large-File Render Benchmark" gap, part (b)).
//!
//! # The gap this closes
//!
//! [`rust_pdf::parser::PdfReader::from_file`] memory-maps its input (via
//! `memmap2`) specifically so that opening -- and reading a handful of
//! pages from -- a multi-gigabyte PDF does not require multi-gigabyte
//! process memory (see the "Large-file streaming" section of
//! `src/parser/mod.rs`'s module docs). That was, until this test, an
//! architectural claim only: nothing in the repository actually opened a
//! real multi-gigabyte file and measured this process's real RSS against
//! its size.
//!
//! This test builds (or reuses a cached) real ~2GB / 10,000-page PDF on
//! disk, opens it via [`PdfReader::from_file`], structurally locates and
//! fully decodes the content streams of its first two pages (the
//! "baca ... 1-2 halaman" the task calls for), reads this process's
//! actual RSS via the platform `ps` utility, and asserts it stays far
//! below the fixture's on-disk size.
//!
//! # Measuring RSS
//!
//! There is no portable stdlib API for a process's own RSS. This uses
//! `ps -o rss= -p <pid>` (a real kernel-reported resident-set-size
//! query, not an estimate), which is supported by both macOS (BSD `ps`)
//! and Linux (`procps`/`busybox` `ps`) -- the two platforms a Tauri
//! desktop app built on this crate targets outside of Windows. This
//! avoids reading Linux-only `/proc/self/status` (would not run on
//! macOS, this project's primary development platform) or adding a new
//! dependency (e.g. `libc` + `mach`/`task_info` FFI) for a single
//! ignored, manually-run test.
//!
//! # Running
//!
//! This test is `#[ignore]`d by default (building/opening a ~2GB fixture
//! is far too slow/heavy for a routine `cargo test`). Run it explicitly,
//! with `--exact` naming *this* test (not the sibling
//! `build_fixture_subprocess_entrypoint` "test" below, which is only a
//! subprocess entrypoint -- see [`ensure_fixture_out_of_process`]'s docs
//! for why it exists and must not run in the same process as this one):
//!
//! ```sh
//! cargo test --release --features parser --test large_file_rss_bench -- \
//!   --ignored --exact rss_stays_far_below_file_size_when_reading_large_fixture --nocapture
//! ```
//!
//! The fixture is cached at
//! `<temp dir>/rust_pdf_bench_10000pages_2gb_rss.pdf` and reused (by size
//! check) across runs. See `tests/large_file_render_bench.rs` for the
//! sibling wall-clock-time benchmark (which uses its own, separately
//! cached fixture so the two can be run/measured independently without
//! interfering with each other's process memory).

#![cfg(feature = "parser")]

use rust_pdf::prelude::*;
use std::path::PathBuf;
use std::time::Instant;

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
    match std::env::var("RUST_PDF_BENCH_RSS_FIXTURE_PATH") {
        Ok(p) => PathBuf::from(p),
        Err(_) => std::env::temp_dir().join("rust_pdf_bench_10000pages_2gb_rss.pdf"),
    }
}

fn build_filler_page(filler_text: &str) -> Page {
    let content = ContentBuilder::new().text("F1", 8.0, 36.0, 806.0, filler_text);
    let mut page = PageBuilder::a4().content(content).build();
    page.add_font("F1", Font::from(Standard14Font::Helvetica));
    page
}

/// Builds a small, ordinary content page (used for the first couple of
/// pages this test actually reads back) -- kept distinct from the filler
/// pages so `decode_all()`'s returned byte count for those pages is small
/// and predictable, not itself gigabytes.
fn build_small_page(index: usize) -> Page {
    let content = ContentBuilder::new().text(
        "F1",
        12.0,
        72.0,
        760.0,
        &format!("Large-file RSS benchmark fixture -- page {index}."),
    );
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

    // One filler page's worth of plain-ASCII text (no PDF string
    // escaping needed), sized so
    // `(page_count - 2) * filler_len ~= target_bytes`.
    let unit = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod \
                tempor incididunt ut labore et dolore magna aliqua ";
    let filler_len_target = (target_bytes as usize) / (page_count.max(3) - 2);
    let repeats = filler_len_target / unit.len() + 1;
    let filler_text = unit.repeat(repeats);

    eprintln!(
        "large_file_rss_bench: building {page_count}-page fixture (~{:.2} GB target) at {}",
        target_bytes as f64 / 1e9,
        path.display()
    );
    let build_start = Instant::now();

    // Pages 0 and 1 are the ones this test actually reads back -- kept
    // small/ordinary (see build_small_page's docs). Pages 2..page_count
    // carry the filler text that makes the file ~2GB.
    let mut builder = DocumentBuilder::new().title("rust-pdf large-file RSS benchmark fixture");
    builder = builder.page(build_small_page(0));
    if page_count > 1 {
        builder = builder.page(build_small_page(1));
    }
    for i in 2..page_count {
        builder = builder.page(build_filler_page(&filler_text));
        if i % 2000 == 0 {
            eprintln!("large_file_rss_bench: constructed {i}/{page_count} pages");
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
        "large_file_rss_bench: fixture built in {:?}, {} pages, {:.2} GB on disk",
        build_start.elapsed(),
        page_count,
        bytes_written as f64 / 1e9
    );
}

/// Name of the `#[test]` function below used as an out-of-process fixture
/// builder -- see [`ensure_fixture_out_of_process`]'s docs for why this
/// indirection exists.
const BUILD_SUBPROCESS_TEST_NAME: &str = "build_fixture_subprocess_entrypoint";

/// Ensures the fixture exists at `path` and is at least
/// `target_file_bytes() * 0.9` bytes, building it **in a separate child
/// process** if not (a cached fixture from a previous run with the same
/// size target is reused in-process, since no construction happens in
/// that case).
///
/// # Why a subprocess for (re)building
///
/// Building the ~2GB fixture means holding roughly that much `Page`/
/// `String` data in whichever process's heap runs
/// `DocumentBuilder`/`Document::save_to_file`. Even after that `Document`
/// is dropped, the system allocator does not necessarily return every
/// freed page back to the OS immediately -- so building the fixture and
/// then measuring "my own RSS" in the *same* process risks measuring
/// leftover allocator arena size from fixture construction, not the RSS
/// this test actually cares about (opening + reading an
/// *already-existing* large file, which is the real-world scenario the
/// audited claim is about). Building in a fresh child process -- which
/// exits, fully releasing all its memory back to the OS, once the
/// fixture is written -- keeps the RSS measured by
/// [`rss_stays_far_below_file_size_when_reading_large_fixture`] clean of
/// that confound. This was confirmed empirically during development: an
/// earlier in-process version of this test measured a "before opening
/// the file" RSS baseline already inflated by fixture construction.
fn ensure_fixture_out_of_process(path: &std::path::Path) {
    let min_acceptable = (target_file_bytes() as f64 * 0.9) as u64;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() >= min_acceptable {
            eprintln!(
                "large_file_rss_bench: reusing cached fixture at {} ({:.2} GB)",
                path.display(),
                meta.len() as f64 / 1e9
            );
            return;
        }
    }

    let exe = std::env::current_exe().expect(
        "this test binary's own path must be resolvable (needed to build the fixture \
         out-of-process)",
    );
    eprintln!(
        "large_file_rss_bench: fixture missing/undersized, building it in a fresh subprocess \
         ({} {BUILD_SUBPROCESS_TEST_NAME} --exact --ignored --nocapture) so this test's own RSS \
         measurement isn't polluted by fixture-construction memory",
        exe.display()
    );
    let status = std::process::Command::new(&exe)
        .args([
            BUILD_SUBPROCESS_TEST_NAME,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .status()
        .expect("spawning the fixture-building subprocess must succeed");
    assert!(
        status.success(),
        "the fixture-building subprocess exited with {status}; see its output above"
    );

    let meta = std::fs::metadata(path)
        .expect("fixture file must exist after the building subprocess exits successfully");
    assert!(
        meta.len() >= min_acceptable,
        "fixture-building subprocess exited successfully but the resulting file ({} bytes) is \
         smaller than the expected minimum ({min_acceptable} bytes)",
        meta.len()
    );
}

/// Internal entrypoint: builds the shared fixture and exits. Invoked as a
/// subprocess by [`ensure_fixture_out_of_process`] -- see its docs. Not
/// meant to be run directly (though doing so is harmless: it just builds
/// the same cached fixture file the real test uses).
#[test]
#[ignore = "internal: invoked as a subprocess by ensure_fixture_out_of_process to build the \
            fixture in an isolated process; not a real assertion on its own"]
fn build_fixture_subprocess_entrypoint() {
    build_fixture(&fixture_path());
}

/// Reads this process's current resident-set size (RSS) in bytes via
/// `ps -o rss= -p <pid>`. See the module docs' "Measuring RSS" section
/// for why this (rather than `/proc/self/status` or a new FFI
/// dependency) is used. Returns `None` if `ps` is unavailable or its
/// output doesn't parse -- callers should skip (not fail) the test in
/// that case, since this is an environment limitation, not a defect
/// under test.
#[cfg(unix)]
fn current_rss_bytes() -> Option<u64> {
    let pid = std::process::id().to_string();
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .ok()
        .map(|kb| kb * 1024)
}

#[cfg(not(unix))]
fn current_rss_bytes() -> Option<u64> {
    // No portable non-`ps`-based RSS query is wired up for non-Unix
    // targets; the test below skips itself (with an explanatory message)
    // rather than failing when this returns `None`.
    None
}

#[test]
#[ignore = "builds/reads a ~2GB, 10,000-page fixture; run explicitly, see module docs"]
fn rss_stays_far_below_file_size_when_reading_large_fixture() {
    let path = fixture_path();
    ensure_fixture_out_of_process(&path);

    let file_bytes = std::fs::metadata(&path)
        .expect("fixture file must exist")
        .len();

    let rss_before = match current_rss_bytes() {
        Some(rss) => rss,
        None => {
            eprintln!(
                "skipping rss_stays_far_below_file_size_when_reading_large_fixture: could not \
                 read this process's RSS via `ps -o rss=` on this platform."
            );
            return;
        }
    };

    // ---- The actual measurement: open the ~2GB fixture through the
    // real, memory-mapped `PdfReader::from_file` entry point, then
    // structurally locate and fully decode two pages' content streams
    // (ISO 32000-1 7.7.3.3's /Contents) -- exactly what
    // `extract_text`/`render_page` do per page. ----
    let open_start = Instant::now();
    let reader =
        PdfReader::from_file(&path).expect("opening the large fixture via mmap must succeed");
    assert_eq!(reader.page_count(), page_count());

    let pages_to_read = 2.min(page_count());
    let mut total_content_bytes_read = 0usize;
    for i in 0..pages_to_read {
        let page_dict = reader
            .get_page(i)
            .unwrap_or_else(|| panic!("page {i} must resolve via the page tree"));
        let contents_ref = match page_dict.get("Contents") {
            Some(Object::Reference(id)) => *id,
            other => {
                panic!("expected page {i}'s /Contents to be an indirect reference, got {other:?}")
            }
        };
        let stream = match reader.resolve_reference(contents_ref) {
            Some(Object::Stream(s)) => s,
            other => panic!("expected page {i}'s /Contents to resolve to a stream, got {other:?}"),
        };
        let decoded = stream
            .decode_all()
            .expect("decoding a page's content stream must succeed");
        total_content_bytes_read += decoded.len();
    }
    let read_elapsed = open_start.elapsed();

    let rss_after = current_rss_bytes().expect(
        "RSS was readable before opening the fixture, so it must be readable again immediately \
         after -- a `None` here would mean `ps` itself became unavailable mid-test",
    );

    eprintln!(
        "large_file_rss_bench: fixture {:.2} GB on disk ({} pages); opened + read {} page(s)' \
         content ({total_content_bytes_read} bytes decoded) in {read_elapsed:?}; RSS before \
         open: {:.1} MB, RSS after: {:.1} MB (delta: {:.1} MB)",
        file_bytes as f64 / 1e9,
        page_count(),
        pages_to_read,
        rss_before as f64 / 1e6,
        rss_after as f64 / 1e6,
        (rss_after as f64 - rss_before as f64) / 1e6,
    );

    // The core assertion this test exists to prove: RSS *after* opening
    // the file and reading a couple of pages' content stays far below
    // the fixture's on-disk size. A naive "read the whole file into a
    // Vec<u8>" implementation would push RSS to within a few percent of
    // `file_bytes`; the mmap-backed, lazy-parsing design this crate uses
    // should stay a small fraction of it regardless of file size, since
    // only the header/xref/trailer plus the handful of objects actually
    // touched (2 page dictionaries + 2 content streams, not 10,000 pages'
    // worth) are ever copied into the process's own heap.
    assert!(
        rss_after < file_bytes / 4,
        "RSS after opening + reading {pages_to_read} page(s) ({rss_after} bytes) is not far \
         below the {file_bytes}-byte fixture -- expected well under 25% of it, which would \
         indicate the whole (or a large fraction of the) file ended up copied into process \
         memory instead of staying memory-mapped"
    );
}
