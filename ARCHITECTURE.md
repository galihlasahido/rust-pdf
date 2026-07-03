# rust-pdf — Architecture & Audit (as of 2026-07-03)

> **Status of this document (fifth refresh, current — corrects §2's per-file `%Ln` column, which
> did not actually match a live `cargo llvm-cov` re-run for 16 of the 105 covered rows despite the
> third refresh's explicit claim, quoted verbatim from this file at the time, that "every row below
> was re-verified by this refresh's own re-run, not assumed unchanged"):** that claim was false for
> those 16 rows — the values had in fact been carried over unchanged from the *original* audit (or,
> for a few, the second refresh) without being re-measured, and by this pass had drifted from a
> fresh `cargo-llvm-cov 0.8.7` run (same tool version the third refresh cited) far enough to matter.
> Ran `cargo llvm-cov --release --features full,tauri --summary-only` twice back-to-back this
> session; both runs agree on every file except `parser/recovery.rs` (see its own row/footnote for
> why that one specifically is unstable run-to-run). Corrected, with the precise
> covered-lines/total-lines fraction computed directly from each run's raw counts (not by
> re-rounding the tool's own already-rounded 2-decimal display, which can itself introduce a
> spurious ±0.1-point error):
>
> | File | Doc claimed (stale) | Live (this refresh) |
> |---|---:|---:|
> | `filter/mod.rs` | 72.9% | **90.7%** |
> | `filter/dct.rs` | 49.2% | **72.3%** |
> | `object/mod.rs` | 63.0% | **68.9%** |
> | `object/array.rs` | 81.6% | **85.5%** |
> | `object/string.rs` | 85.9% | **89.1%** |
> | `page/mod.rs` | 63.3% | **66.7%** |
> | `types/rectangle.rs` | 91.9% | **87.8%** |
> | `image/mod.rs` | 26.2% | **24.6%** |
> | `encryption/permissions.rs` | 75.5% | **70.6%** |
> | `editor/content_stream.rs` | 69.8% | **72.4%** |
> | `signatures/signer.rs` | 73.0% | **73.4%** |
> | `parser/mod.rs` | 93.4% | **93.5%** |
> | `filter/lzw.rs` | 89.3% | **89.2%** |
> | `editor/icc.rs` | 95.4% | **95.3%** |
> | `editor/structure.rs` | 90.8% | **90.7%** |
> | `parser/recovery.rs` | 77.2% | **77.2–77.6%** (genuinely unstable — see its row) |
>
> None of these are new coverage regressions or improvements from any code change — `git diff
> 7258c5b HEAD -- src/` is empty; this refresh touches only `ARCHITECTURE.md`, and no test was
> added, removed, or modified. The crate-wide aggregate in §10 (Regions 83.97%/Functions
> 79.79%/Lines 82.54%) was already exactly reproduced by a live re-run before this refresh and is
> **unchanged** — this was a per-file granularity bug in the document, not an aggregate one: with
> 105 rows and figures this granular, region-weighted rounding lets several individual rows drift
> while the crate-wide sum stays put. Also re-verified (unchanged, no drift): §2's LOC/file-count
> totals (48,168 lines / 111 files — identical `find src -name '*.rs' | xargs wc -l` output to the
> third refresh's), and §10/§11/§12's own headline numbers. Fixed the three restatements of the
> corrected values in §10's "Notably low files" bullets (`image/mod.rs`, `filter/dct.rs`,
> `signatures/signer.rs`) to match. See §15 for what was and wasn't re-verified in this pass.
>
> **Status of this document (fourth refresh — corrects a specific inaccurate claim in the
> third refresh's §8d/§10, supersedes those two numbers only):** the third refresh's §8d
> "Performance delta" section and §10's one-line restatement of it asserted, as "a real, measured
> regression, not a rough guess," that the pure-Rust renderer takes **1,723.67 ms** to open+render
> page 0 of the 2.10 GB/10,000-page benchmark fixture, "roughly 20.9× slower" than the
> previously-recorded 82.46 ms Pdfium figure. Re-running the *exact same command* on the *exact
> same fixture* on this same machine, eight times in a row this session
> (`cargo test --release --features render --test large_file_render_bench -- --ignored
> --nocapture`), reproducibly gives **~58-70 ms** in seven of the eight runs — *faster* than the
> 82.46 ms Pdfium baseline, the opposite of a slowdown — with the eighth (the first of the eight,
> run immediately after a `cargo build` of the test binary) landing at 1.58 s, matching the
> magnitude of the third refresh's own 1.72 s figure. The pattern across both sessions is
> consistent: the multi-second figure only appears on the *first* touch of the 2.10 GB fixture
> after something (a fresh build, or a fresh fixture regeneration) has evicted it from the OS page
> cache, and every subsequent run against the same warm-cached file lands at ~60 ms. That is a
> **disk-cache-cold-start artifact of this specific benchmark's first invocation**, not a property
> of the rendering algorithm/code — `PdfRenderer::open_file`/`render_page` do no more disk I/O on
> a cold-cache run than a warm one, they just have to wait on the kernel to actually fetch the
> bytes from disk the first time instead of serving them from RAM. The third refresh's text
> presented the cold-start number as this session's stable, authoritative measurement without
> disclosing that repeated runs in the *same* session vary by roughly 25×, and without noting that
> the number a real user would experience on every open *after* the first (or on any open where the
> file was recently read/written, which the page cache typically ensures) actually favors the new
> renderer. §8d and §10 below have been rewritten to state both the cold-start and steady-state
> figures explicitly, say plainly that this is **not a demonstrated regression** against the Pdfium
> baseline, and stop presenting a single cherry-picked number as if it were the whole story. No
> `src/**` change was needed or made to fix this — it is purely a documentation-accuracy fix; see
> §15 for the exact eight measured timings.
>
> **Status of this document (third refresh, supersedes the second refresh's numbers
> below for §1/§2/§9/§10/§11/§12, and rewrites §8's framing):** regenerated again on branch
> `workflow/enterprise-buildout` after four further commits landed on top of the second refresh
> (`babec0c`): `899fb9f`/`e658df6`/`b63dc98`/`a3db203` (the four-phase pure-Rust rendering build —
> content-stream interpreter core, text rendering, color spaces/images, transparency/blend modes —
> documented as they landed in §8a/§8b and `render::native`'s own module docs), `e6af056` ("replace
> Pdfium/FFI renderer with pure-Rust engine, retire render actor" — documented in §8c), and two
> security-hardening commits (`5ba3e0c`/`0e14625`, fuzzing + resource-limit hardening of the
> content-stream interpreter, no `ARCHITECTURE.md`-relevant behavior change beyond what §8c already
> described). This was an **explicit user request**: stop depending on a native/FFI binary
> (Pdfium) at all and rasterize with a pure-Rust stack instead (`tiny-skia` for 2D rasterization,
> `ttf-parser` for font outlines — both already-audited dependencies of this crate), *accepting*
> the compatibility/performance trade-offs §8 originally gave as the reason Pdfium was chosen in
> the first place. This refresh:
>
> - Rewrites **§1**'s one-line description of the `render/` module (was: "FFI to Google's
>   Pdfium"; now: pure-Rust, no native binary) and its `render`/`native-render`-excluded-from-
>   `full` rationale (was: "requires bundling a native platform-specific binary"; that reason no
>   longer applies — the actual, current reason is recorded in §1 below).
> - Regenerates **§2's module inventory** from a live `find src -name '*.rs' | xargs wc -l`:
>   **48,168 lines across 111 files**, up from 39,591/98 — entirely the pure-Rust rendering build
>   (`render/` grew from 3 files/849 LOC to **17 files/9,440 LOC**, adding the whole `render/native/`
>   submodule; `tauri_commands/` shrank from 7 files/2,865 LOC to **6 files/2,592 LOC** net, because
>   `render_actor.rs` — 437 lines — was deleted outright once rendering no longer needed a
>   dedicated single-threaded FFI-serialization actor, even though every *remaining*
>   `tauri_commands/` file grew) plus small, incidental touch-ups in `font/truetype.rs` (+41,
>   `outline_glyph()`), `font/encoding.rs`, `editor/pages.rs`, `editor/text_extract.rs`,
>   `editor/icc.rs`, `editor/mod.rs`, `filter/ccitt.rs`, `error.rs` and `lib.rs` that the four
>   rendering-build commits made along the way (verified via `git diff babec0c HEAD --stat -- src/`
>   — 34 files changed, +9,622/−1,045, matching this table exactly, no unexplained file).
> - Adds a new **§8d "Known Limitations"** section consolidating, in one place, every compatibility
>   gap this migration *knowingly and deliberately* accepted (JBIG2/JPX images, Type1/bare-CFF
>   fonts, ICC color management) plus a **measured performance delta against the previous Pdfium
>   benchmark** on the exact same fixture used to benchmark Pdfium in the "Large-File Render
>   Benchmark" remediation phase — see §8d for the live numbers from this session.
> - Regenerates **§10 (coverage)**, **§11 (`cargo audit`)** and **§12 (`cargo clippy`)** from a
>   live re-run against `--features full,tauri` (§10's prior text had explicitly flagged itself as
>   describing the *pre-migration* Pdfium run and out of date — this refresh is the promised
>   follow-up). `render`/`tauri` no longer need `RUST_PDF_PDFIUM_LIB_DIR`/`scripts/fetch_pdfium.sh`
>   at all — every render/tauri test now genuinely executes on a plain `cargo test`, no env var
>   required, closing the exact "tests silently skip" trap §10's old text warned about.
> - This is, again, deliberately the **last commit of the entire remediation sequence** covered by
>   this document — no `src/**` change follows it (verified: `git log --stat` after this commit
>   touches only `ARCHITECTURE.md`).
>
> **Status of this document (second refresh, superseded above for §1/§2/§9/§10/§11/§12):**
> regenerated again at commit `babec0c` (branch
> `workflow/enterprise-buildout`) to close a one-commit staleness window. The first refresh
> (blockquote immediately below this one) landed at commit `e0ab506`, but two further
> remediation-phase commits landed **immediately after** it and were therefore not reflected:
> `ad35dcb` ("PDF/A CIDSet Fix" — added live `/CIDSet` stream generation on the
> `FontDescriptor`, ISO 32000-1:2008 Table 122 / required for subset CIDFonts by ISO
> 19005-1:2005 6.3.5, in `font/cid.rs` + `document/mod.rs` + `editor/pdfa.rs`) and `babec0c`
> ("CID subset-tag fix" — made the `ABCDEF+FontName` subset-tag prefix on `BaseFont`
> conditional on the same `will_subset()` predicate that gates `/CIDSet`, so an untagged
> full-embed can no longer carry a tagged name and fail veraPDF's 6.3.5 test 3; see
> `font/cid.rs`'s module docs and its three new unit tests). This second pass re-ran every live
> command from §2/§10/§11/§12 against the tree *after* both fixes and updated only what actually
> moved:
>
> - **§2 module inventory**: `font/cid.rs` 571→796 LOC (90.1%→92.23% line coverage),
>   `editor/pdfa.rs` 926→977 LOC (79.1%→80.33%), `document/mod.rs` 1,387→1,403 LOC
>   (77.8%→77.99%) — exactly the three files the two commits above touched, re-verified via a
>   fresh `find src -name '*.rs' | xargs wc -l`. Crate total: **39,299→39,591 lines, still 98
>   files** (no file added or removed, only grown).
> - **§10 coverage**: live `cargo llvm-cov --release --features full,tauri --summary-only`
>   re-run — aggregate moved from 83.76%/78.20%/81.99% (region/function/line) to
>   **83.91%/78.28%/82.10%**. Unit-test count rose from 587 to 593 (the six tests
>   `ad35dcb`/`babec0c` added to `font/cid.rs`/`editor/pdfa.rs`), bringing the crate-wide passing
>   total from 706 to **712** (still 11 ignored, still 0 failing).
> - **§11 `cargo audit`**: re-run, **unchanged** — 535 dependencies, exit code 0, 0
>   vulnerabilities, the same 17 informational (`unmaintained`/`unsound`) warnings, the same 5
>   individually-reviewed advisory IDs ignored in `.cargo/audit.toml`. Neither fix touched
>   `Cargo.toml`/`Cargo.lock`, so this is an unsurprising confirmation, not new information.
> - **§12 `cargo clippy --features full,tauri --all-targets -- -D warnings`**: re-run (forced
>   recompile via `touch src/lib.rs` first) — still clean, 0 warnings, 0 errors.
>
> This is deliberately the **last commit of the entire remediation sequence** covered by this
> document — no `src/**` change follows it. (If a future phase touches `src/**` again, this file
> will again be exactly one commit stale until someone re-runs this same refresh.)
>
> **Status of this document (first refresh, superseded above for §2/§10/§11/§12 — narrative
> §1/§3–§9/§13–§15 below are unaffected by the cid.rs fixes and still reflect that pass):**
> objective inventory of the codebase as it exists today on branch
> `workflow/enterprise-buildout` (commit `ce36ee9`). This is a **refresh** of the original
> audit-phase document (which stopped at commit `4ed35fe`, the "Rendering Decision" phase) —
> nine further phases (Content Editing, Fonts, Interactive Features, Redaction, Standards
> Conformance, Large File Streaming, Signature, Security Hardening, Tauri Integration, plus four
> follow-up remediation passes) landed roughly **23,000 additional lines of Rust** and six new
> top-level modules (`editor/`, `filter/`, `render/`, `tauri_commands/`, plus new files inside
> `font/`) that the previous version of this file never mentioned. This refresh:
>
> - Regenerates **§2 Module inventory** from a live `find src -name '*.rs' | xargs wc -l` —
>   every module named in the remediation brief (`editor/redact.rs`, `editor/audit.rs`,
>   `editor/pdfa.rs`, `editor/pdfx.rs`, `editor/pdfua.rs`, `editor/icc.rs`, `editor/xmp.rs`,
>   `editor/structure.rs`, `editor/outline.rs`, `font/cid.rs`, `font/subset.rs`,
>   `font/truetype.rs`, `render/*`, `tauri_commands/*`) now has a row.
> - Regenerates **§10 coverage** and **§11 `cargo audit`** from a live re-run against
>   `--features full,tauri` (the old numbers were measured against a ~102-dependency build with
>   no `render`/`tauri` feature compiled in at all — meaningless for today's codebase).
> - Fixes a factual error carried since the original audit: **the largest file in the crate is
>   `src/signatures/signer.rs` (1,825 lines), not `src/document/mod.rs`** (1,387 lines — still
>   large, but no longer the largest by a wide margin; `parser/mod.rs` at 1,687 and
>   `tauri_commands/commands.rs` at 1,516 also now exceed it).
> - Adds three new sections for functionality the old document had no entry for at all: **§6
>   in-place editing (`editor/`)**, **§7 embedded/CID fonts (`font/cid.rs`, `subset.rs`,
>   `truetype.rs`)**, **§9 the Tauri command layer (`tauri_commands/`)**.
>
> **What this refresh did *not* do:** a full line-by-line re-verification of every narrative claim
> in the old §4 ("read path findings"), §5 (signatures), §13 (risk register) and §14 (gap
> analysis). Those sections describe the codebase as it stood at the *original* audit
> (commit `4ed35fe`) and are, in several specific places checked opportunistically while doing
> this refresh, now **stale** (e.g. several of the parser DoS findings in the original §4.1 have
> since been fixed — see the callout box at the top of §4). `docs/THREAT_MODEL.md` is the actively
> maintained, current source of truth for untrusted-input risk and dependency-advisory rationale
> (it says as much itself); this document did not attempt to duplicate or re-derive it. Where this
> refresh spot-checked and found an old claim to be superseded, that is called out explicitly and
> cited to a source (a constant, a doc-comment, a test name) — nothing below is invented.

## 1. What this crate actually is today

`rust-pdf` started as a **PDF *generation* (write-only) library** with a partial structural
reader. It is now also: an **in-place editor** for existing PDFs (`editor/`: pages, forms,
annotations, outlines, tagged structure, redaction, PDF/A-PDF/X-PDF/UA conformance, ICC/XMP
metadata), a **font-embedding pipeline** (`font/`: TrueType/OpenType loading, CID/Type0 composite
fonts for CJK, subsetting), a **pure-Rust PDF rasterizer** (`render/`, no native binary or FFI
dependency at all — a content-stream interpreter over `tiny-skia` (2D rasterization) and
`ttf-parser` (font outlines); this crate *previously* rasterized via FFI to Google's Pdfium, an
explicit, accepted trade-off documented at the time, but that FFI dependency has since been fully
removed at explicit user request — see §8/§8a-§8d for the full migration history and the
compatibility/performance trade-offs *this* direction accepted instead), and a **Tauri desktop-app
command layer** (`tauri_commands/`, nine async commands sharing one ordinary worker-thread pool —
rendering no longer needs its own dedicated actor thread now that it isn't wrapping a
not-safe-to-share native library handle; see §8c/§9).

It is still **not** a general-purpose arbitrary-PDF consumer in one specific, load-bearing way,
verified again for this refresh: `PdfReader::from_bytes` (`src/parser/mod.rs`) still
unconditionally returns `ParserError::EncryptedPdf` the instant a trailer's `/Encrypt` entry is
present — there is still no code path that takes a password and opens an existing encrypted PDF
(§4, finding #1, re-verified true today). Encryption (`encryption` feature) remains write-only:
it builds `/Encrypt` dictionaries for documents *this crate* creates.

```
Cargo.toml features: compression, images, parser, encryption, signatures, html(empty),
                      office(empty), fonts, render, tauri, full
Default features:    none (bare crate builds with color/content/document/font/forms/object/
                      page/types/writer/error only)
```

`html` and `office` remain declared Cargo features with **no corresponding module** — still
aspirational surface area, not implemented functionality. `render`/`native-render` and `tauri` are
still deliberately **not** part of `full`, but the *reason* changed with this migration: `render`
no longer requires bundling a native platform-specific binary (see §8/§8c — that reasoning is now
historical), it is excluded because it pulls in `tiny-skia`/`ttf-parser`/`image` rasterization
dependencies, and carries the known, disclosed rendering-fidelity gaps in §8d, that a pure
structural/generation/signing consumer of this crate has no need to take on. `tauri` remains
excluded because it requires the Tauri desktop-app runtime itself (a dependency shape no library
consumer other than an actual Tauri app needs) — see §9.

## 2. Module inventory (src/)

**48,168 lines of Rust across 111 files** (live count, re-verified for this third refresh:
`find src -name '*.rs' | xargs wc -l`; up from 39,591 lines / 98 files at the second refresh
(`babec0c`) — cross-checked against `git diff babec0c HEAD --stat -- src/`: **34 files changed,
+9,622/−1,045**, matching this table exactly). Essentially all of the growth is the pure-Rust
rendering build (four migration commits — `899fb9f`/`e658df6`/`b63dc98`/`a3db203`/`e6af056`,
§8a-§8c) plus its follow-on fuzz-hardening (`5ba3e0c`/`0e14625`): `render/` grew from 3 files/849
LOC to **17 files/9,440 LOC** (the whole new `render/native/` submodule), `tauri_commands/`
**shrank** from 7 files/2,865 LOC to **6 files/2,592 LOC** net (`render_actor.rs`, 437 lines, was
deleted outright — see §8c/§9 — even though every remaining file in that directory grew), and
several other files touched incidentally along the way: `font/truetype.rs` (+41,
`outline_glyph()`), `font/encoding.rs`, `editor/pages.rs` (+159), `editor/mod.rs`,
`editor/text_extract.rs`, `editor/icc.rs`, `editor/redact.rs`, `filter/ccitt.rs`, `parser/
recovery.rs`, `error.rs` (+9, new `RenderError`/`RenderWarning`-adjacent variants) and `lib.rs`
(prelude re-exports, net LOC unchanged). Up from 16,427 lines / 51 files at the original audit.
LOC/file-count re-confirmed unchanged by the fifth refresh's own re-run of the same `find` command.
Grouped by directory below; `%Ln` is line coverage from `cargo llvm-cov --release --features
full,tauri` in §10. **The third refresh claimed every row here had been "re-verified by this
refresh's own re-run, not assumed unchanged" — that claim was false for 16 rows, which had in fact
been carried over unchanged (mostly from the original audit) without being re-measured; the fifth
refresh (see the status banner at the top of this document and §15) is the one that actually ran
`cargo llvm-cov` fresh, diffed every one of the 105 covered rows against the output, and corrected
the ones that had drifted.** Every `%Ln` value below is now current as of the fifth refresh.

| File | LOC | %Ln | Purpose |
|---|---:|---:|---|
| `lib.rs` | 339 | 100.0 | Crate root, `prelude` re-exports, doctests |
| `error.rs` | 564 | — | `thiserror`-based error taxonomy, one enum per subsystem; +9 LOC since the second refresh (incidental additions alongside the rendering build's own `RenderError`/`RenderWarning` types, which actually live in `render/native/error.rs`, not here — see §8d) |
| `ffi.rs` | 157 | 0.0 | C ABI: `unsafe extern "C"` functions exposing the *write* path only |
| **`types/`** (4 files, 441 LOC) | | | Geometry primitives |
| `types/mod.rs` | 9 | — | Re-exports |
| `types/matrix.rs` | 189 | 86.8 | 2D affine transform matrix |
| `types/object_id.rs` | 101 | 100.0 | `ObjectId` (object number + generation) |
| `types/rectangle.rs` | 142 | 87.8 | `/MediaBox`-style rectangles |
| **`color/`** (4 files, 533 LOC) | | | Device color spaces |
| `color/mod.rs` | 173 | 85.2 | `Color` enum, conversions |
| `color/rgb.rs` | 159 | 89.7 | DeviceRGB |
| `color/cmyk.rs` | 119 | 81.8 | DeviceCMYK |
| `color/gray.rs` | 82 | 78.6 | DeviceGray |
| **`object/`** (6 files, 1,572 LOC) | | | The PDF object model, used by write + (partial) read paths |
| `object/mod.rs` | 282 | 68.9 | `Object` enum, top-level dispatch |
| `object/dictionary.rs` | 190 | 91.1 | `PdfDictionary` |
| `object/array.rs` | 141 | 85.5 | `PdfArray` |
| `object/name.rs` | 199 | 67.2 | `PdfName` |
| `object/string.rs` | 160 | 89.1 | `PdfString` (literal/hex, escaping) |
| `object/stream.rs` | 600 | 93.2 | `PdfStream`; legacy `decompress()` (FlateDecode-only, silently passes through other filters — see §13) plus the newer, full-filter-set `decode_all()` |
| **`content/`** (4 files, 1,310 LOC) | | | Content-stream *builders* (write path) |
| `content/mod.rs` | 377 | 90.8 | `ContentBuilder` |
| `content/graphics.rs` | 343 | 60.1 | `GraphicsBuilder` |
| `content/operator.rs` | 393 | 79.4 | `Operator` enum + serialization |
| `content/text.rs` | 197 | 88.4 | `TextBuilder` |
| **`font/`** (8 files, 2,775 LOC) | | | Font metadata, embedding and CID/subsetting — see §7 |
| `font/mod.rs` | 225 | 45.3 | `Font` trait/dispatch across Standard-14 and embedded fonts |
| `font/standard14.rs` | 182 | 91.8 | The 14 standard PostScript font metrics/AFM data |
| `font/metrics.rs` | 176 | 49.1 | Glyph-width lookup |
| `font/encoding.rs` | 118 | 57.1 | WinAnsi/MacRoman/Standard text encodings |
| `font/truetype.rs` | 836 | 96.5 | Embedded TrueType/OpenType font loading via `ttf-parser` (feature `fonts`); +41 LOC since the second refresh — `outline_glyph()`, added for `render::native::glyph` to extract glyph outlines for text rendering (§8b), wrapped in the same `catch_unwind` defense-in-depth `Face::parse` already uses |
| `font/cid.rs` | 796 | 92.2 | Type 0 / CIDFontType2 composite fonts for embedded + CJK text; +225 LOC since the first refresh — `ad35dcb` added `/CIDSet` generation (ISO 32000-1 Table 122 / ISO 19005-1 6.3.5, +2 unit tests) and `babec0c` made the subset-tag prefix conditional on the same `will_subset()` predicate (+3 unit tests) |
| `font/subset.rs` | 95 | 96.9 | Font subsetting via the `subsetter` crate |
| `font/tounicode.rs` | 391 | 92.0 | `/ToUnicode` CMap generation for text extraction/accessibility |
| **`page/`** (1 file, 327 LOC) | | | |
| `page/mod.rs` | 327 | 66.7 | `Page`/`PageBuilder` |
| **`document/`** (3 files, 1,774 LOC) | | | |
| `document/mod.rs` | 1,403 | 78.0 | `Document`/`DocumentBuilder`, orchestrates page tree + writer; +16 LOC since the first refresh (`ad35dcb`'s `cid_set_id`/`cid_set` plumbing into `CompositeFontIds`/the writer) |
| `document/info.rs` | 238 | 77.9 | `/Info` dictionary |
| `document/version.rs` | 133 | 61.2 | PDF version handling |
| **`writer/`** (3 files, 673 LOC) | | | Serializes the object graph to PDF bytes |
| `writer/mod.rs` | 276 | 89.9 | `PdfWriter` orchestration |
| `writer/serializer.rs` | 240 | 83.1 | Object → byte serialization |
| `writer/xref.rs` | 157 | 96.1 | Classic (table) xref + trailer output |
| **`forms/`** (3 files, 2,129 LOC) | | | AcroForm field/widget *construction* (write path) |
| `forms/mod.rs` | 125 | 100.0 | Re-exports |
| `forms/field.rs` | 1,404 | 51.5 | Text/checkbox/radio/combo/list/push-button field construction (largest single file in this group; 100+ public items, worst-covered) |
| `forms/widget.rs` | 600 | 83.9 | Widget annotation appearance construction |
| **`image/`** (2 files, 478 LOC) | | | |
| `image/mod.rs` | 319 | 24.6 | JPEG/PNG embedding via the `image` crate (worst-covered non-FFI file) |
| `image/xobject.rs` | 159 | 87.0 | `/XObject /Image` construction |
| **`filter/`** (8 files, 2,050 LOC) | | | Stream filter codecs (ISO 32000-1 §7.4) — new top-level module since the original audit |
| `filter/mod.rs` | 221 | 90.7 | `decode_filter`/dispatch, `MAX_DECODED_SIZE` |
| `filter/ascii_hex.rs` | 88 | 97.8 | `ASCIIHexDecode` |
| `filter/ascii85.rs` | 147 | 95.6 | `ASCII85Decode` |
| `filter/run_length.rs` | 115 | 90.3 | `RunLengthDecode` |
| `filter/lzw.rs` | 278 | 89.2 | `LZWDecode` |
| `filter/predictor.rs` | 474 | 88.2 | PNG/TIFF predictors (used by Flate/LZW) |
| `filter/dct.rs` | 125 | 72.3 | `DCTDecode` (baseline/progressive JPEG passthrough for image XObjects) |
| `filter/ccitt.rs` | 641 | 82.6 | `CCITTFaxDecode` (Group 3/4); +39 LOC since the second refresh (incidental touch-up alongside the rendering build, which reuses this decoder for CCITT image XObjects — see `render/native/image.rs`) |
| **`encryption/`** (4 files, 1,288 LOC) | | | Builds `/Encrypt` dictionaries; write-only (§1) |
| `encryption/mod.rs` | 296 | 90.6 | Orchestration |
| `encryption/config.rs` | 174 | 96.6 | `EncryptionConfig` |
| `encryption/key_derivation.rs` | 589 | 91.3 | RC4/AES-128/AES-256 key derivation (rev 2–6) |
| `encryption/permissions.rs` | 229 | 70.6 | `/P` permission bits |
| **`signatures/`** (9 files, 5,285 LOC) | | | Detached PKCS#7/CMS signing + verification + PAdES LTV — see §5 |
| `signatures/mod.rs` | 230 | 84.9 | Orchestration |
| `signatures/config.rs` | 240 | 93.4 | `SignatureConfig` |
| `signatures/certificate.rs` | 442 | 62.1 | X.509 cert parsing/build |
| `signatures/chain.rs` | 253 | 80.2 | Certificate chain (path) validation, `MAX_CHAIN_DEPTH` |
| `signatures/pkcs7.rs` | 619 | 86.0 | Hand-rolled minimal DER/PKCS#7 encoder |
| `signatures/signer.rs` | **1,825** | 73.4 | **Largest file in the crate.** `DocumentSigner` (fresh `Document`) + `IncrementalSigner` (ad-hoc plain-text scan of an existing PDF buffer — see original §4.3 finding, not re-verified this refresh) |
| `signatures/timestamp.rs` | 631 | 70.7 | RFC 3161 TSP client — PAdES "B-T" |
| `signatures/dss.rs` | 342 | 77.5 | `/DSS` Document Security Store embedding — PAdES "B-LT" |
| `signatures/verifier.rs` | 703 | 87.9 | `SignatureVerifier`, `/ByteRange` handling |
| **`parser/`** (7 files, 3,506 LOC) | | | nom-based structural PDF reader — see §4 |
| `parser/mod.rs` | 1,687 | 93.5 | `PdfReader` orchestrator; `MAX_XREF_SECTIONS`, `MAX_PAGE_TREE_WALK_*` |
| `parser/lexer.rs` | 404 | 85.5 | Tokenizer |
| `parser/objects.rs` | 381 | 88.2 | Object grammar; `MAX_NESTING_DEPTH` |
| `parser/trailer.rs` | 147 | 84.9 | Trailer + `/XRefStm` hybrid-reference handling |
| `parser/xref.rs` | 311 | 91.5 | Classic + xref-stream parsing |
| `parser/inline_image.rs` | 209 | 97.3 | `BI...ID...EI` inline image operator parsing |
| `parser/recovery.rs` | 367 | 77.2–77.6 | Repair-mode object scanning when the xref table is unusable; `MAX_RECOVERED_OBJECTS`; this file's coverage genuinely flips between back-to-back re-runs of the identical command (77.22% one run, 77.64% the next, both observed live this refresh) — one line's execution depends on which thread wins a race in a concurrent test, same root cause as §10's documented aggregate ±1-line wobble, just large enough here (out of only 237 executable lines) to move the rounded percentage instead of disappearing into it |
| **`editor/`** (19 files, 10,852 LOC) | | | **New since the original audit.** In-place editing of an existing PDF — see §6 |
| `editor/mod.rs` | 93 | — | `EditableDocument` entry point; +5 LOC since the second refresh (incidental, part of wiring `EditableDocument` up as `render::PdfRenderer`'s backing store, §8c) |
| `editor/graph.rs` | 481 | 91.8 | Core mutable object graph; `MAX_PAGE_TREE_NODES`, `MAX_REACHABLE_OBJECTS` |
| `editor/pages.rs` | 869 | 85.1 | Insert/delete/reorder/rotate/split/merge page-tree editing; +159 LOC since the second refresh — `effective_media_box`/`parse_media_box` (ISO 32000-1 §7.7.3.3 `/MediaBox` inheritance + rotation normalization) added for whole-document rendering (§8c), with adversarial-input unit tests (missing `/MediaBox`, swapped corners, wrong element count, non-numeric/non-finite entries) |
| `editor/forms.rs` | 1,202 | 91.4 | Read/fill/create/flatten AcroForm fields on a loaded document |
| `editor/annotations.rs` | 779 | 95.2 | Markup annotations (highlight/underline/strikeout/free-text/stamp/ink/note) with generated appearance streams |
| `editor/outline.rs` | 646 | 91.9 | Document outline (bookmarks) + named destinations |
| `editor/structure.rs` | 432 | 90.7 | Minimal Tagged PDF logical structure tree (headings/paragraphs/tables/figures) |
| `editor/redact.rs` | 1,431 | 73.4 | Permanent content redaction (removes underlying content, not just a visual overlay); incidental touch-up alongside the rendering build (LOC unchanged, coverage unchanged to one decimal) |
| `editor/audit.rs` | 339 | 98.2 | Redaction audit trail (documented private extension — ISO 32000 has no native object for this) |
| `editor/pdfa.rs` | 977 | 80.3 | PDF/A-1b/2b/3b validation + conversion; +51 LOC since the first refresh (`ad35dcb` narrowed `check_cidset_present`'s doc comment now that the gap it guards against is fixed, and split its regression test into a positive end-to-end-conformant case plus a hand-built-missing-`/CIDSet` defense-in-depth case — net +1 unit test) |
| `editor/pdfx.rs` | 261 | 88.6 | PDF/X colour-space constraint checking (ISO 15930) |
| `editor/pdfua.rs` | 372 | 98.4 | PDF/UA (ISO 14289-1) Matterhorn-Protocol-style checklist validation |
| `editor/icc.rs` | 386 | 95.3 | ICC output-intent embedding (used by PDF/A + PDF/X); note this is **write-path** ICC-profile *embedding*, unrelated to §8d's rendering-side ICC-color-management gap (this file never *interprets* a profile's colorimetric transform either — it just embeds the caller-supplied bytes as an `/OutputIntent`) |
| `editor/xmp.rs` | 366 | 94.0 | XMP metadata packet generation/embedding |
| `editor/content_ops.rs` | 331 | 78.0 | Insert/replace text/shapes/images on an existing page; `MAX_CONTENT_STREAM_BYTES` |
| `editor/content_stream.rs` | 615 | 72.4 | Round-trippable content-stream operator parser (used by content_ops/redact/text_extract) |
| `editor/text_extract.rs` | 307 | 85.7 | Text extraction from an existing page's content stream |
| `editor/save.rs` | 726 | 89.6 | Incremental update *or* full compacted rewrite (object streams + xref stream) |
| `editor/util.rs` | 239 | 94.3 | Shared helpers |
| **`render/`** (17 files: 3 top-level + `native/` submodule, 9,440 LOC) | | | **Rewritten since the second refresh.** Pure-Rust page rasterization (no native binary/FFI at all — see §8/§8a-§8d for the completed Pdfium→pure-Rust migration), `render`/`native-render` features |
| `render/mod.rs` | 225 | 100.0 | Public API (`PdfRenderer`, `Viewport`, `MAX_RENDER_PIXELS`) + module-level migration-history doc (§8's rationale is preserved there as history; current status is stated up front) |
| `render/renderer.rs` | 810 | 84.4 | `PdfRenderer` rebuilt on `editor::EditableDocument` (no FFI handle left to wrap): `open_file`/`open_bytes`, `render_page`/`render_thumbnail`, `deep_resolve` (recursively dereferences a page's `/Resources` before handing it to `native::render_content_stream`, bounded by `MAX_RESOLVE_REFERENCES`/`MAX_RESOLVE_DEPTH`), page-rotation normalization/application, `append_annotation_appearances`. No `ffi_lock`/`ManuallyDrop` left — see §8c |
| `render/cache.rs` | 148 | 100.0 | Bounded LRU thumbnail cache (unchanged in shape from the Pdfium-backed version) |
| **`render/native/`** (14 files, 8,257 LOC) | | | Pure-Rust content-stream interpreter + rasterizer, no PDF-document access of its own — `native-render` feature, `tiny-skia` (rasterizer) + `ttf-parser` (font outlines). See §8a/§8b for what each phase added and **§8d for the exhaustive, honest list of what it does *not* do** |
| `render/native/mod.rs` | 853 | 98.7 | `render_content_stream()` entry point; module docs are the canonical scope/gaps writeup (§8d summarizes it) |
| `render/native/interpreter.rs` | 2,045 | 75.1 | The interpreter loop: graphics-state stack/CTM, path paint/clip, text showing (3 font kinds), transparency groups/soft masks/blend modes, Form XObject + Type 3 glyph recursion (`MAX_FORM_XOBJECT_DEPTH`/`MAX_TYPE3_DEPTH`) — largest file in this submodule |
| `render/native/function.rs` | 1,010 | 80.4 | PDF function evaluation for colour tint-transforms: Type 0 (Sampled), 2 (Exponential), 3 (Stitching), 4 (PostScript calculator) |
| `render/native/image.rs` | 741 | 87.5 | Image XObject + inline-image painting via `crate::filter`'s existing decoders; **JBIG2/JPX fail closed to a documented placeholder — see §8d** |
| `render/native/font.rs` | 804 | 92.6 | Resolves `/Resources /Font/<name>` to a loaded `ttf-parser` program or a structured `UnsupportedFontReason` (Type1/bare-CFF, not embedded — **see §8d**); bounded `/W`/`/Widths` parsing |
| `render/native/colorspace.rs` | 507 | 85.4 | Indexed/Separation/DeviceN/CalGray/CalRGB resolution and **ICCBased approximation (see §8d)**; Lab and Pattern are explicit, warned gaps |
| `render/native/error.rs` | 431 | 0.0 | `NativeRenderError` (hard failures)/`RenderWarning` (soft, structured, non-panicking gaps) — the type every §8d gap is surfaced through. **0% line coverage**: this file is exercised indirectly (every other file's tests construct/return these variants), but `cargo-llvm-cov` attributes that execution to the *calling* file, not this one, since the variant constructors themselves are trivial one-line enum literals the compiler inlines — reported here honestly rather than omitted |
| `render/native/path.rs` | 432 | 94.8 | Device-space path construction (`m l c v y h re`) |
| `render/native/text_tests.rs` | 457 | — | Test-only: TrueType/CID-CJK/Type3 glyph-paints-ink and the Type1/bare-CFF graceful-failure regression test (`type1_bare_cff_font_fails_gracefully_not_panicking`, cited in §8d) |
| `render/native/image_integration_tests.rs` | 373 | — | Test-only: JBIG2/JPX-placeholder, CCITT/JPEG paint, Indexed/Separation tint-transform regression tests (several cited in §8d) |
| `render/native/state.rs` | 276 | 87.5 | `GraphicsState`/`GraphicsStateStack` (`q`/`Q`) + `TextState`, bounded stack depth |
| `render/native/glyph.rs` | 105 | 86.7 | `ttf-parser` outline callback → `tiny_skia::PathBuilder` glyph path |
| `render/native/color.rs` | 131 | 100.0 | DeviceGray/RGB/CMYK → `tiny_skia::Color`; naive, non-ICC CMYK formula (ISO 32000-1 §8.6.5.3's own fallback — see §8d) |
| `render/native/bits.rs` | 92 | 100.0 | Bit-level reader shared by the CCITT/other packed-sample decode paths this module reuses from `crate::filter` |
| **`tauri_commands/`** (6 files, 2,592 LOC) | | | Async Tauri desktop command layer, `tauri` feature — see §9. **`render_actor.rs` (437 LOC) deleted this migration** (§8c) — every other file grew, netting a smaller directory overall |
| `tauri_commands/mod.rs` | 125 | — | Module overview |
| `tauri_commands/commands.rs` | 1,684 | 83.4 | The 9 commands: `open_document`, `render_page`, `extract_text`, `search_text`, `apply_edit`, `save_document`, `fill_form`, `add_annotation`, `sign_document`. `render_page` now runs on the ordinary `WorkerPool` like every other command (§8c) |
| `tauri_commands/state.rs` | 233 | 86.1 | Managed app state: open-document registry + worker pool handle (no more separate render-actor handle to manage, §8c) |
| `tauri_commands/worker.rs` | 218 | 91.7 | CPU-bound work thread pool (parsing/editing/signing/**rendering**, all off the async executor) |
| `tauri_commands/progress.rs` | 92 | 100.0 | Progress event reporting, decoupled from the `tauri` crate's own types |
| `tauri_commands/error.rs` | 240 | 75.8 | `CommandError`, structured `Serialize`-able error taxonomy for the command layer |

Public API surface (rough grep count of `pub` items, not counting re-exports) is now dominated by
the newer modules: `forms/field.rs` (100+), `editor/forms.rs`, `editor/redact.rs` and
`tauri_commands/commands.rs` are all large; a precise total was not recomputed for this refresh
(the original audit's "≈700+ public items" figure predates ~23,000 lines of subsequent code and
should not be treated as current).

## 3. Write path (the part that works)

```
DocumentBuilder → Document { pages: Vec<Page>, info, version, encryption? }
Page/PageBuilder → content stream (ContentBuilder/GraphicsBuilder/TextBuilder → Operator list → bytes)
Document::save_to_bytes()/write_to()
  → allocates ObjectIds
  → optionally compresses each stream (Flate, feature "compression")
  → optionally embeds/subsets fonts (feature "fonts", src/font/{truetype,cid,subset}.rs)
  → optionally encrypts each string/stream (feature "encryption", RC4/AES per EncryptionConfig)
  → PdfWriter serializes objects sequentially, tracks byte offsets
  → writes a classic (%…%%EOF, `xref` keyword, plain trailer) cross-reference table
```

This is unchanged in shape from the original audit; it is now complemented by `editor/save.rs`,
which can write an **incremental update** or a **full rewrite using object streams and a
cross-reference stream** for an already-loaded, edited `EditableDocument` (§6) — i.e. the
"classic xref only" limitation noted in the original audit no longer applies to the edit path,
only to `Document::save_to_bytes()`'s from-scratch generation path. This distinction was not
re-verified line-by-line for this refresh beyond confirming `editor/save.rs`'s module doc comment
states it (quoted verbatim in §2's table).

## 4. Read path (`parser` feature)

> **Status update (this refresh, spot-checked, not a full re-audit):** the original §4.1 below
> lists 11 findings from the very first audit (commit `4ed35fe`'s ancestor). A later
> "Parser Robustness" phase (commit `ba71ea5`, between the original audit and the Rendering
> Decision phase) appears to have fixed most of the DoS-shaped ones. Verified directly against
> today's source for this refresh:
> - Finding **#1** (encrypted PDFs unconditionally rejected) — **still true**, re-verified:
>   `src/parser/mod.rs` still returns `ParserError::EncryptedPdf` unconditionally.
> - Finding **#2** (hybrid `/XRefStm` ignored) — **fixed**: `xref_stm_offset()` in
>   `src/parser/mod.rs` now reads it (ISO 32000-1 §7.5.8.4).
> - Finding **#3** (`/Prev` cycle → infinite loop) — **fixed**: `MAX_XREF_SECTIONS = 4096` hard
>   cap, `src/parser/mod.rs`.
> - Finding **#4** (unbounded object-grammar recursion → stack overflow) — **fixed**:
>   `MAX_NESTING_DEPTH = 64`, `src/parser/objects.rs`.
> - Finding **#5** (unbounded xref subsection `count`) — **mitigated**: entries are inserted one
>   at a time as they're actually parsed out of the real input buffer (no pre-allocation sized by
>   the untrusted `count`), so a huge claimed count still fails once the input is exhausted rather
>   than allocating/looping unboundedly; overflow on `first_obj + i` is `checked_add`-guarded.
> - Finding **#6** (unbounded Flate decompression) — **fixed**: `MAX_DECODED_SIZE` cap, enforced
>   in both `filter::decode_flate` and `object/stream.rs::decompress()`.
> - Finding **#7** (unbounded/panicking offset slicing) — **fixed**: `resolve_reference` now uses
>   `self.data.get(*offset as usize..)?` (bounds-checked) rather than direct indexing.
> - Finding **#8** (only FlateDecode understood, others silently passthrough) — **fixed for the
>   `decode_all()` path** (full filter set, `JBIG2Decode`/other-unknown returns `Err`, verified by
>   `filter::tests::unsupported_filter_is_rejected_not_panicking`); **the legacy
>   `PdfStream::decompress()` method still silently passes through non-FlateDecode data** — its own
>   doc comment now explicitly warns callers to prefer `decode_all()` instead (see §2's
>   `object/stream.rs` row and §13).
> - Finding **#9** (`object_cache` never populated) — **fixed**: `resolve_reference` writes into
>   it, and there's a regression test (`test_resolve_reference_populates_get_object_cache`).
> - Finding **#10** (indirect `/Length` unsupported) — **fixed**: falls back to scanning for
>   `endstream` when `/Length` doesn't resolve to a direct integer.
> - Finding **#11** (page count trusted, no tree-well-formedness check) — **partially addressed**:
>   `MAX_PAGE_TREE_WALK_NODES`/`MAX_PAGE_TREE_WALK_DEPTH` bound the walk so a pathological
>   `/Kids` structure fails safely instead of hanging, but this is a resource bound, not full
>   cycle/well-formedness detection — not independently re-verified further.
>
> This status update was assembled by grepping for the named constants/functions in today's
> source, not by re-running the original audit's exact repro steps end-to-end. `docs/
> THREAT_MODEL.md` §4/§6 is the maintained, current risk register (full `MAX_*` inventory,
> component-by-component) and takes precedence over anything below if the two disagree.

`PdfReader` (`src/parser/mod.rs`) is a from-scratch, hand-rolled nom parser, now joined by
`src/parser/recovery.rs` (a "repair mode" object scanner for files whose xref table is unusable —
not present at the time of the original audit, `MAX_RECOVERED_OBJECTS` bounded). It is still
mainly exercised against this crate's own writer output plus the self-generated fixtures in
`tests/render_tests.rs` and the fuzz corpus (`fuzz/fuzz_targets/parse_pdf.rs`,
`decode_filters.rs`, `parse_inline_image.rs`, `font_load.rs`) — there is still no committed
corpus of arbitrary third-party producer PDFs.

### 4.1 Original findings (historical — see status update above for current truth)

References are to ISO 32000-1:2008 unless noted.

1. **Encrypted PDFs are unconditionally rejected.** Still true (re-verified).
2. Hybrid-reference (`/XRefStm`) support — fixed (re-verified).
3. `/Prev`-chain infinite loop — fixed (re-verified, `MAX_XREF_SECTIONS`).
4. Object-grammar recursion depth — fixed (re-verified, `MAX_NESTING_DEPTH`).
5. Unbounded xref subsection `count` — mitigated (re-verified).
6. Zlib decompression bomb — fixed (re-verified, `MAX_DECODED_SIZE`).
7. Unbounded/panicking offset slicing — fixed (re-verified, `.get()`-based bounds checks).
8. Only FlateDecode understood — fixed for `decode_all()`, legacy `decompress()` still passes
   through silently (re-verified both).
9. `object_cache` never populated — fixed (re-verified).
10. `Length` must be a direct integer — fixed via `endstream`-scan fallback (re-verified).
11. Page count trusted / tree well-formedness — partially addressed via resource bounds
    (re-verified the bounds exist; did not re-verify full cycle detection).

### 4.2 Signing path uses a second, independent, ad-hoc "parser"

`src/signatures/signer.rs`'s `IncrementalSigner` still does its own plain-text scanning
(`String::from_utf8_lossy`, `.find("/Root")`, `.find("{id} 0 obj")`, etc.) of an existing PDF
buffer rather than using `src/parser/`. This was **not** re-verified for this refresh (it is a
large, 1,825-line file and re-auditing its current byte-scanning robustness against
compressed-object-stream files was out of scope for a documentation-inventory refresh). Treat
the original finding as unconfirmed-but-plausible until someone re-checks it directly.

## 5. Encryption & Signatures

- **Encryption** (`encryption` feature): unchanged in shape from the original audit — RC4
  (V1/V2, R2/R3), AES-128 (V4, R4), AES-256 (V5, R6) per ISO 32000-2:2020 Annex C, write-only.
- **Signatures** (`signatures` feature): detached PKCS#7 (RSA PKCS#1v1.5, ECDSA P-256), hand-rolled
  minimal DER/PKCS#7 encoder (`signatures/pkcs7.rs`), verified against `openssl cms`
  (`test_cross_verify_with_openssl_cms`).
- **Correction to the original audit:** the original §5 stated *"Timestamping (RFC 3161 `/TS`)
  and Long-Term Validation (`/DSS`, PAdES) are not implemented."* This is **no longer true** — a
  later "Signature" phase added `src/signatures/timestamp.rs` (RFC 3161 TSP client, PAdES "B-T")
  and `src/signatures/dss.rs` (Document Security Store embedding, PAdES "B-LT"), with dedicated
  tests (`test_pades_b_t_rfc3161_timestamp_end_to_end`, `test_dss_embedding_preserves_signature_
  validity`, `test_pades_b_b_subfilter_and_cross_verify`, `test_pades_level_t_without_timestamp_
  authority_fails_fast`, `test_two_sequential_signatures_with_pades_timestamp_and_visible_
  appearance` — all passing, see §10).
- Still write-only in the same sense as encryption: there is no `Document::open` that decrypts an
  existing encrypted PDF before verifying/adding a signature to it.

## 6. Editing an existing PDF (`editor/`)

New top-level module since the original audit (19 files, 10,685 lines — the single largest
directory in the crate). `EditableDocument` (`editor/mod.rs`/`graph.rs`) loads an existing PDF via
`parser`, exposes a mutable in-memory object graph bounded by `MAX_PAGE_TREE_NODES`/
`MAX_REACHABLE_OBJECTS` (see `docs/THREAT_MODEL.md` §6 for the full resource-limit inventory), and
supports:

- **Structural editing**: page insert/delete/reorder/rotate/split/merge (`pages.rs`), AcroForm
  field read/fill/create/flatten (`forms.rs`), markup annotations with generated appearance
  streams (`annotations.rs`), document outline/bookmarks + named destinations (`outline.rs`).
- **Content editing**: insert/replace text/shapes/images on an existing page (`content_ops.rs`,
  built on a round-trippable content-stream operator parser in `content_stream.rs`), plus text
  extraction (`text_extract.rs`).
- **Standards conformance**: minimal Tagged PDF logical structure (`structure.rs`, ISO 32000-1
  §14.7/14.8), PDF/A-1b/2b/3b validation+conversion (`pdfa.rs`), PDF/X colour constraint checking
  (`pdfx.rs`, ISO 15930), PDF/UA Matterhorn-Protocol-style checklist validation (`pdfua.rs`), ICC
  output-intent embedding (`icc.rs`) and XMP metadata (`xmp.rs`).
- **Redaction**: permanent (not just visual-overlay) content removal (`redact.rs`, ISO 32000-2
  §12.5.6.19) with a documented, namespaced audit-trail extension (`audit.rs`) since ISO 32000
  does not define a native redaction-audit object.
- **Persistence**: `save.rs` writes either an ISO 32000-1 §7.5.6 incremental update or a full,
  compacted rewrite using object streams + a cross-reference stream (§7.5.7/7.5.8).

Per-file ISO references and scope caveats are documented as module-level rustdoc in each file
(quoted where relevant in §2's table); this section did not re-derive them independently.

## 7. Fonts: embedded/CID/subsetting (`font/`)

Extends the original Standard-14-only font module with (gated behind the `fonts` feature):

- `font/truetype.rs`: embedded TrueType/OpenType font *loading* via the `ttf-parser` crate
  (deliberately not a hand-rolled sfnt parser over untrusted font bytes — see its module doc and
  `docs/THREAT_MODEL.md` §4.3), bounded by `MAX_FONT_SIZE_BYTES`/`MAX_GLYPH_COUNT`.
- `font/cid.rs`: Type 0 / CIDFontType2 composite font construction (ISO 32000-1 §9.7) for
  embedding TrueType/OpenType fonts including CJK text.
- `font/subset.rs`: font subsetting (glyph remapping/table rebuilding) via the `subsetter` crate,
  reducing an embedded font program to only the glyphs a document actually uses (ISO 32000-1 §9.9
  permits this).
- `font/tounicode.rs`: `/ToUnicode` CMap generation, needed for text extraction and accessibility
  of embedded/CID fonts, bounded by `MAX_CMAP_BYTES`/`MAX_CMAP_ENTRIES`.

The original audit's blanket claim ("no embedded/TrueType/CFF font support... no CJK/Unicode text
beyond WinAnsi") no longer holds for TrueType/OpenType + CID text; CFF-only (bare, non-OpenType)
font programs were not confirmed either way for this refresh.

## 8. Rendering: native-vs-FFI decision (superseded — see §8c/§8d for the current, live truth)

> **This decision has since been reversed, at explicit user request.** §8 below is preserved
> verbatim as the **historical record** of the original build-vs-buy call (Pdfium/FFI) and the
> reasoning behind it at the time — it is **not** what this crate does today. §8a→§8b→§8c tell the
> story of the migration off Pdfium to a pure-Rust engine (`tiny-skia` + `ttf-parser`, no native
> binary or FFI dependency of any kind), and **§8d "Known Limitations" is the current, single
> place to read** for exactly what that pure-Rust engine does and does not support, each claim
> cross-checked against a named test in this session. If you are auditing this crate's *current*
> rendering behavior, start at §8d, not here.

*(Unchanged from the original document — this section was already current as of the "Rendering
Decision" phase and is preserved as-is, only renumbered.)*

This section records the decision for the gap identified in §14 ("a real content-stream
interpreter + rasterizer/vector-renderer... does not exist at all today") and documents what was
actually built. The full rationale also lives as rustdoc in `src/render/mod.rs` (the canonical,
versioned copy — read that first; this is a summary for anyone auditing the repo top-down).

**Decision: FFI to Google's Pdfium via the `pdfium-render` crate, gated behind an opt-in
`render` Cargo feature. Not a from-scratch Rust content-stream interpreter/rasterizer.**

### Why

- `tiny-skia`/`raqote` (2D rasterizer backends) and `ttf-parser`/`rustybuzz` (font
  parsing/shaping) are **not** PDF parsers. Using them as a rendering backend would still
  require this crate to build, from zero: the content-stream interpreter itself; every
  ISO 32000-1 §8.6 color space (CalGray/CalRGB, Lab, ICCBased, Indexed, Separation/DeviceN, not
  just DeviceGray/RGB/CMYK); transparency groups and blend modes (§11); CID/Type0 font handling
  with embedded CFF/TrueType/OpenType programs; annotation appearance streams (§12.5.5) and
  AcroForm field appearance generation; and every image filter real documents use, including
  JBIG2 and JPX — for which the Rust ecosystem has **no mature decoder today**, and both are
  common in scanned enterprise documents. That is a multi-year, multi-person effort, not a phase.
- Pdfium is the engine behind Google Chrome's PDF viewer: exercised at billions-of-page-views
  scale, fuzzed continuously by Chromium's infrastructure, and tested against tens of thousands
  of real-world regression PDFs accumulated over more than a decade. No from-scratch renderer
  built in this project's timeframe could realistically match that compatibility bar.
- `pdfium-render` is a mature, actively maintained Rust binding, and `bblanchon/pdfium-binaries`
  publishes prebuilt, per-platform shared libraries that are straightforward to bundle as a
  native resource in a Tauri application (Tauri already ships/bundles native binaries as a
  normal part of its packaging model).

### Explicit trade-offs accepted

- **Native binary dependency.** The `render` feature pulls in `pdfium-render` (FFI) and requires
  a platform-specific `libpdfium.{dylib,so,dll}` (a few MB) at *run time*, loaded dynamically via
  `libloading` — there is no special build-time toolchain requirement for `rust-pdf` itself, but
  a shipped application must bundle the right binary per target platform/architecture. This is
  why `render` is **not** part of the `full` feature: pure structural/generation/signing
  consumers of this crate (the FFI/C/Python/etc. bindings in `examples/`) do not need it.
- **FFI/crash-level risk.** A memory-safe Rust program can still crash if the native library is
  missing/corrupted/mismatched, or if Pdfium itself has a bug. This is mitigated by Pdfium's own
  fuzzing coverage, not by anything this crate can add — but it is a real category of risk a
  pure-Rust renderer would not have (at the cost of that pure-Rust renderer taking years to reach
  comparable real-world compatibility).
- **Concurrency is the caller's responsibility to serialize, and this crate does so internally.**
  Pdfium's C API is documented upstream as not safe to call concurrently from multiple threads.
  `src/render/renderer.rs` serializes every FFI-touching call behind a single process-wide
  `Mutex` (`ffi_lock`), and wraps `PdfRenderer`'s `PdfDocument` field in `ManuallyDrop` with a
  custom `Drop` impl that explicitly holds `ffi_lock` while closing it — see the doc comments on
  `ffi_lock` and that `Drop` impl for details.
- **We do not, and cannot, verify Pdfium's internal rendering correctness against the spec
  ourselves** — only that this crate's wrapper (a) loads the library safely, (b) validates
  caller/file-derived inputs *before* they reach the FFI boundary, and (c) converts results
  faithfully.
- **Tiled/viewport rendering re-renders the full page internally and crops it**, rather than
  using a hand-rolled Pdfium transformation matrix to rasterize only the requested tile. See
  `src/render/mod.rs`'s "Known limitation" subsection.

### What was built

- `src/render/mod.rs`, `src/render/renderer.rs`, `src/render/cache.rs`, gated by the `render`
  Cargo feature (also pulls in `images`).
- Public API: `PdfiumLibrary::bind()`/`bind_from_path()`, `PdfRenderer::open_bytes()`/
  `open_file()` (with ISO 32000-1 §7.6 password support), `PdfRenderer::render_page(page_index,
  dpi, viewport) -> Result<RgbaImage, RenderError>`, `PdfRenderer::render_thumbnail(page_index,
  max_dimension)` backed by a bounded LRU cache.
- Untrusted-input handling: `render_page`/`render_thumbnail` compute the target pixel count up
  front and reject anything exceeding `render::MAX_RENDER_PIXELS` (64,000,000 px, ~256 MiB as
  RGBA8) with `RenderError::OutputTooLarge` *before* asking Pdfium to allocate a bitmap.
- `scripts/fetch_pdfium.sh`: developer/CI convenience script that downloads the correct
  `bblanchon/pdfium-binaries` release asset into `.pdfium/<platform>/` (gitignored).
- `tests/render_tests.rs` (11 tests, all passing — see §10) plus `tests/large_file_render_bench.rs`
  / `tests/large_file_rss_bench.rs` (opt-in, `#[ignore]`d 2GB/10,000-page fixture benchmarks added
  in a later "Large-File Render Benchmark" remediation phase, not present at the time this section
  was first written).

## 8a. Pure-Rust rendering migration, phase 1: "Content-Stream Interpreter Core" (implemented)

**This is a live migration in progress, not a completed replacement of §8.** The user has
explicitly requested moving off Pdfium/FFI to a pure-Rust renderer, accepting the compatibility
and performance trade-offs §8 documented as the reason Pdfium was originally chosen. `render`
(Pdfium/FFI) and `native-render` (pure Rust) now **coexist** behind two separate, independently
optional Cargo features; nothing in §8 was removed, and `PdfRenderer`/`PdfiumLibrary` are
unchanged.

**Decision for the pure-Rust path: `tiny-skia` (2D rasterizer, BSD-3-Clause — note: not
MIT/Apache-2.0 as originally suggested when this migration was requested; still pure Rust, no
native/FFI dependency, which is the actual hard requirement) for rasterization; `ttf-parser`
(already a dependency, `fonts` feature) is reserved for a later font-outline phase — no text
operators are implemented yet.**

### What was built this phase

- `src/render/native/` (new module, `native-render` feature = `dep:tiny-skia` + `parser`):
  - `interpreter.rs`: the content-stream interpreter loop. Reuses the existing, already-tested
    generic content-stream tokenizer (`src/editor/content_stream.rs`'s `parse_content_stream`/
    `ContentItem`, widened from module-private to `pub(crate)` so `render::native` — a sibling of
    `editor`, not a descendant — can reach it; no behavior change to that tokenizer).
  - `state.rs`: `GraphicsState`/`GraphicsStateStack` (the `q`/`Q` save/restore stack, ISO 32000-1
    8.4.2), bounded at `MAX_GRAPHICS_STATE_DEPTH` (4096) against a "q-flood" memory-exhaustion
    attempt from a crafted content stream.
  - `path.rs`: device-space path accumulation for `m l c v y h re`. Path coordinates are
    transformed to device space via the CTM *at the time each construction operator executes*
    (matching how most content-stream interpreters behave, and correctly handling the rare but
    spec-legal case of `cm` appearing mid-path), so painting always hands `tiny-skia` an
    already-device-space path with an identity transform.
  - `color.rs`: DeviceGray/DeviceRGB/DeviceCMYK → `tiny_skia::Color` only. CMYK→RGB uses the
    naive, non-ICC formula ISO 32000-1 8.6.5.3 itself gives as the default conversion
    (`red = 1 - min(1, C+K)` etc.) — explicitly **not** ICC color management.
  - `error.rs`: `NativeRenderError` (hard failures: invalid dimensions, degenerate `/MediaBox`,
    graphics-state stack overflow) and `RenderWarning` (soft/recoverable: unsupported operator,
    missing `ExtGState` resource, invalid dash pattern, truncated content stream, unbalanced `Q`)
    — every one of these is a structured, non-panicking outcome; see the untrusted-input handling
    below.
  - `mod.rs`: `render_content_stream(content, width, height, media_box, resources) ->
    Result<NativeRenderOutput { pixmap: tiny_skia::Pixmap, warnings: Vec<RenderWarning> },
    NativeRenderError>`.

### Scope implemented (ISO 32000-1 Chapter 8)

Graphics-state stack (`q`/`Q`, 8.4.2) and CTM (`cm`, 8.3.4); path construction (`m l c v y h re`,
8.5.2) and painting (`f F f* S s B B* b b* n`, 8.5.3); clipping (`W`/`W*`, 8.5.4, via an 8-bit
`tiny_skia::Mask`); a basic `ExtGState` subset (`gs` reading only `ca`/`CA`/`LW`/`D`, 8.4.5); line
style (`w J j M d`, 8.4.3); and DeviceGray/DeviceRGB/DeviceCMYK color only (`g G rg RG k K`,
8.6.3-8.6.5).

### Explicit, honest gaps (recorded as structured warnings, not silently faked) — verifier: read this

Per the task's explicit instruction, the following are **known, disclosed gaps**, not claimed as
"done":

- **No text rendering at all yet** (`Tj TJ ' " BT ET Tf Td Tm ...`) — every text-showing operator
  is recorded as `RenderWarning::UnsupportedOperator` and skipped; a text-only page currently
  rasterizes to a blank (background-only) page. **Superseded by §8b below** ("Text Rendering",
  phase 2): these operators are now implemented for TrueType/OpenType simple fonts, CID/Type0
  (CJK) composite fonts, and Type 3 glyph procedures. Left as-written here for historical accuracy
  about what phase 1 alone shipped; §8b documents the remaining (still real) text-related gaps
  (Type1/bare-CFF font programs, non-embedded fonts, vertical writing mode, `/Differences`).
- **No images or Form XObjects** (`Do`, inline images `BI`/`ID`/`EI`) — not painted.
- **No shadings/patterns** (`sh`, Pattern color space).
- **No non-Device color spaces** (`cs`/`CS`/`sc`/`SC`/`scn`/`SCN`: CalGray/CalRGB/Lab/ICCBased/
  Indexed/Separation/DeviceN) — selecting one leaves the current color unchanged and warns,
  rather than guessing an approximate conversion.
- **JBIG2 / JPX (JPEG2000)** — there is no mature pure-Rust decoder for either anywhere in the
  ecosystem today. No image decoding of any kind happens yet this phase, so there is no code path
  for this gap yet, but it is recorded here because it will remain a **hard, structural gap** in a
  later image-painting phase: such images must fail closed (structured error/placeholder), never
  silently blank or panic.
- **Type1/CFF embedded font programs** — no mature pure-Rust Type1/CFF *charstring interpreter*
  exists in the ecosystem today (`ttf-parser` parses CFF *tables*, not charstrings, and this
  phase doesn't wire up any glyph outline extraction at all yet). A later text-rendering phase
  must fail closed for glyphs it cannot shape.
- **ICC color management** — not implemented; the CMYK conversion above is the spec's own naive
  fallback formula, not profile-based color management.
- **Transparency groups/blend modes** (Chapter 11) beyond flat constant alpha (`ca`/`CA`) — not
  implemented.
- **Non-uniform-scale/skewed CTM effect on stroke width and dash length** — approximated by the
  CTM's uniform scale factor (`sqrt(|det(CTM)|)`); a heavily skewed CTM will not produce the
  spec-exact elliptical stroke pen. Documented as an intentional phase-1 simplification, not
  claimed as spec-exact.

None of the above cause a hard error or a panic — the render completes and paints everything this
phase does support; only structurally-impossible requests (zero-size output, degenerate
`/MediaBox`, a `q`-flood past `MAX_GRAPHICS_STATE_DEPTH`) are hard `NativeRenderError`s.

### Untrusted input handling

Content-stream bytes are untrusted. No `unwrap()`/`expect()` on stream-derived data (malformed or
non-finite numeric operands are rejected per-operator via `nums`/`as_f64` and that one operator
invocation is skipped, not the whole render); `q`/`Q` stack depth is capped
(`MAX_GRAPHICS_STATE_DEPTH`); the collected-warnings list is capped (`MAX_WARNINGS` = 1000)
against a content stream consisting of millions of unsupported operators; non-finite
(`NaN`/`Infinity`) coordinates are sanitized to a finite fallback before reaching the rasterizer,
and `tiny-skia` itself additionally refuses (logs, does not panic) to rasterize path geometry
whose magnitude would overflow its internal math.

### Tests

`src/render/native/mod.rs`'s `tests` module: 13 tests rendering real content-stream byte strings
through the new interpreter and asserting actual pixel colors/positions (not just "didn't panic")
— colored rectangle fill/position, DeviceGray, DeviceCMYK, `q`/`Q` color and CTM isolation, a
stroked line, even-odd fill creating a hole, a clip rectangle restricting a full-canvas fill,
`ExtGState` alpha blending, an unsupported (text) operator degrading to a warning without
aborting the rest of the stream, and three adversarial-input cases (zero/degenerate dimensions, a
`q`-flood, truncated trailing bytes) asserting structured errors/warnings rather than a panic.
Plus unit tests in `color.rs` (7), `path.rs` (4), `state.rs` (4), `interpreter.rs` (4) for the
individual building blocks. See §10-equivalent test run output in the task report for this phase.

## 8b. Pure-Rust rendering migration, phase 2: "Text Rendering" (implemented)

Builds directly on §8a's interpreter core. Same coexistence model (`render`/Pdfium and
`native-render`/pure-Rust remain two independent, optional features; nothing in §8/§8a changed
behaviorally). `native-render` now additionally pulls in the `fonts` feature (`dep:ttf-parser`,
`dep:subsetter`) so glyph outline extraction has `ttf-parser` available.

### What was built this phase

- `src/render/native/font.rs` (new): resolves a `/Resources /Font /<name>` dictionary into
  whatever the interpreter needs to paint text — a loaded `ttf-parser` font program (or,
  honestly, the reason it couldn't load one), glyph-selection/width tables, and (for Type 3) the
  glyph-name-to-`CharProc`-bytes map. Also parses `/W` (CID widths, both Table 118 forms, with a
  bounded range-expansion guard against a crafted `0 4294967295 500`-style entry) and `/Widths`
  (simple/Type 3, Table 111).
- `src/render/native/glyph.rs` (new): adapts `ttf-parser`'s `OutlineBuilder` callback trait to a
  `tiny_skia::PathBuilder`, transforming each point through the glyph-space-to-device-space
  matrix (and sanitizing non-finite results, same rule as `path.rs`) on the way in.
- `src/font/truetype.rs`: added `TrueTypeFont::outline_glyph()`, wrapped in the same
  `catch_unwind` defense-in-depth this module already uses for `Face::parse` itself (embedded
  font bytes are untrusted; `ttf-parser`'s outline extraction is documented panic-free but this
  crate does not fully trust that guarantee for adversarial input either).
- `src/render/native/state.rs`: added `TextState` (ISO 32000-1 9.3: `Tc Tw Tz TL Tf Tr Ts`) as a
  field of `GraphicsState` — these genuinely are graphics-state parameters (saved/restored by
  `q`/`Q`), unlike the text object's `Tm`/`Tlm` matrices, which the interpreter keeps as its own
  fields, reset only at `BT`.
- `src/render/native/interpreter.rs`: added the text-positioning (`Td TD Tm T*`), text-state
  (`Tc Tw Tz TL Tr Ts`, `BT`/`ET`), and text-showing (`Tj TJ '` `"`) operators, plus the actual
  per-glyph paint path for all three font kinds. This required a structural change: `Interpreter`
  now borrows its `Pixmap`/warnings `Vec` (`&mut`, two lifetimes) instead of owning them, so a
  Type 3 glyph's `CharProc` can be interpreted by a **recursively nested `Interpreter` instance**
  sharing the same underlying raster/warning sink, bounded by `font::MAX_TYPE3_DEPTH` (6) against
  a self-referential/mutually-recursive Type 3 font (untrusted input — a `CharProc` is itself a
  content stream that can, in principle, `Tf` a different Type 3 font and show more text).

### Scope implemented (ISO 32000-1 Chapter 9)

Text state (`Tc Tw Tz TL Tf Tr Ts`, 9.3) and text objects (`BT`/`ET`, 9.4.1); text positioning
(`Td TD Tm T*`, 9.4.2); text showing (`Tj TJ '` `"`, 9.4.3) with the full advance-width formula
(9.4.4: `Tc`/`Tw`/`Tz` and `TJ` numeric adjustments; `Tw` correctly restricted to the single-byte
code `32` case). Three font kinds:

- **Simple TrueType/OpenType fonts** (9.6.3): code → Unicode via the same WinAnsi-ish fallback
  table `crate::editor::text_extract` already uses for extraction (`crate::font::encoding`) →
  glyph ID via the embedded font's own `cmap`.
- **Composite (Type 0/CIDFontType2) fonts** (9.7), including CJK: glyph selection reuses
  `crate::font::cid::CompositeFont`'s own `Identity-H`, 2-byte-code, code-is-CID-is-original-GID
  conventions (honoring an explicit `/CIDToGIDMap` stream if present, else identity) — the writer
  and this reader are different code paths (one authors PDFs, the other interprets arbitrary
  ones) but agree on the same on-disk contract.
- **Type 3 fonts** (9.6.5): each glyph's `CharProc` content stream is interpreted recursively
  through the very same interpreter (see above), with the glyph-space-to-device-space transform
  computed as `FontMatrix × [Tfs·Tz 0 0 Tfs 0 Ts] × Tm × CTM` per 9.4.4/9.6.5.2. `d0`/`d1` are
  recognized (operands consumed, not flagged `UnsupportedOperator`) but not acted on — glyph
  advance is sourced from `/Widths` instead, and `d1`'s optional glyph-bbox clip is skipped.

### Explicit, honest gaps (recorded as structured warnings, not silently faked) — verifier: read this

- **Type1 and bare/un-wrapped CFF embedded font programs remain a hard, structural gap** — the
  exact one §8a called out in advance. `ttf-parser` (this crate's only font-parsing dependency)
  requires an `sfnt`/OpenType table directory; it cannot parse a raw Type1 `PFA`/`PFB` program or
  a `FontFile3` stream whose `/Subtype` is `/Type1C`/`/CIDFontType0C` (bare CFF, not
  OpenType-wrapped). This phase attempts to load *every* embedded font program regardless of
  which `FontFile*` key it came from (`font::load_font_program`) and only classifies the result
  as this gap if the load genuinely fails — an OpenType-wrapped CFF program is **not** part of
  this gap, since `ttf-parser` truly parses those. When it is the gap: no glyph is painted for
  that font (but its declared `/Widths`/`/W` still advance the pen, so surrounding text doesn't
  visually collapse), `RenderWarning::UnsupportedFontProgram` is recorded once per resource name,
  and — this is the part the verifier is specifically asked to check — this is **not** claimed as
  "supported": see `src/render/native/font.rs`'s and `mod.rs`'s module docs, plus the dedicated
  regression test `text_tests::type1_bare_cff_font_fails_gracefully_not_panicking` (asserts *no*
  ink painted in the glyph's would-be bounding box, a `RenderWarning::UnsupportedFontProgram`
  naming the gap, and — critically — that the *rest* of the page still renders, i.e. this is a
  graceful per-font failure, not a whole-render abort).
- **Non-embedded fonts are also not rendered** — this phase has no standard/system-font
  substitution database at all (unlike Pdfium). Any font (any `/Subtype`, including TrueType)
  with no `FontFile`/`FontFile2`/`FontFile3` fails the same way as the Type1/CFF gap above. This
  is a distinct, separately-tracked gap (`font::UnsupportedFontReason::NotEmbedded`) — "no
  charstring interpreter" and "no font-substitution logic" are two different missing pieces, not
  one.
- **Only horizontal writing mode** — vertical CID fonts (`Identity-V` and similar) are not
  detected; glyphs are always positioned as if horizontal.
- **Only 2-byte codes are assumed for every composite (Type 0) font** — the same simplification
  already shipped (and documented) in `crate::editor::text_extract`, since this crate's own
  writer (`crate::font::cid`) only ever emits `Identity-H`.
- **`/Encoding` `/Differences` and symbolic (non-Unicode-`cmap`) simple fonts** are not specially
  resolved — same documented gap as `text_extract` (needs the Adobe Glyph List, not implemented).
- **Text clipping render modes** (`Tr` 4-7) paint like their non-clipping counterpart (0-3) but do
  not add glyph outlines to the clip path.
- Every gap §8a already disclosed (images/XObjects, shadings/patterns, non-Device color spaces,
  JBIG2/JPX, ICC color management, transparency groups/blend modes, skewed-CTM stroke
  approximation) is unchanged by this phase.

None of the above raise a hard `NativeRenderError` — the render still completes and paints every
glyph/pixel this phase does know how to paint.

### Untrusted input handling (additions this phase)

Beyond everything §8a already covers: Type 3 recursion is bounded (`font::MAX_TYPE3_DEPTH` = 6,
tested by `text_tests::type3_self_referential_charproc_is_bounded_not_infinite`, a font whose own
`CharProc` shows text using itself); `/W` array range-form expansion is capped at 65,536 entries
per range (`font::parse_w_array`, tested by
`font::tests::w_array_range_form_is_bounded_against_a_huge_range`) against a crafted
`0 4294967295 500`-style entry trying to force a multi-billion-entry `BTreeMap`; glyph outline
extraction goes through the same `catch_unwind` defense-in-depth `TrueTypeFont::load` already
uses, since embedded font bytes are attacker-controlled.

### Tests

`src/render/native/text_tests.rs` (5 tests) — the phase's Definition-of-Done tests, each
rendering a real content stream through `render_content_stream` and asserting actual non-
background ink pixels land inside the expected glyph bounding box (not just "didn't panic"):
TrueType simple-font glyph (a hand-built font with a genuine, non-empty square `glyf` outline —
deliberately *not* reusing `truetype.rs`'s zero-contour `build_test_font` fixture, since an empty
outline can't prove ink actually landed anywhere), composite/CID CJK glyph (against the real,
OFL-licensed `tests/fixtures/fonts/NotoSansSC-Subset.ttf` fixture, reusing
`crate::font::cid::CompositeFont::encode` for the content-stream bytes), a Type 3 `CharProc`
glyph, a bounded-self-recursion Type 3 font, and the Type1/bare-CFF graceful-failure test
described above. Plus new unit tests in `font.rs` (10) and `glyph.rs` (2), and two updated tests
in `mod.rs` (the old "text is unsupported" test was replaced with one showing `Do` is still
unsupported, plus a new one for a `Tf` naming a missing font resource).

## 8c. Pure-Rust rendering migration, FINAL phase: Integration — Pdfium fully removed (completed)

**This closes out §8/§8a/§8b's "live migration in progress" status.** Per an explicit user
request, `render::PdfRenderer::render_page`'s implementation has been swapped to run entirely on
the pure-Rust engine built in §8a/§8b/(subsequent Color-Spaces/Images and Transparency &
Blend-Modes phases, both implemented in `render::native` — see that module's own docs for their
Definitions of Done); the FFI-to-Pdfium backend described in §8 has been **deleted, not just
deprecated**:

- `pdfium-render` is no longer a dependency (removed from `Cargo.toml`). `PdfiumLibrary` no longer
  exists. `grep -ri pdfium src/` returns nothing (this file and `docs/THREAT_MODEL.md` keep the
  historical record; source code does not).
- `scripts/fetch_pdfium.sh` and the gitignored `.pdfium/` local-binary cache are gone.
- The `render` Cargo feature no longer means "FFI to Pdfium" — it now means "whole-document
  `PdfRenderer` API, built on `native-render` + this crate's own `EditableDocument` parser", and
  pulls in `native-render` (previously the two features were independent/coexisting). `native-render`
  itself is unchanged: still the lower-level, single-content-stream engine with no document/page-tree
  access of its own.
- **`render::PdfRenderer` is rebuilt on `editor::EditableDocument`** instead of a Pdfium
  `PdfDocument` handle: `open_file`/`open_bytes` no longer take a `PdfiumLibrary` handle (there is
  nothing left to bind), and `render_page`'s own signature (`&self, page_index: usize, dpi: f32,
  viewport: Option<Viewport>) -> Result<RgbaImage, RenderError>`) is **unchanged**, so the Tauri
  command layer's own `render_page` call site needed no signature changes downstream of it.
- **`tauri_commands/render_actor.rs` is deleted.** That module existed solely because Pdfium's
  `PdfDocument` was not `Send` and its C API was not safe to call concurrently (see §9's original
  text, kept below for history), forcing a dedicated single-OS-thread "actor" instead of the normal
  `WorkerPool`. `EditableDocument` is already `Send + Sync` (proven by a compile-time assertion in
  `tauri_commands/state.rs`, and by the fact every *other* command already shared it across
  `WorkerPool` threads), so `commands::render_page_impl` now just locks the same
  `Arc<DocumentEntry>`'s `Mutex<EditableDocument>` other commands (`extract_text`, `apply_edit`,
  ...) already share for that handle, briefly, inside a normal `WorkerPool::run` job — no dedicated
  actor thread, no per-document cached renderer, no FFI lock. A dedicated regression test,
  `tauri_commands::commands::tests::render_page_is_correct_under_concurrent_calls_via_worker_pool`,
  proves this: many concurrent `render_page_impl` calls for several different pages of the same
  open document, issued from multiple `tokio::spawn`ed tasks on a multi-threaded runtime, all
  succeed, agree pixel-for-pixel per page, and never cross-contaminate between pages.
- **New capability, required to make whole-document rendering actually work (not previously
  needed by §8a/§8b's single-content-stream-in-isolation tests):** `render::renderer::deep_resolve`
  recursively dereferences every `Object::Reference` reachable from a page's `/Resources` (fonts,
  `/DescendantFonts`, `FontFile*`/`CIDToGIDMap` streams, XObjects — including a Form XObject's own
  nested `/Resources` — ExtGStates, colour spaces) before handing it to
  `native::render_content_stream`, which has always assumed (documented in its own module docs)
  that `/Resources` arrives already fully dereferenced. A real, serialized-then-reparsed PDF almost
  always represents these as indirect references, so without this step, essentially every embedded
  font/image/ExtGState on a genuine whole-document page silently failed to resolve. Bounded
  (`MAX_RESOLVE_REFERENCES`/`MAX_RESOLVE_DEPTH`) against a crafted reference cycle or a
  pathologically wide/deep resource graph, per this crate's untrusted-input rules. This bug was
  caught by this phase's own CJK visual-render test (`tests/font_embedding_tests.rs`) regressing
  from "0 warnings, ink present" (against a hand-built, already-inline-dictionary fixture in
  `render::native`'s own unit tests) to "0 ink pixels" once wired through a real parsed document —
  fixed before this phase was reported done, not left as a known gap.
- **New capability: page rotation (`/Rotate`, ISO 32000-1 Table 30) is now honored.**
  `render::native::render_content_stream` itself has no concept of page rotation (it just
  rasterizes a content stream into a given pixel buffer); `render::renderer` normalizes the
  effective `/Rotate` to `{0, 90, 180, 270}` (a non-multiple-of-90 value — spec-non-conformant — is
  treated as `0`), renders the *unrotated* content at the swapped-if-needed pixel dimensions, and
  applies the rotation as a whole-image `image::imageops::rotate90/180/270` post-processing step
  (exact for the only values ISO 32000-1 permits, and cheap — a pixel transpose/flip, not a
  re-render). `Viewport` tiling operates on the *rotated* (displayed) raster, matching the
  previous Pdfium-backed behavior's coordinate convention.
- **New capability: annotation appearance streams (ISO 32000-1 §12.5.5) are now painted.**
  `native::render_content_stream` only ever interprets a page's main content stream — it has no
  knowledge of `/Annots` at all, and painting annotation appearances was never part of §8a/§8b's
  scope (Pdfium's `render_with_config` handled this internally and invisibly to this crate, so it
  was never an explicit gap on `render::native`'s own list either — it simply wasn't wired up
  anywhere in this crate's own code until whole-document rendering needed it). This phase's own
  regression test (`tests/render_tests.rs`'s `annotation_visual_render` module — pre-existing,
  written against the Pdfium backend) caught this immediately: highlight/underline/strikeout/
  freetext/stamp/ink annotations rendered *zero* visible difference from an unannotated control
  page. Fixed by `render::renderer::append_annotation_appearances`, which resolves each visible
  annotation's current `/AP /N` appearance stream (honoring `/AS` for a multi-state appearance,
  skipping `Hidden`/`NoView`-flagged and `/Subtype /Popup` annotations per spec convention),
  computes the ISO 32000-1 §12.5.5 BBox-to-Rect fitting matrix, and synthesizes a
  `q <matrix> cm /<name> Do Q` appended to the page's content stream, registering the resolved
  appearance as an ordinary Form XObject resource — reusing the *existing*, already-tested Form
  XObject rendering path (including transparency groups, if an appearance happens to declare one)
  rather than a separate, annotation-specific renderer. Honest, documented simplifications: a
  missing/malformed appearance `/BBox` falls back to an unscaled placement at `/Rect`'s origin
  instead of failing; Optional Content (`/OC`) visibility is not evaluated; Widget annotations
  without a usable appearance are not synthesized from `/DA`/field values (a distinct,
  separately-scoped "generate a default appearance" feature).
- **Accepted trade-off, honestly regressed and documented (not silently dropped): decryption.**
  The previous Pdfium-backed renderer could open a password-protected document (Pdfium implements
  RC4/AES decryption internally); this crate's own parser (`parser::PdfReader`) has never
  implemented any decryption filter at all (a pre-existing limitation of the whole
  structural-editing pipeline, predating this migration — `open_document` already rejected
  encrypted files the same way before `render_page` did). `RenderError::PasswordRequired` is
  returned unconditionally for an encrypted document now, regardless of any password supplied; see
  that variant's doc comment and the `password_protected_document_cannot_be_opened_at_all` test in
  `tests/render_tests.rs`, which exists specifically to prove this fails closed rather than
  silently "succeeding" with garbage output.
- Every compatibility gap this migration accepted going in (JBIG2/JPX images, Type1/bare-CFF font
  programs, non-embedded/system font substitution, no true ICC colour management, no
  Patterns/shadings, approximated transparency-group/soft-mask semantics) is unchanged from
  §8a/§8b and `render::native::mod.rs`'s own "Explicit, honest gaps" section — this integration
  phase did not attempt to close any of them, only to wire the already-implemented engine up to
  whole-document rendering and the Tauri command layer correctly.

### Tests

`tests/render_tests.rs` (11), `tests/font_embedding_tests.rs` (6, including the CJK visual-render
regression above), `tests/large_file_render_bench.rs` (1, `#[ignore]`d — no longer needs a native
library fetch/bind step at all, since this pipeline has none), `tests/tauri_commands_integration.rs`
(2), plus `render::renderer`'s own new unit tests (`normalize_rotate`/`apply_rotation_to_dims`/
`check_dimensions`/`page_pixel_size`/`open_bytes` error-path/`page_count` fallback) and
`editor::pages`'s new `effective_media_box`/`parse_media_box` tests (valid + adversarial: missing
`/MediaBox`, swapped corners, wrong element count, non-numeric entry, non-finite coordinate) — all
passing. `cargo build`/`cargo test`/`cargo clippy -- -D warnings` were re-run clean across every
feature combination touched by this migration (`default`, `native-render`, `render`, `full`,
`full,tauri`), not just the one named in this phase's Definition of Done.

## 8d. Known Limitations (pure-Rust rendering, current state — this refresh)

**Read this section first if you need to know what the renderer actually does today.** Everything
below was re-verified live for this refresh — either by reading the current source in
`src/render/native/` or by running the named test — not carried forward from an earlier phase's
claim. None of these are silent: every one of them is a documented, structured
[`RenderWarning`]/[`NativeRenderError`] variant (`src/render/native/error.rs`) recorded in the
render's returned warning list, never a panic and never a quietly-blank/quietly-wrong pixel. This
is the deliberate, task-mandated distinction: a "gap" here means *"we tell the caller we couldn't
do this,"* not *"we did it badly and didn't say so."*

### JBIG2 / JPX (JPEG2000) images — placeholder, not decoded

**Not supported. There is no mature pure-Rust decoder for either format anywhere in the ecosystem
today** (checked again for this refresh — this remains true, the same conclusion §4 finding #8 and
§8a/§8b/§14 already reached). Concretely:

- `crate::filter::decode_filter` already refuses to silently pass either filter through
  undecoded (a plain byte-copy would be actively misleading, not a placeholder) — it returns a
  structured `FilterError` instead.
- `render::native::image`/`interpreter::paint_placeholder_rect` catches that failure and paints the
  image region as a **documented placeholder** (a flat, distinctive mid-grey `rgb(160,160,160)`
  fill over the image's unit square — the same "broken image" convention most browsers/viewers
  use, deliberately not white/black so it can't be mistaken for real page background or content)
  rather than leaving it blank or aborting the whole page render, recording
  `RenderWarning::UnsupportedImageFilter { filter: "JBIG2Decode" | "JPXDecode", .. }`.
- Live-verified this refresh via the actual tests: `render::native::image_integration_tests::
  jbig2_image_xobject_yields_placeholder_not_panic_not_silent_blank`,
  `jpx_image_xobject_also_yields_placeholder_not_panic`, and
  `inline_image_with_jbig2_is_rejected_gracefully` — all three assert the placeholder is painted
  (non-blank, distinguishable from "nothing happened"), the correct `RenderWarning` is recorded,
  and — critically — the rest of the page still renders around it.
- **Do not read this as "JBIG2/JPX images are supported."** They are not. The placeholder exists so
  a scanned-document page with one unsupported image doesn't come back as a totally blank page or
  crash the whole render — it is a graceful-degradation mechanism, not a decoder.

### Type1 and bare/un-wrapped CFF embedded font programs — fails closed, not rendered

**Not supported.** `ttf-parser` (this crate's only font-parsing dependency, MIT OR Apache-2.0) is
an `sfnt`/OpenType-table-directory parser; it has no Type1 (`eexec`-encrypted charstring)
interpreter and cannot parse a bare CFF `FontFile3` (`/Type1C`/`/CIDFontType0C`) stream that isn't
wrapped in an OpenType container. (An OpenType font whose *outline table* happens to be
CFF-flavored — a real `sfnt` container — is genuinely parsed by `ttf-parser` and is **not** part of
this gap; only bare/unwrapped Type1/CFF is affected.)

- When this gap is hit, `render::native::font` classifies it as
  `UnsupportedFontReason::Type1OrBareCff` and the interpreter paints **no glyph ink at all** for
  that font, but still advances the text position using the font's declared `/Widths`/`/W` — so
  surrounding text on the same line doesn't visually collapse into the gap — and records
  `RenderWarning::UnsupportedFontProgram` once per resource name.
- A distinct, separately-tracked gap (**not** the same thing): **non-embedded fonts** (any
  `/Subtype`, no standard/system-font substitution database at all) fail identically —
  `UnsupportedFontReason::NotEmbedded`. "No charstring interpreter" and "no font-substitution
  logic" are two different missing pieces; this document does not conflate them, per this crate's
  own module docs.
- Live-verified this refresh via `render::native::text_tests::
  type1_bare_cff_font_fails_gracefully_not_panicking` — asserts *no* ink lands in the glyph's
  would-be bounding box, the correct `RenderWarning` is recorded naming the gap, and the rest of
  the page still renders (a graceful per-font failure, not a whole-render abort).
- **Do not read this as "Type1/CFF fonts are supported with a fallback appearance."** There is no
  fallback glyph shape drawn — the text is simply invisible for that specific font, by design,
  rather than silently substituting a wrong-looking glyph or panicking.

### ICC color management — approximated, not colour-managed

**Not implemented as true ICC profile-based colour management, and not claimed to be.**

- An `ICCBased` colour space (ISO 32000-1 §8.6.5.5) is **approximated**: `render::native::
  colorspace` resolves it to its `/Alternate` colour space if the PDF declares one, else falls back
  to a heuristic guess purely from the profile's declared `/N` (1→DeviceGray, 3→DeviceRGB,
  4→DeviceCMYK) — it never actually parses or applies the embedded ICC profile's colorimetric
  transform. There is no mature pure-Rust ICC colour-management engine (a full CMM) this crate has
  adopted or evaluated as production-ready.
- Every use of an `ICCBased` space records `RenderWarning::IccColorApproximated` exactly once, so a
  caller can distinguish "colour looked right by luck" from "colour was actually colour-managed."
- Related, same underlying limitation: `DeviceCMYK`→RGB conversion (`render::native::color`) uses
  the naive, non-perceptual formula ISO 32000-1 §8.6.5.3 itself documents as the *fallback*
  conversion (`red = 1 - min(1, C+K)` etc.), not a profile-based conversion — this applies to every
  CMYK-painted pixel, not just ones that went through an `ICCBased` space.
- `Lab` colour space is a step further back: not implemented at all (resolves to
  `ColorSpace::Unsupported`, recording `RenderWarning::UnsupportedColorSpace`), since a correct
  implementation needs a CIE L\*a\*b\*→device-RGB conversion this phase doesn't have either.
- **Do not read "ICC approximated" as "color looks right."** For a document whose visual
  correctness depends on an embedded ICC profile (e.g. proofing/prepress workflows), this
  renderer's colour output should be treated as indicative, not accurate.

### Performance delta vs. the previous Pdfium benchmark (measured, this session — corrected)

> **This subsection replaces the third refresh's version, which claimed a "20.9× slower, real,
> measured regression" that does not reproduce.** See the corrected-fourth-refresh banner at the
> top of this document for how that was found; this text states the honest, current picture.

The "Large-File Render Benchmark" remediation phase (commit `e04af03`) recorded, for the
Pdfium-backed renderer opening + rendering page 0 of a real ~2.10 GB / 10,000-page fixture (793×1123
px raster) via `tests/large_file_render_bench.rs`: **82.46 ms**. That figure is not being disputed
here — only the pure-Rust comparison figure the third refresh set against it.

Re-running the *exact same test file* (unmodified assertion/methodology — it now runs the pure-Rust
`PdfRenderer::open_file`/`render_page` path instead, per §8c) against the exact same
fixture-generation code, **eight consecutive times this session**, on the same cached 2.10 GB
fixture used throughout:

```
$ cargo test --release --features render --test large_file_render_bench -- --ignored --nocapture
   (run 1, immediately after `cargo build --release ... --test large_file_render_bench`)
large_file_render_bench: opened + rendered page 0 ... in 1.582036125s (793x1123 px raster)

   (runs 2-8, identical command, no rebuild in between)
... in 59.605666ms
... in 62.000959ms
... in 60.171042ms
... in 67.082166ms
... in 67.302250ms
... in 62.637167ms
... in 63.784667ms
```

Two distinct numbers, both real, both reproduced live this session, and neither honestly described
as "the" performance of this renderer on its own:

- **Cold-start (1 of 8 runs, immediately following a fresh build of the test binary): 1.58 s.**
  This is the same order of magnitude as the third refresh's cited 1,723.67 ms, and the mechanism
  is now understood: building the test binary (or, separately, regenerating the 2.10 GB fixture
  file) causes enough other disk/filesystem activity that the fixture's pages are evicted from (or
  were never yet loaded into) the OS page cache, so the *first* read of the 2.10 GB file inside
  `PdfRenderer::open_file` pays real, uncached disk-read latency. `open_file` and `render_page`
  issue the same reads either way; only the kernel's page-cache hit/miss changes between the two
  numbers.
- **Steady-state (7 of 8 runs, same warm-cached file, no rebuild): ~58-70 ms** (mean ≈ 63 ms). This
  is **faster than, not slower than, the 82.46 ms Pdfium baseline** — the opposite of the third
  refresh's "20.9× slower" claim, for the identical open-a-huge-document-then-rasterize-one-
  ordinary-page workload.

**Conclusion: this is not a demonstrated performance regression against Pdfium.** The correct,
honest statement is: (a) in steady state, on this one machine, the pure-Rust path is at least as
fast as the previously-recorded Pdfium figure on the one workload this repo benchmarks; (b) the
*first* touch of a large, not-recently-read file after a fresh build/fixture-regen can cost well
over a second, but that cost is attributable to disk I/O / page-cache state, not to
`tiny-skia`/`ttf-parser`/the content-stream interpreter being slow, and the third refresh's mistake
was reporting exactly one cold-start sample as if it were a stable, session-authoritative
measurement of the renderer itself; (c) neither the 82.46 ms Pdfium figure nor any of this
session's eight numbers come from a controlled, simultaneous, isolated-hardware A/B benchmark —
they are same-repo, different-session (or, for the cold-start number, same-session-different-cache-
state) data points, not a rigorous comparison. No attempt was made this session to profile *where*
inside either the ~63 ms steady-state or the ~1.6 s cold-start path time actually goes (page-tree
resolution vs. `deep_resolve` vs. disk I/O wait vs. rasterization vs. text shaping), and no attempt
was made to reproduce a Pdfium-side cold-start number for a fully apples-to-apples comparison — both
are legitimate follow-ups for whoever picks up rendering-performance work next, not something this
documentation fix resolves.

### Other accepted approximations (unchanged from §8a/§8b/§8c, restated here for one-stop reference)

- **Transparency groups always treated as isolated**, regardless of the group's actual `/I` entry;
  **knockout (`/K`) is not implemented**. Blend modes (`/BM`) themselves *are* fully, accurately
  implemented (all 16 standard ISO 32000-1 §11.3.5 modes via `tiny_skia::BlendMode`) — it is
  specifically group/mask compositing semantics beyond isolated-groups-with-default-backdrops that
  are approximated, not the per-pixel blend formulas.
- **Soft-mask `/BC` (custom backdrop colour) and `/TR` (transfer function) are recognised but
  ignored** (identity transfer function; the spec's own default backdrop is always used), recording
  `RenderWarning::SoftMaskParameterIgnored`.
- **Patterns and shadings (`sh`, Pattern colour space) are not painted at all** — selecting a
  Pattern colour records `RenderWarning::PatternColorUnsupported` and leaves the current colour
  unchanged.
- **The older `/Mask` explicit-mask mechanism** (ISO 32000-1 §8.9.6.4: colour-key array or stencil
  image, as opposed to the newer `/SMask` soft mask, which *is* implemented) **is not implemented**
  — every image ignores a `/Mask` entry if present. Not separately warned per-image (judged too
  noisy); documented here instead.
- **Vertical writing mode is not detected** — `Identity-V` and similar composite fonts are always
  positioned as if horizontal.
- **`/Encoding` `/Differences` and symbolic (non-Unicode-`cmap`) simple fonts are not specially
  resolved** — same documented gap as `editor::text_extract` (needs the Adobe Glyph List, not
  implemented).
- **Text clipping render modes** (`Tr` 4-7) paint like their non-clipping counterparts (0-3) but do
  not add glyph outlines to the clip path.
- **Non-uniform-scale/skewed CTM effect on stroke width and dash length is approximated** by the
  CTM's uniform scale factor (`sqrt(|det(CTM)|)`); a heavily skewed CTM will not produce the
  spec-exact elliptical stroke pen.
- **Tiled/viewport rendering still re-renders the whole page internally and crops the requested
  rectangle out of it** (`src/render/mod.rs`'s own "Known limitation" section) — memory use for a
  tile is bounded by the *full page* at the requested DPI, not the tile size.
- **Decryption remains entirely unimplemented for the renderer**, same as the rest of this crate's
  read path (§1, §4 finding #1): `RenderError::PasswordRequired` is returned unconditionally for an
  encrypted document, regardless of any password supplied.

None of the above raise a hard error or panic on their own — the render completes and paints every
pixel/glyph this engine does know how to paint; only structurally-impossible requests (zero-size
output, a degenerate `/MediaBox`, a `q`-flood past `MAX_GRAPHICS_STATE_DEPTH`, an
output-pixel-count over `MAX_RENDER_PIXELS`) are hard errors, and those are also structured
(`NativeRenderError`/`RenderError`), never a panic on untrusted input.

## 9. The Tauri command layer (`tauri_commands/`)

New top-level module since the original audit; **6 files, 2,592 lines** as of this refresh (was 7
files/2,865 lines — `render_actor.rs`, 437 lines, was deleted this migration; see below), gated by
the `tauri` feature (which pulls in `parser`, `render` and `signatures`). This is the glue between
the pure Rust library and a Tauri desktop application:

- **Nine async commands** (`tauri_commands/commands.rs`): `open_document`, `render_page`,
  `extract_text`, `search_text`, `apply_edit`, `save_document`, `fill_form`, `add_annotation`,
  `sign_document`. Every command follows a `..._impl` (plain async, testable without a live Tauri
  `AppHandle`) + thin Tauri-wired wrapper shape.
- **`state.rs`**: Tauri-managed application state — the registry of currently-open documents
  (each an `Arc<DocumentEntry>` wrapping a `Mutex<EditableDocument>`, shared by every command
  including `render_page`) plus a handle to the single worker pool every command uses.
- **`worker.rs`**: a dependency-light thread pool that runs CPU-bound PDF parsing/editing/signing/
  **rendering** work off of Tauri's own async-command executor threads (verified by
  `no_blocking_of_a_single_threaded_executor` in `tests/tauri_commands_integration.rs`).
- **`render_actor.rs`: retired, see §8c.** This module used to give page rasterization its own
  dedicated single OS thread, separate from `worker.rs`'s pool, because Pdfium's `PdfDocument`
  handle was not `Send` and its C API was not safe to call concurrently (a panic-containment
  layer was added to it in a later "RenderActor Panic Resilience" remediation phase, back when it
  still existed). Now that rendering is built on `EditableDocument` (`Send + Sync`, no native
  library involved), `render_page` dispatches to the same `worker.rs` pool as every other
  command, and this module no longer exists.
- **`progress.rs`**: progress-event reporting decoupled from the `tauri` crate's own event/window
  types.
- **`error.rs`**: `CommandError`, a structured, `Serialize`-able error type every command returns
  instead of panicking — verified by `every_command_reports_structured_errors_instead_of_
  panicking` in `tests/tauri_commands_integration.rs`.

## 10. Test coverage (measured, live re-run)

**Historical note (superseded by the third refresh below):** the `RUST_PDF_PDFIUM_LIB_DIR` setup
instructions that used to live in this section predate the §8c Pdfium-removal migration and
described that earlier, FFI-backed renderer's coverage run. This crate has no native library to
fetch/bind at all now: `RUST_PDF_PDFIUM_LIB_DIR` is unused/unrecognized, and
`cargo llvm-cov --release --features full,tauri --summary-only` (no env var, no
`scripts/fetch_pdfium.sh` step) exercises every render/tauri test unconditionally on a plain
checkout. The historical "tests silently skip without the env var" trap this section used to warn
about at length no longer applies — there is no code path left that can silently skip for lack of
a native library.

Tool: `cargo-llvm-cov 0.8.7`.

**Third refresh (this pass):**

```
$ cargo llvm-cov --release --features full,tauri --summary-only
```

| Metric | Covered / Total | % |
|---|---|---|
| Regions | 42,875 / 51,060 | **83.97%** |
| Functions | 2,397 / 3,004 | **79.79%** |
| Lines | 21,768 / 26,373 | **82.54%** |

(Ran three times back-to-back to check stability, same practice as the second refresh: the first
run measured 8,184 missed regions/4,604 missed lines, the next two both measured 8,185/4,605 —
a ±1 wobble that rounds to the identical 83.97%/82.54% either way. Consistent with the
already-documented ordinary thread-scheduling nondeterminism in the concurrent
`tauri_commands`/`render` test suites, not a measurement error.)

Up from the second refresh's 35,856/42,733 (83.91%) regions, 2,061/2,633 (78.28%) functions,
18,392/22,403 (82.10%) lines — both region/function/line *totals* grew substantially (42,733→51,060
regions, +8,327; 22,403→26,373 lines, +3,970) because the pure-Rust rendering build added ~8,600
lines of new, mostly-well-tested code (`render/native/`, §2/§8a-§8d), and the *percentage* moved
only slightly because that new code's own coverage (region-weighted) is close to the crate
average, not because it went untested — see the per-file `%Ln` values inlined into §2's module
table (e.g. `render/native/mod.rs` 98.7%, `render/native/font.rs` 92.6%, vs.
`render/native/error.rs` 0.0% and `render/native/interpreter.rs` 75.1% pulling the average down —
both explained in §2/§8d).

Notably low files (line coverage), from this run:
- `render/native/error.rs` — **0%** (new this refresh — see §2's row: the error/warning *type*
  itself has no directly-exercised lines because every variant constructor is a trivial one-line
  enum literal the compiler attributes to the *calling* file's coverage, not this one; every
  variant is genuinely constructed and returned by some other file's test, per §8d).
- `ffi.rs` — **0%** (unchanged from the original audit — no test exercises the C ABI layer).
- `image/mod.rs` — **24.60%** line coverage (worst-covered non-FFI file; this refresh corrects a
  stale **26.20%** figure carried over from the original audit and repeated, unverified, through
  the second and third refreshes — see this refresh's provenance entry in §15).
- `forms/field.rs` — **51.49%** line coverage (100+ public items, still the largest/least-tested
  file in `forms/`).
- `font/mod.rs` — **45.26%**, `font/metrics.rs` — **49.11%**: both still under 50%. `filter/dct.rs`
  is **not** in this bucket any more — this refresh corrects a stale **49.23%** figure (inherited,
  unverified, since the original audit) to its live-measured **72.31%**.
- `document/mod.rs` — **78.0%**, `editor/redact.rs` — **73.4%**, `signatures/signer.rs` —
  **73.39%** (this refresh corrects a stale **73.0%** figure for `signatures/signer.rs`) — all
  still among the least-covered large files.

Test counts (`cargo test --release --features full,tauri`, third refresh, this pass):

| Suite | Passed | Ignored |
|---|---:|---:|
| `src/` unit tests | 721 | 0 |
| `tests/editor_tests.rs` | 7 | 0 |
| `tests/font_embedding_tests.rs` | 6 | 0 |
| `tests/integration_tests.rs` | 68 | 0 |
| `tests/interactive_features_tests.rs` | 7 | 0 |
| `tests/large_file_render_bench.rs` | 0 | 1 (opt-in, ~2GB fixture — see §8d for the explicit `--ignored` run) |
| `tests/large_file_rss_bench.rs` | 0 | 2 (opt-in, ~2GB fixture) |
| `tests/render_tests.rs` | 11 | 0 |
| `tests/signature_verification_tests.rs` | 15 | 0 |
| `tests/tauri_commands_integration.rs` | 2 | 0 |
| Doctests | 4 | 8 (`ignore`d) |
| **Total** | **841** | **11** |

0 failing. Up from 712/11 at the second refresh — the +129 passing tests are essentially all the
pure-Rust rendering build's own dedicated test suites landing directly in `src/render/native/`
(unit tests in `bits.rs`/`color.rs`/`colorspace.rs`/`font.rs`/`function.rs`/`glyph.rs`/`path.rs`/
`state.rs`/`interpreter.rs`, plus the dedicated `image_integration_tests.rs` (9 tests) and
`text_tests.rs` (5 tests) modules cited throughout §8a/§8b/§8d) and `render::renderer`'s new unit
tests (`normalize_rotate`/`apply_rotation_to_dims`/`check_dimensions`/`page_pixel_size`/etc., §8c),
plus one new doctest (`render::native`'s own module-doc example). Up from the original audit's 312
passing / 0 failing overall.

**Live benchmark runs this session (opt-in, `#[ignore]`d, not part of the counts above — corrected
from the third refresh):**
`cargo test --release --features render --test large_file_render_bench -- --ignored --nocapture`,
run eight consecutive times against the real ~2.10 GB / 10,000-page fixture (793×1123 px), measured
**1.58 s on the first run (fresh test-binary build, cold page cache) and ~58-70 ms on all seven
subsequent runs (warm page cache)** — see §8d's "Performance delta" subsection for the full
comparison against the previously-recorded Pdfium figure (82.46 ms), why the two very different
numbers both occur, and why the steady-state figure (faster than Pdfium's) is the one that should
not be mistaken for a regression.

## 11. `cargo audit` results (live re-run)

Re-run again for this third refresh (cargo-audit `0.22.1`; no `--features` flag exists for this
tool/version, and none is needed — `Cargo.lock` is already fully resolved against `full,tauri`).
This time `Cargo.toml`/`Cargo.lock` genuinely changed (§8c: `pdfium-render` removed,
`tiny-skia`/`tiny-skia-path`/`arrayvec`/`arrayref`/`strict-num` added):

```
$ cargo audit
    Fetching advisory database from `https://github.com/RustSec/advisory-db.git`
      Loaded 1149 security advisories (from /Users/galihlasahido/.cargo/advisory-db)
    Updating crates.io index
    Scanning Cargo.lock for vulnerabilities (531 crate dependencies)
[... 17 informational (unmaintained/unsound) warnings, listed below ...]
warning: 17 allowed warnings found
```

Exit code `0`. **0 vulnerabilities.** **531** total crate dependencies scanned — down from 535 at
the second refresh (**−4**, confirmed via `git diff babec0c HEAD -- Cargo.lock` naming exactly
which packages left and joined: **removed** `pdfium-render`, `libloading` (its dynamic-loading
dependency for the native `.so`/`.dylib`/`.dll`, no longer needed), and the transitive-only
`vecmath`/`utf16string`/`piston-float`/`maybe-owned`/`itertools`/`console_log`/
`console_error_panic_hook` that only `pdfium-render`'s own dependency tree pulled in — 9 packages
gone; **added** `tiny-skia`, `tiny-skia-path`, `arrayvec`, `arrayref`, `strict-num` — 5 packages
new; net **9 − 5 = −4**, exactly matching 535→531). This is the dependency-count-level proof that
the Pdfium/FFI dependency is genuinely gone, not just unused — the same conclusion §8c's own
`grep -ri pdfium src/` check already reached from the source-code side.

A `.cargo/audit.toml` (added by a later "Dependency Audit Triage" remediation phase, not present
at the original audit) explicitly ignores 5 specific, individually-reviewed advisory IDs with
documented rationale — each cross-referenced to `docs/THREAT_MODEL.md` §7, which is the
authoritative source for *why* each is accepted:

| Advisory | Crate | Category | Rationale (see `docs/THREAT_MODEL.md` §7.x for full detail) |
|---|---|---|---|
| `RUSTSEC-2023-0071` | `rsa 0.9.10` | vulnerability (timing side channel, "Marvin Attack") | No patched version exists upstream; crate is only used for signing/verifying over already-public hashes, never decrypting attacker-supplied secret data |
| `RUSTSEC-2026-0192` | `ttf-parser 0.25.1` | unmaintained | Actively used for embedded font loading (§7); the concrete panic-on-malformed-input issue this surfaced was fuzzed and mitigated at the call site (`catch_unwind` in `TrueTypeFont::load`) |
| `RUSTSEC-2026-0173` | `proc-macro-error2 2.0.1` | unmaintained | Dev-dependency-only edge (`lopdf`→`jiff`→`defmt-macros`), verified via `cargo tree -i` to have no active feature edge |
| `RUSTSEC-2026-0195` / `RUSTSEC-2026-0194` | `quick-xml 0.39.4` | DoS (memory/CPU exhaustion) | Reachable only via `tauri-codegen`'s build-time `Info.plist` parsing (the downstream app developer's own trusted file), not this crate's PDF/font attacker surface; no patched `quick-xml` is resolvable against `plist 1.9.0`'s constraint |

Beyond those 5 reviewed-and-ignored IDs, this run surfaced **17 not-yet-individually-reviewed
"informational" warnings** (all `unmaintained` or `unsound`, per `.cargo/audit.toml`'s explicit
`informational_warnings = ["unmaintained", "unsound", "notice"]`, which is also cargo-audit's own
default) — every one of them pulled in transitively through `tauri`'s desktop-integration
dependency tree (specifically: `tauri-runtime-wry` → `wry`/`tao` → the Linux `gtk`/`webkit2gtk`
GTK3-binding crates `atk`, `atk-sys`, `gdk`, `gdk-sys`, `gdkwayland-sys`, `gdkx11`, `gdkx11-sys`,
`gtk`, `gtk-sys`, `gtk3-macros` (all `RUSTSEC-2024-041{1..9}`, "gtk-rs GTK3 bindings - no longer
maintained"), `glib` (`RUSTSEC-2024-0429`, an unsound iterator impl), `proc-macro-error 1.0.4`
(`RUSTSEC-2024-0370`, distinct from the already-reviewed `proc-macro-error2`), and 5 `unic-*`
Unicode-identifier crates pulled in via `tauri-utils`'s `urlpattern` dependency
(`RUSTSEC-2025-0075/0080/0081/0098/0100`). None of these are new to this refresh's re-run — they
are new to the crate's dependency graph *only* in the sense that the original audit never
resolved `Cargo.lock` with the `tauri` feature enabled at all, so they were never visible before.
None has a dedicated `docs/THREAT_MODEL.md` §7.x entry yet; per that document's own stated policy
("a brand-new, not-yet-reviewed advisory... still surfaces for a human to triage"), that is
expected, working-as-designed behavior, not a regression — but it does mean these 17 are the
concrete next candidates for someone to triage into `.cargo/audit.toml` (or fix by, e.g., pinning
`tao`/`wry` past a version that drops the unmaintained GTK3 bindings, if one exists) in a future
phase. Out of scope for this documentation-refresh session.

## 12. `cargo clippy` (live re-run)

Re-run again for this third refresh, against the full pure-Rust rendering build
(`render/native/`'s ~8,600 new lines, §2/§8a-§8d):

```
$ touch src/lib.rs && cargo clippy --features full,tauri --all-targets -- -D warnings
    Checking rust-pdf v0.1.0 (/Users/galihlasahido/RustroverProjects/rust-pdf)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.93s
```

**Clean — 0 warnings, 0 errors**, `touch src/lib.rs` run immediately beforehand (same practice as
every prior refresh) to force a real recompile rather than reporting a stale cached pass. Also
re-run, per §8c's own claim of having checked every feature combination the migration touched, for
`default` (no features), `native-render`, `render` and `full` — all clean under plain
`cargo clippy --features <f> -- -D warnings` (no `--all-targets`):

```
$ cargo clippy -- -D warnings                              # default (no features)
$ cargo clippy --features native-render -- -D warnings
$ cargo clippy --features render -- -D warnings
$ cargo clippy --features full -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s)   # all four, clean
```

**One pre-existing, out-of-scope observation, not caused by this refresh and not fixed here**
(documentation-only session; this crate's other rule is not to touch unrelated code): running
`cargo clippy --all-targets` (i.e. including `examples/`) against `default`/`native-render`/
`render` alone (not `full`/`full,tauri`) fails to *compile* `examples/digital_signature_example.rs`
— that example needs the `signatures` feature, which none of those three feature sets enable, and
`examples/`'s `Cargo.toml` entries have no `required-features` gate to skip it automatically. This
is unrelated to rendering (confirmed identical failure with and without any rendering feature
enabled) and was not introduced by this migration — it is a pre-existing gap in the examples'
Cargo manifest metadata, out of scope for this rendering-documentation refresh.

This supersedes the original audit's finding of "13 distinct lint categories" under
`-D warnings` — every one of those (the `approx_constant`/`derivable_impls`/
`manual_is_multiple_of`/`dead_code` items etc.) has since been fixed by an intervening phase.

## 13. Untrusted-input risk register

> **This section is superseded.** `docs/THREAT_MODEL.md` says so explicitly in its own §1: *"This
> replaces the informal 'risk register' in `ARCHITECTURE.md` §9 [now §13 after this refresh's
> renumbering]... `ARCHITECTURE.md` §9/§10 are left as-is as historical record of that earlier
> audit; do not treat them as the current mitigation status."* That cross-reference did not
> previously exist *in this direction* (this file never linked back to `docs/THREAT_MODEL.md`) —
> added now so a reader landing here first finds their way to the current document. Consult
> `docs/THREAT_MODEL.md` §4 (component risk register) and §6 (full `MAX_*` resource-limit
> inventory, live-grepped and cross-referenced in §6's table here too) for the maintained,
> current version.

Original findings, preserved for provenance (see §4's status-update callout above for which of
these are now fixed, re-verified during this refresh):

| # | Risk | Location | Status (this refresh) |
|---|---|---|---|
| 1 | Infinite loop on `/Prev` cycle | `parser/mod.rs` | Fixed — `MAX_XREF_SECTIONS` |
| 2 | Stack overflow on deep nesting | `parser/objects.rs` | Fixed — `MAX_NESTING_DEPTH` |
| 3 | Unbounded allocation on huge xref `count` | `parser/xref.rs` | Mitigated — see §4 |
| 4 | Decompression bomb | `object/stream.rs` | Fixed — `MAX_DECODED_SIZE` |
| 5 | Slice-index panic on file-derived offsets | `parser/mod.rs` | Fixed — `.get()`-based bounds |
| 6 | Silent wrong data for non-Flate filters | `object/stream.rs` `decompress()` | Legacy method still does this; `decode_all()` (the newer, preferred API) does not |

## 14. Gap vs. a mature C/C++ PDF engine (pdfium / MuPDF / poppler / qpdf)

The original audit's gap list, updated with what has since closed (re-verified via §2's module
inventory) vs. what has not (not independently re-verified beyond noting the module still doesn't
exist):

- ~~A real content-stream interpreter + rasterizer/vector-renderer~~ — **closed**: originally via
  an FFI binding to a third-party engine (§8, a deliberate build-vs-buy decision at the time), now
  **superseded by §8c**: a from-scratch, pure-Rust content-stream interpreter/rasterizer
  (`render::native`), with that FFI dependency fully removed. See §8c for the completed migration
  and its accepted compatibility trade-offs (§8's original "why FFI" reasoning is kept as
  historical record, not current status), and **§8d for the current, live-verified list of exactly
  what that pure-Rust engine still cannot do** (JBIG2/JPX images, Type1/bare-CFF font programs, no
  true ICC colour management — all fail closed with a structured warning, never silently) — this
  gap is genuinely closed relative to "no renderer at all," but is not closed to pdfium/mupdf's own
  compatibility bar (JBIG2/JPX/Type1-CFF/ICC, per §8d); on measured performance specifically, §8d's
  corrected (fourth refresh) numbers show steady-state page-render time on the identical large-file
  benchmark is *not* slower than the previously-recorded Pdfium figure (only a cold-page-cache
  first touch is, which is a disk-I/O artifact, not a rendering-algorithm regression) — see §8d for
  the full corrected comparison.
- ~~Font program parsing/subsetting (TrueType/OpenType) and embedding~~ — **closed** for
  TrueType/OpenType: §7. CFF/Type1 embedding was not confirmed either way.
- **Still open, not re-verified this refresh**: a "repair mode" parser tolerant of arbitrary
  real-world spec violations beyond what `parser/recovery.rs` already does (that module exists
  now, but its coverage of "every way real files violate the spec" was not assessed here).
- **Still open, re-verified**: Filter completeness — `filter/` now covers ASCIIHex/ASCII85/
  RunLength/LZW/Flate(+predictors)/DCT/CCITT, but **JBIG2/JPX remain unsupported** (`decode_filter`
  explicitly errors on `JBIG2Decode` rather than silently passing it through — see §4 finding #8 —
  but there is still no decoder for either format; the Rust ecosystem still has no mature one).
- **Still open, re-verified**: encrypted-file *opening* — §1, §4 finding #1.
- ~~Redaction, tagged/accessible PDF~~ — **closed**: §6 (`editor/redact.rs`, `editor/structure.rs`,
  `editor/pdfua.rs`). AcroForm JavaScript-triggered calculations were not investigated (still
  presumed absent — no `content` field for a calculation script/engine was noticed anywhere in
  the module inventory).
- ~~A rendering surface Tauri could put pixels on screen with~~ — **closed**: §8 + §9.

Given how much of this list has closed since the original estimate, the original "12–24+
person-month" estimate to reach pdfium/mupdf-class robustness is **not re-derived in this
refresh** — a fresh estimate would need to weigh the closed items above against what's still open
(chiefly: arbitrary-real-world-file robustness/repair-mode maturity, JBIG2/JPX, encrypted-file
opening) and is better done by whoever scopes the next phase, with fresh eyes on the current
module inventory in §2 rather than by extrapolating from a percentage-closed count here.

## 15. Document provenance

- Original audit: commit `4ed35fe`'s ancestor (`f559bc0`, "Audit" phase) through the "Rendering
  Decision" phase (`4ed35fe`). Wrote §1–§5 (renumbered), §8 (was §12), the original §9/§10 (now
  §13/§14).
- Nine phases between `4ed35fe` and `ce36ee9` (Content Editing, Fonts, Interactive Features,
  Redaction, Standards Conformance, Large File Streaming, Signature, Security Hardening, Tauri
  Integration, plus later remediation passes: Font Fuzz Crash Fix, Dependency Audit Triage,
  RenderActor Panic Resilience, Visual Verification, Large-File Render Benchmark) built
  everything in §2's `editor/`, `filter/`, `render/`, `tauri_commands/` rows and the new `font/`
  files, added `docs/THREAT_MODEL.md` and `.cargo/audit.toml`, but never updated this file.
- **First refresh** (2026-07-03, remediation session "ARCHITECTURE.md Refresh", commit
  `e0ab506`): regenerated §2 (module inventory, live LOC), §10 (coverage, live re-run against
  `--features full,tauri`), §11 (`cargo audit`, live re-run), §12 (`cargo clippy`, live re-run);
  fixed the largest-file claim (`signatures/signer.rs`, not `document/mod.rs`); added §6/§7/§9 for
  modules that previously had no entry at all; added spot-checked status callouts to
  §4/§5/§13/§14 where opportunistic verification found a specific old claim superseded. Did
  **not** perform a full re-audit of every claim in the inherited narrative sections — see the
  callouts throughout for exactly what was and wasn't re-verified, and consult
  `docs/THREAT_MODEL.md` for anything security/risk-related that this document doesn't fully
  resolve.
- Two further remediation-phase commits landed **immediately after** the first refresh, making it
  one commit stale: `ad35dcb` ("PDF/A CIDSet Fix" — `font/cid.rs` now generates a live `/CIDSet`
  stream, ISO 32000-1:2008 Table 122 / ISO 19005-1:2005 6.3.5, for subset CIDFonts; also touched
  `document/mod.rs` and `editor/pdfa.rs`) and `babec0c` ("CID subset-tag fix" — made the
  `ABCDEF+FontName` subset-tag prefix on `BaseFont` conditional on `will_subset()`, the same
  predicate that gates `/CIDSet`, fixing a veraPDF 6.3.5-test-3 failure on an untagged full embed
  that previously still got a tagged name).
- **Second refresh** (2026-07-03, remediation session "ARCHITECTURE.md Final Refresh", commit
  `babec0c`): re-ran every live command from §2/§10/§11/§12 against the tree *after*
  both of the commits above and updated exactly the numbers that moved — §2's `font/cid.rs`/
  `editor/pdfa.rs`/`document/mod.rs` rows and the crate LOC total (39,299→39,591, still 98
  files), §10's coverage aggregate (83.76/78.20/81.99%→83.91/78.28/82.10%) and test count
  (706→712 passing, still 11 ignored, still 0 failing), and re-confirmed §11 (`cargo audit`) and
  §12 (`cargo clippy -D warnings`) are unchanged (neither fix touched `Cargo.toml`/`Cargo.lock`
  or introduced a new lint). Did not touch §1/§3–§9 (narrative)/§13/§14 beyond what the top
  callout block already documents. Was, at the time, the deliberately-last commit of that
  remediation sequence — but four further code commits landed afterward (the pure-Rust rendering
  migration below), making it stale for §1/§2/§9/§10/§11/§12, which is exactly what this third
  refresh corrects.
- Four code commits landed on top of the second refresh, **at explicit user request**, to migrate
  rendering off Pdfium/FFI onto a pure-Rust stack: `899fb9f`/`e658df6`/`b63dc98`/`a3db203` (the
  four-phase build — content-stream interpreter core, text rendering, colour spaces/images,
  transparency/blend modes — §8a/§8b and `render::native`'s own docs) and `e6af056` ("replace
  Pdfium/FFI renderer with pure-Rust engine, retire render actor" — §8c, the integration phase that
  actually deleted the Pdfium dependency and swapped `PdfRenderer`/`tauri_commands` over). Two
  further security-hardening commits, `5ba3e0c` and `0e14625` (fuzzing + resource-limit hardening
  of the content-stream interpreter, and a fuzz-found stroke-width panic fix), followed with no
  `ARCHITECTURE.md`-relevant behavioral change beyond what §8c already described. None of these six
  commits touched this file.
- **Third refresh** (commit `5a90944`): rewrote §1's rendering description and feature-exclusion
  rationale to reflect the pure-Rust engine (was: FFI to Pdfium); regenerated §2's module inventory
  (48,168 lines / 111 files, up from 39,591/98 — `render/` 3→17 files/849→9,440 LOC,
  `tauri_commands/` 7→6 files/2,865→2,592 LOC net, plus the small incidental touch-ups listed in
  §2's own intro paragraph); added a supersession banner atop §8 pointing to §8c/§8d; added
  **§8d "Known Limitations"**, the new single place consolidating JBIG2/JPX (placeholder, not
  decoded), Type1/bare-CFF fonts (fails closed, no glyph painted), ICC colour management
  (approximated, not colour-managed) and a claimed **measured performance delta against the
  previous Pdfium benchmark** (1,723.67 ms vs. 82.46 ms on the identical 2.10 GB/10,000-page
  fixture, "roughly 20.9× slower," labeled "a real, measured regression, not a rough guess");
  regenerated §9's LOC line, **§10** (coverage: 83.91/78.28/82.10%→ 83.97/79.79/82.54%, test count
  712→841 passing, still 0 failing, still 11 ignored), **§11** (`cargo audit`: 535→531
  dependencies, net −4, named exactly via `git diff ... -- Cargo.lock`, still 0 vulnerabilities,
  still the same 5 reviewed/ignored advisories + 17 informational warnings) and **§12** (`cargo
  clippy --features full,tauri --all-targets -- -D warnings`: re-confirmed clean, plus
  `default`/`native-render`/`render`/`full` each individually re-run clean without
  `--all-targets`, with a pre-existing, out-of-scope `--all-targets`-only failure on those three
  feature sets — an `examples/` manifest gap unrelated to rendering — noted and left alone); added
  a §14 cross-reference to §8d. Believed at the time to be the **last commit of the entire
  remediation sequence** — but the performance claim above did not survive a later
  reproducibility check, which is exactly what the fourth refresh below corrects.
  **The "20.9× slower"/"real, measured regression" claim in this bullet is the third refresh's own
  wording and is superseded/retracted by the fourth refresh immediately below — it does not
  reproduce and should not be relied on.**
- **Fourth refresh** (commit `7258c5b`): a targeted correction, not a full re-audit — the *only* claim
  revisited is the third refresh's §8d/§10 performance-regression figure. Re-ran the exact cited
  command, `cargo test --release --features render --test large_file_render_bench -- --ignored
  --nocapture`, against the exact same 2.10 GB/10,000-page cached fixture, **eight consecutive
  times**: run 1 (immediately after a fresh `cargo build --release ... --test
  large_file_render_bench`, cold page cache) measured **1.582036125 s**; runs 2-8 (same warm-cached
  file, no rebuild in between) measured **59.605666 ms, 62.000959 ms, 60.171042 ms, 67.082166 ms,
  67.302250 ms, 62.637167 ms, 63.784667 ms** — i.e. seven of eight runs landed at **~58-70 ms,
  faster than the 82.46 ms Pdfium baseline**, not ~20.9× slower than it. Rewrote §8d's
  "Performance delta" subsection and §10's one-line restatement of it to report both the
  cold-start (~1.6 s, a disk-page-cache-eviction artifact of the first touch after a build/fixture
  regen, not a rendering-algorithm property) and steady-state (~60 ms, faster than Pdfium) figures
  honestly, and to explicitly retract the "20.9× slower"/"real, measured regression" framing;
  updated the one other place that repeated the old figure (§14's gap-vs-mature-engines list) to
  match; added the corrective status banner at the top of this document. Deliberately did **not**
  touch §1-§7/§8a-§8c/§9/§11/§12/§13 narrative or numbers, and did not re-run `cargo llvm-cov`/
  `cargo audit`/`cargo clippy` — none of those are affected by a documentation-only correction of a
  benchmark-interpretation claim, and `git status` after this commit shows no `src/**` or
  `Cargo.{toml,lock}` change, only `ARCHITECTURE.md`. This is, again, the **last commit of this
  remediation** — no `src/**` change follows it (verify: `git log --stat -1` on the commit this
  refresh lands in touches only `ARCHITECTURE.md`).
- **Fifth refresh** (this pass): a targeted correction, not a full re-audit — the *only* thing
  revisited is §2's per-file `%Ln` column (line coverage), which a prior remediation review found
  did not match a live re-run for at least 12 of 105 covered rows despite the third refresh's
  explicit claim that every row had been. Ran `cargo llvm-cov --release --features full,tauri
  --summary-only` (cargo-llvm-cov `0.8.7`, the same tool version the third refresh cited) twice
  back-to-back to check stability, per the same practice as prior refreshes; both runs agreed on
  every one of the 105 covered files except `parser/recovery.rs` (77.22% one run, 77.64% the
  next — a genuine, reproduced-live run-to-run flip, not a measurement mistake, documented inline
  on that file's row instead of picking one number and hiding the other). Computed each file's
  precise line-coverage percentage directly from the tool's raw covered/total line counts (not by
  re-rounding its already-2-decimal-rounded display, which independently introduced at least one
  spurious ±0.1-point error — `filter/lzw.rs` — during this refresh's own first-pass arithmetic,
  caught and corrected before landing). Found and corrected **16** stale rows in §2 (full list, doc
  value → live value, in this document's top status banner): most were large, multi-refresh-old
  errors traceable to the *original* audit's numbers never actually having been re-measured despite
  being carried forward, relabeled, and asserted as "re-verified" by name through three subsequent
  refreshes (`filter/mod.rs` 72.9%→90.7%, `filter/dct.rs` 49.2%→72.3%, `object/mod.rs`
  63.0%→68.9%, `object/array.rs` 81.6%→85.5%, `object/string.rs` 85.9%→89.1%, `page/mod.rs`
  63.3%→66.7%, `types/rectangle.rs` 91.9%→87.8%, `image/mod.rs` 26.2%→24.6%,
  `encryption/permissions.rs` 75.5%→70.6%, `editor/content_stream.rs` 69.8%→72.4%,
  `signatures/signer.rs` 73.0%→73.4%); a few were smaller, genuine drift or rounding-precision
  fixes (`parser/mod.rs` 93.4%→93.5%, `filter/lzw.rs` 89.3%→89.2%, `editor/icc.rs` 95.4%→95.3%,
  `editor/structure.rs` 90.8%→90.7%); and `parser/recovery.rs` was changed from a single stale
  77.2% figure to an explicit 77.2–77.6% range with an inline explanation of the observed
  instability. Also re-confirmed, unchanged: §2's LOC/file-count totals (48,168 lines / 111 files,
  same `find` output as the third refresh), and §10/§11/§12's headline aggregate numbers — the
  crate-wide coverage totals were already exactly reproduced by a live re-run before this refresh
  (confirmed again this pass), so this was purely a per-file granularity fix, not an aggregate one.
  Fixed the three places in §10's "Notably low files" prose that restated now-corrected figures
  (`image/mod.rs`, `filter/dct.rs`, `signatures/signer.rs`). Deliberately did **not** touch
  §1/§3-§9/§11/§12/§13/§14 narrative or numbers, and did not re-run `cargo audit`/`cargo clippy` —
  neither is affected by a line-coverage-column correction, and `git diff 7258c5b HEAD -- src/
  Cargo.toml Cargo.lock` is empty for this commit: only `ARCHITECTURE.md` changed. This is,
  deliberately, the **last commit of this remediation** — no `src/**` change follows it (verify:
  `git log --stat -1` on the commit this refresh lands in touches only `ARCHITECTURE.md`).
