# rust-pdf — Architecture & Audit (as of 2026-07-03)

> **Status of this document (second refresh, current — supersedes the first refresh's numbers
> below for §2/§10/§11/§12):** regenerated again at commit `babec0c` (branch
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
fonts for CJK, subsetting), a **PDF rasterizer** (`render/`, FFI to Google's Pdfium — see §8), and
a **Tauri desktop-app command layer** (`tauri_commands/`, nine async commands wired to a worker
pool and a dedicated render actor thread).

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
aspirational surface area, not implemented functionality. `render` and `tauri` are deliberately
**not** part of `full` (see §8/§9 — both require bundling a native platform-specific binary /
desktop-app runtime that a pure structural/generation/signing consumer does not need).

## 2. Module inventory (src/)

**39,591 lines of Rust across 98 files** (live count, re-verified for the second refresh at
commit `babec0c`: `find src -name '*.rs' | xargs wc -l`; up from 39,299 lines / 98 files at the
first refresh (`e0ab506`) — the `+292` lines are entirely `font/cid.rs` (+225),
`editor/pdfa.rs` (+51) and `document/mod.rs` (+16), the three files touched by the
`ad35dcb`/`babec0c` CIDSet/subset-tag fixes; no file was added or removed), up from 16,427
lines / 51 files at the original audit — more than double the codebase. Grouped by directory
below; `%Ln` is line coverage from the live `cargo llvm-cov --release --features full,tauri`
re-run in §10 (not a stale carry-over) — three rows (`font/cid.rs`, `editor/pdfa.rs`,
`document/mod.rs`) changed since the first refresh; every other row is identical to before,
re-confirmed by this refresh's own re-run rather than assumed unchanged.

| File | LOC | %Ln | Purpose |
|---|---:|---:|---|
| `lib.rs` | 339 | 100.0 | Crate root, `prelude` re-exports, doctests |
| `error.rs` | 555 | — | `thiserror`-based error taxonomy, one enum per subsystem |
| `ffi.rs` | 157 | 0.0 | C ABI: `unsafe extern "C"` functions exposing the *write* path only |
| **`types/`** (4 files, 441 LOC) | | | Geometry primitives |
| `types/mod.rs` | 9 | — | Re-exports |
| `types/matrix.rs` | 189 | 86.8 | 2D affine transform matrix |
| `types/object_id.rs` | 101 | 100.0 | `ObjectId` (object number + generation) |
| `types/rectangle.rs` | 142 | 91.9 | `/MediaBox`-style rectangles |
| **`color/`** (4 files, 533 LOC) | | | Device color spaces |
| `color/mod.rs` | 173 | 85.2 | `Color` enum, conversions |
| `color/rgb.rs` | 159 | 89.7 | DeviceRGB |
| `color/cmyk.rs` | 119 | 81.8 | DeviceCMYK |
| `color/gray.rs` | 82 | 78.6 | DeviceGray |
| **`object/`** (6 files, 1,572 LOC) | | | The PDF object model, used by write + (partial) read paths |
| `object/mod.rs` | 282 | 63.0 | `Object` enum, top-level dispatch |
| `object/dictionary.rs` | 190 | 91.1 | `PdfDictionary` |
| `object/array.rs` | 141 | 81.6 | `PdfArray` |
| `object/name.rs` | 199 | 67.2 | `PdfName` |
| `object/string.rs` | 160 | 85.9 | `PdfString` (literal/hex, escaping) |
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
| `font/encoding.rs` | 115 | 57.1 | WinAnsi/MacRoman/Standard text encodings |
| `font/truetype.rs` | 795 | 96.5 | Embedded TrueType/OpenType font loading via `ttf-parser` (feature `fonts`) |
| `font/cid.rs` | 796 | 92.2 | Type 0 / CIDFontType2 composite fonts for embedded + CJK text; +225 LOC since the first refresh — `ad35dcb` added `/CIDSet` generation (ISO 32000-1 Table 122 / ISO 19005-1 6.3.5, +2 unit tests) and `babec0c` made the subset-tag prefix conditional on the same `will_subset()` predicate (+3 unit tests) |
| `font/subset.rs` | 95 | 96.9 | Font subsetting via the `subsetter` crate |
| `font/tounicode.rs` | 391 | 92.0 | `/ToUnicode` CMap generation for text extraction/accessibility |
| **`page/`** (1 file, 327 LOC) | | | |
| `page/mod.rs` | 327 | 63.3 | `Page`/`PageBuilder` |
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
| `image/mod.rs` | 319 | 26.2 | JPEG/PNG embedding via the `image` crate (worst-covered non-FFI file) |
| `image/xobject.rs` | 159 | 87.0 | `/XObject /Image` construction |
| **`filter/`** (8 files, 2,050 LOC) | | | Stream filter codecs (ISO 32000-1 §7.4) — new top-level module since the original audit |
| `filter/mod.rs` | 221 | 72.9 | `decode_filter`/dispatch, `MAX_DECODED_SIZE` |
| `filter/ascii_hex.rs` | 88 | 97.8 | `ASCIIHexDecode` |
| `filter/ascii85.rs` | 147 | 95.6 | `ASCII85Decode` |
| `filter/run_length.rs` | 115 | 90.3 | `RunLengthDecode` |
| `filter/lzw.rs` | 278 | 89.3 | `LZWDecode` |
| `filter/predictor.rs` | 474 | 88.2 | PNG/TIFF predictors (used by Flate/LZW) |
| `filter/dct.rs` | 125 | 49.2 | `DCTDecode` (baseline/progressive JPEG passthrough for image XObjects) |
| `filter/ccitt.rs` | 602 | 81.8 | `CCITTFaxDecode` (Group 3/4) |
| **`encryption/`** (4 files, 1,288 LOC) | | | Builds `/Encrypt` dictionaries; write-only (§1) |
| `encryption/mod.rs` | 296 | 90.6 | Orchestration |
| `encryption/config.rs` | 174 | 96.6 | `EncryptionConfig` |
| `encryption/key_derivation.rs` | 589 | 91.3 | RC4/AES-128/AES-256 key derivation (rev 2–6) |
| `encryption/permissions.rs` | 229 | 75.5 | `/P` permission bits |
| **`signatures/`** (9 files, 5,285 LOC) | | | Detached PKCS#7/CMS signing + verification + PAdES LTV — see §5 |
| `signatures/mod.rs` | 230 | 84.9 | Orchestration |
| `signatures/config.rs` | 240 | 93.4 | `SignatureConfig` |
| `signatures/certificate.rs` | 442 | 62.1 | X.509 cert parsing/build |
| `signatures/chain.rs` | 253 | 80.2 | Certificate chain (path) validation, `MAX_CHAIN_DEPTH` |
| `signatures/pkcs7.rs` | 619 | 86.0 | Hand-rolled minimal DER/PKCS#7 encoder |
| `signatures/signer.rs` | **1,825** | 73.0 | **Largest file in the crate.** `DocumentSigner` (fresh `Document`) + `IncrementalSigner` (ad-hoc plain-text scan of an existing PDF buffer — see original §4.3 finding, not re-verified this refresh) |
| `signatures/timestamp.rs` | 631 | 70.7 | RFC 3161 TSP client — PAdES "B-T" |
| `signatures/dss.rs` | 342 | 77.5 | `/DSS` Document Security Store embedding — PAdES "B-LT" |
| `signatures/verifier.rs` | 703 | 87.9 | `SignatureVerifier`, `/ByteRange` handling |
| **`parser/`** (7 files, 3,506 LOC) | | | nom-based structural PDF reader — see §4 |
| `parser/mod.rs` | 1,687 | 93.4 | `PdfReader` orchestrator; `MAX_XREF_SECTIONS`, `MAX_PAGE_TREE_WALK_*` |
| `parser/lexer.rs` | 404 | 85.5 | Tokenizer |
| `parser/objects.rs` | 381 | 88.2 | Object grammar; `MAX_NESTING_DEPTH` |
| `parser/trailer.rs` | 147 | 84.9 | Trailer + `/XRefStm` hybrid-reference handling |
| `parser/xref.rs` | 311 | 91.5 | Classic + xref-stream parsing |
| `parser/inline_image.rs` | 209 | 97.3 | `BI...ID...EI` inline image operator parsing |
| `parser/recovery.rs` | 367 | 77.2 | Repair-mode object scanning when the xref table is unusable; `MAX_RECOVERED_OBJECTS` |
| **`editor/`** (19 files, 10,685 LOC) | | | **New since the original audit.** In-place editing of an existing PDF — see §6 |
| `editor/mod.rs` | 88 | — | `EditableDocument` entry point |
| `editor/graph.rs` | 481 | 91.8 | Core mutable object graph; `MAX_PAGE_TREE_NODES`, `MAX_REACHABLE_OBJECTS` |
| `editor/pages.rs` | 710 | 82.9 | Insert/delete/reorder/rotate/split/merge page-tree editing |
| `editor/forms.rs` | 1,202 | 91.4 | Read/fill/create/flatten AcroForm fields on a loaded document |
| `editor/annotations.rs` | 779 | 95.2 | Markup annotations (highlight/underline/strikeout/free-text/stamp/ink/note) with generated appearance streams |
| `editor/outline.rs` | 646 | 91.9 | Document outline (bookmarks) + named destinations |
| `editor/structure.rs` | 432 | 90.8 | Minimal Tagged PDF logical structure tree (headings/paragraphs/tables/figures) |
| `editor/redact.rs` | 1,431 | 73.4 | Permanent content redaction (removes underlying content, not just a visual overlay) |
| `editor/audit.rs` | 339 | 98.2 | Redaction audit trail (documented private extension — ISO 32000 has no native object for this) |
| `editor/pdfa.rs` | 977 | 80.3 | PDF/A-1b/2b/3b validation + conversion; +51 LOC since the first refresh (`ad35dcb` narrowed `check_cidset_present`'s doc comment now that the gap it guards against is fixed, and split its regression test into a positive end-to-end-conformant case plus a hand-built-missing-`/CIDSet` defense-in-depth case — net +1 unit test) |
| `editor/pdfx.rs` | 261 | 88.6 | PDF/X colour-space constraint checking (ISO 15930) |
| `editor/pdfua.rs` | 372 | 98.4 | PDF/UA (ISO 14289-1) Matterhorn-Protocol-style checklist validation |
| `editor/icc.rs` | 385 | 95.4 | ICC output-intent embedding (used by PDF/A + PDF/X) |
| `editor/xmp.rs` | 366 | 94.0 | XMP metadata packet generation/embedding |
| `editor/content_ops.rs` | 331 | 78.0 | Insert/replace text/shapes/images on an existing page; `MAX_CONTENT_STREAM_BYTES` |
| `editor/content_stream.rs` | 615 | 69.8 | Round-trippable content-stream operator parser (used by content_ops/redact/text_extract) |
| `editor/text_extract.rs` | 305 | 85.7 | Text extraction from an existing page's content stream |
| `editor/save.rs` | 726 | 89.6 | Incremental update *or* full compacted rewrite (object streams + xref stream) |
| `editor/util.rs` | 239 | 94.3 | Shared helpers |
| **`render/`** (3 files, 849 LOC) | | | **New since the original audit.** Pdfium-FFI page rasterization, `render` feature — see §8 |
| `render/mod.rs` | 234 | 100.0 | Public API + module-level rationale doc (canonical rendering-decision writeup) |
| `render/renderer.rs` | 468 | 83.3 | `PdfiumLibrary`/`PdfRenderer`; `ffi_lock` mutex, `ManuallyDrop` close ordering. Coverage/tests here **require** `RUST_PDF_PDFIUM_LIB_DIR=.pdfium/<platform>/lib` to be set (see §8) — the render/tauri test suites *silently skip* (report `ok`, not `FAILED`) rather than exercise this file at all if it isn't, which this refresh initially missed on its first `cargo llvm-cov` run (got 7.9%/6.8% with the env var unset) before re-running with it set and getting the number in this row; see §10's note |
| `render/cache.rs` | 147 | 100.0 | Bounded LRU thumbnail cache |
| **`tauri_commands/`** (7 files, 2,865 LOC) | | | **New since the original audit.** Async Tauri desktop command layer, `tauri` feature — see §9 |
| `tauri_commands/mod.rs` | 112 | — | Module overview |
| `tauri_commands/commands.rs` | 1,516 | 82.3 | The 9 commands: `open_document`, `render_page`, `extract_text`, `search_text`, `apply_edit`, `save_document`, `fill_form`, `add_annotation`, `sign_document` |
| `tauri_commands/state.rs` | 254 | 87.3 | Managed app state: open-document registry + worker pool/render actor handles |
| `tauri_commands/worker.rs` | 217 | 91.7 | CPU-bound work thread pool (parsing/editing/signing off the async executor) |
| `tauri_commands/render_actor.rs` | 437 | 93.3 | Single dedicated OS thread owning every open `PdfRenderer`; panic containment (see original "RenderActor Panic Resilience" remediation) |
| `tauri_commands/progress.rs` | 92 | 100.0 | Progress event reporting, decoupled from the `tauri` crate's own types |
| `tauri_commands/error.rs` | 237 | 75.0 | `CommandError`, structured `Serialize`-able error taxonomy for the command layer |

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

## 8. Rendering: native-vs-FFI decision (implemented)

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

## 9. The Tauri command layer (`tauri_commands/`)

New top-level module since the original audit (7 files, 2,865 lines), gated by the `tauri`
feature (which pulls in `parser`, `render` and `signatures`). This is the glue between the pure
Rust library and a Tauri desktop application:

- **Nine async commands** (`tauri_commands/commands.rs`): `open_document`, `render_page`,
  `extract_text`, `search_text`, `apply_edit`, `save_document`, `fill_form`, `add_annotation`,
  `sign_document`. Every command follows a `..._impl` (plain async, testable without a live Tauri
  `AppHandle`) + thin Tauri-wired wrapper shape.
- **`state.rs`**: Tauri-managed application state — the registry of currently-open documents plus
  handles to the worker pool and render actor.
- **`worker.rs`**: a dependency-light thread pool that runs CPU-bound PDF parsing/editing/signing
  work off of Tauri's own async-command executor threads (verified by
  `no_blocking_of_a_single_threaded_executor` in `tests/tauri_commands_integration.rs`).
- **`render_actor.rs`**: a single dedicated OS thread owning every open document's `PdfRenderer`
  (Pdfium instances are not `Send` across arbitrary threads the way this actor pattern needs —
  see §8's concurrency trade-offs), with panic containment added in a later "RenderActor Panic
  Resilience" remediation phase (a panic inside a render call no longer takes down the actor
  thread or poisons subsequent renders).
- **`progress.rs`**: progress-event reporting decoupled from the `tauri` crate's own event/window
  types.
- **`error.rs`**: `CommandError`, a structured, `Serialize`-able error type every command returns
  instead of panicking — verified by `every_command_reports_structured_errors_instead_of_
  panicking` in `tests/tauri_commands_integration.rs`.

## 10. Test coverage (measured, live re-run)

Tool: `cargo-llvm-cov 0.8.7`.

```
$ RUST_PDF_PDFIUM_LIB_DIR=.pdfium/mac-arm64/lib cargo llvm-cov --release --features full,tauri --summary-only
```

**Note on `RUST_PDF_PDFIUM_LIB_DIR`:** the `render`/`tauri`-feature test suites (`tests/
render_tests.rs`, parts of `tests/tauri_commands_integration.rs`) are written to *skip* (report
`ok`, not `FAILED`) rather than exercise any Pdfium-touching code at all when the native library
can't be located (§8's "known limitation" — by design, so `cargo test --features render` still
passes in a checkout without the native binary). The first refresh's *initial* `cargo llvm-cov`
run did not set this variable, so all 11 `render_tests` silently skipped despite reporting `ok`,
and `render/renderer.rs` measured 6.77%/7.93% region/line coverage as a direct result — not a
reflection of real coverage. Re-running with `RUST_PDF_PDFIUM_LIB_DIR=.pdfium/mac-arm64/lib`
(after `scripts/fetch_pdfium.sh`, already present in this checkout) set, all 11 tests genuinely
executed (confirmed via `--nocapture`: `rendered 51 pages across 9 documents`). This is called
out at this length because it is exactly the kind of "tests report ok but didn't run the code"
trap the "jalankan sendiri... sertakan output aktual" rule exists to catch.

**Second refresh (this pass, commit `babec0c`, post-cid.rs-fix):** re-ran the same command with
the same env var. Aggregate (per-file breakdown folded into §2's module table above — every
row's `%Ln` column is from this run):

| Metric | Covered / Total | % |
|---|---|---|
| Regions | 35,856 / 42,733 | **83.91%** |
| Functions | 2,061 / 2,633 | **78.28%** |
| Lines | 18,392 / 22,403 | **82.10%** |

(Ran three times back-to-back to check stability: missed-region/-line counts wobbled by ±1–2
between runs — e.g. 6,876/6,877/6,878 missed regions, 4,010/4,011/4,012 missed lines — which
rounds to the same 83.9x%/82.1x% either way; the region/function totals (42,733 / 2,633) and the
percentages shown above were identical across all three runs. This is consistent with ordinary
thread-scheduling nondeterminism in the concurrent `tauri_commands`/`render_actor` test suites,
not a measurement error, and is well within the same ballpark the first refresh already reported
for the (then-unfixed) `font/cid.rs`/`editor/pdfa.rs` region.)

Up from the first refresh's 35,443/42,314 (83.76%) regions, 2,052/2,624 (78.20%) functions,
18,253/22,263 (81.99%) lines — the increase tracks the `+292` new lines in `font/cid.rs`/
`editor/pdfa.rs`/`document/mod.rs` (§2) plus their new tests, essentially all of it well-covered
(`font/cid.rs` alone: 90.1%→92.2%). This remains a large improvement over the original audit's
67.89% line coverage — most of the ~23,000 lines added across nine phases landed with dedicated
test suites (`tests/editor_tests.rs`, `tests/font_embedding_tests.rs`,
`tests/interactive_features_tests.rs`, `tests/render_tests.rs`,
`tests/signature_verification_tests.rs`, `tests/tauri_commands_integration.rs`) as part of each
phase's own DoD.

Notably low files (line coverage), from this run — unchanged from the first refresh (none of
these files were touched by the cid.rs fixes):
- `ffi.rs` — **0%** (unchanged from the original audit — no test exercises the C ABI layer).
- `image/mod.rs` — **26.20%** line coverage (unchanged finding from the original audit).
- `forms/field.rs` — **51.49%** line coverage (100+ public items, still the largest/least-tested
  file in `forms/`).
- `font/mod.rs` — **45.26%**, `font/metrics.rs` — **49.11%**, `filter/dct.rs` — **49.23%**: all
  three under 50%, none flagged in the original audit (they didn't exist, or `font/mod.rs` was
  differently shaped, at that time).
- `document/mod.rs` — **78.0%** (was 77.75% at the first refresh, +16 LOC from `ad35dcb`'s
  `cid_set_id` plumbing) — still not the crate's least-covered large file (`editor/redact.rs`
  73.4%, `signatures/signer.rs` 73.0%, §2/§4.2, and several others remain below it).

Test counts (`cargo test --release --features full,tauri`, same branch, commit `babec0c`):

| Suite | Passed | Ignored |
|---|---:|---:|
| `src/` unit tests | 593 | 0 |
| `tests/editor_tests.rs` | 7 | 0 |
| `tests/font_embedding_tests.rs` | 6 | 0 |
| `tests/integration_tests.rs` | 68 | 0 |
| `tests/interactive_features_tests.rs` | 7 | 0 |
| `tests/large_file_render_bench.rs` | 0 | 1 (opt-in, ~2GB fixture) |
| `tests/large_file_rss_bench.rs` | 0 | 2 (opt-in, ~2GB fixture) |
| `tests/render_tests.rs` | 11 | 0 |
| `tests/signature_verification_tests.rs` | 15 | 0 |
| `tests/tauri_commands_integration.rs` | 2 | 0 |
| Doctests | 3 | 8 (`ignore`d) |
| **Total** | **712** | **11** |

0 failing. Up from 706/11 at the first refresh — the +6 unit tests are exactly the ones
`ad35dcb` (+2, `build_subset_cid_set_has_correct_bit_layout`/
`build_subset_cid_set_marks_unused_gaps_within_the_used_range`, plus splitting one
`editor/pdfa.rs` test into two, net +1) and `babec0c` (+3,
`build_full_embed_uses_untagged_base_font_name`/
`build_with_no_glyphs_used_falls_back_to_untagged_full_embed`/
`build_subset_uses_tagged_base_font_name`) added — see §2's `font/cid.rs`/`editor/pdfa.rs` rows.
Up from the original audit's 312 passing / 0 failing overall.

## 11. `cargo audit` results (live re-run)

Re-run again for this second refresh at commit `babec0c` (cargo-audit `0.22.1`; no `--features`
flag exists for this tool/version, and none is needed — `Cargo.lock` is already fully resolved
against `full,tauri`, exactly as it was for the first refresh's run below, since neither
`ad35dcb` nor `babec0c` touched `Cargo.toml`/`Cargo.lock`):

```
$ cargo audit
    Fetching advisory database from `https://github.com/RustSec/advisory-db.git`
      Loaded 1149 security advisories (from /Users/galihlasahido/.cargo/advisory-db)
    Updating crates.io index
    Scanning Cargo.lock for vulnerabilities (535 crate dependencies)
[... 17 informational (unmaintained/unsound) warnings, listed below ...]
warning: 17 allowed warnings found
```

Exit code `0`. **0 vulnerabilities.** 535 total crate dependencies scanned — identical to the
first refresh's number, and more than 5× the original audit's 102, because that number was
measured on a build with no `render`/`tauri` feature (hence no `pdfium-render`, no
`tauri`/`wry`/`webkit2gtk`/GTK3-binding transitive tree at all) compiled in.

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

Re-run again for this second refresh at commit `babec0c` (i.e. against `font/cid.rs`,
`editor/pdfa.rs`, `document/mod.rs` as changed by `ad35dcb`/`babec0c`):

```
$ touch src/lib.rs && cargo clippy --features full,tauri --all-targets -- -D warnings
    Checking rust-pdf v0.1.0 (/Users/galihlasahido/RustroverProjects/rust-pdf)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.26s
```

**Clean — 0 warnings, 0 errors**, `touch src/lib.rs` run immediately beforehand (both this run
and the first refresh's below) to force a real recompile rather than reporting a stale cached
pass. Same result as the first refresh's run (reproduced verbatim below for provenance):

```
$ cargo clippy --features full,tauri --all-targets -- -D warnings
    Checking rust-pdf v0.1.0 (/Users/galihlasahido/RustroverProjects/rust-pdf)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.70s
```

This supersedes the original audit's finding of "13 distinct lint categories" under
`-D warnings` — every one of those (the `approx_constant`/`derivable_impls`/
`manual_is_multiple_of`/`dead_code` items etc.) has since been fixed by an intervening phase; the
first refresh did not identify which specific phase, only confirmed the current clean state, and
this second refresh only re-confirms it stayed clean after the cid.rs fixes.

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

- ~~A real content-stream interpreter + rasterizer/vector-renderer~~ — **closed**: §8, via Pdfium
  FFI (a deliberate build-vs-buy decision, not a from-scratch interpreter — see §8 for why that
  distinction still matters for the "setara Adobe Acrobat" framing).
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
  `babec0c`, this pass): re-ran every live command from §2/§10/§11/§12 against the tree *after*
  both of the commits above and updated exactly the numbers that moved — §2's `font/cid.rs`/
  `editor/pdfa.rs`/`document/mod.rs` rows and the crate LOC total (39,299→39,591, still 98
  files), §10's coverage aggregate (83.76/78.20/81.99%→83.91/78.28/82.10%) and test count
  (706→712 passing, still 11 ignored, still 0 failing), and re-confirmed §11 (`cargo audit`) and
  §12 (`cargo clippy -D warnings`) are unchanged (neither fix touched `Cargo.toml`/`Cargo.lock`
  or introduced a new lint). Did not touch §1/§3–§9 (narrative)/§13/§14 beyond what the top
  callout block already documents, since none of their claims depend on `font/cid.rs`'s exact
  line count or the coverage percentage of a single file. This refresh is the deliberately-last
  commit of the whole remediation sequence — no `src/**` change follows it.
