# rust-pdf — Architecture & Audit (as of 2026-07-02)

> **Status of this document:** objective inventory of the codebase as it exists today on branch
> `workflow/enterprise-buildout` (commit `fb6f8e9`). It is **not** a design proposal. Where the
> code diverges from what the README/marketing text implies, that is called out explicitly.
> No functional code was changed while producing this document (audit-only phase).

## 1. What this crate actually is today

`rust-pdf` is primarily a **PDF *generation* (write-only) library** with a secondary,
partially-implemented **structural PDF reader**. It is good at building new PDF 1.7/2.0 files
(text, vector graphics, images, AcroForm widgets, classic encryption, detached PKCS#7
signatures) from a Rust object model. It is **not** currently a general-purpose PDF consumer:
it cannot open arbitrary third-party PDFs reliably, cannot decrypt existing encrypted PDFs,
and has no text/content extraction API.

This distinction matters for the stated goal ("engine PDF enterprise-grade setara Adobe
Acrobat" — open/view/edit/sign arbitrary PDFs from the wild). See §8 for a gap analysis.

```
Cargo.toml features: compression, images, parser, encryption, signatures, html(empty), office(empty), full
Default features:    none (bare crate builds with color/content/document/font/forms/object/page/types/writer/error only)
```

`html` and `office` are declared Cargo features with **no corresponding module** — they compile
(because they gate nothing) but do nothing. This is aspirational surface area, not implemented
functionality.

## 2. Module inventory (src/)

16,427 lines of Rust across 51 files. Table below: purpose, and whether the module is part of
the **write path** (Document → bytes) or **read path** (bytes → Document), plus measured line
coverage (see §6).

| Module | LOC | Path | Purpose | Line cov. |
|---|---|---|---|---|
| `types/` (matrix, object_id, rectangle) | 341 | shared | Geometry primitives, `ObjectId`, transform matrix | 86–100% |
| `color/` (rgb, cmyk, gray) | 373 | shared | Device color spaces | 79–91% |
| `object/` (mod, array, dictionary, name, string, stream) | 1,363 | shared | The PDF object model (`Object`, `PdfDictionary`, `PdfArray`, `PdfName`, `PdfString`, `PdfStream`) used by both write and (partially) read paths | 63–94% |
| `content/` (mod, graphics, operator, text) | 1,247 | write | Content-stream builders (`ContentBuilder`, `GraphicsBuilder`, `TextBuilder`, `Operator`) | 60–89% |
| `font/` (mod, metrics, standard14) | 484 | write | Standard-14 font metadata only — **no embedded/TrueType/CFF font support** | 49–100% |
| `page/mod.rs` | 327 | write | `Page`/`PageBuilder` | 69% |
| `document/` (mod, info, version) | 1,665 | write | `Document`/`DocumentBuilder`, orchestrates page tree + writer | 28–80% (mod.rs itself is the least-covered file in the crate) |
| `writer/` (mod, serializer, xref) | 673 | write | Serializes the object graph to PDF bytes; **classic (table) xref only**, no xref-stream / object-stream output | 74–95% |
| `forms/` (mod, field, widget) | 2,132 | write | AcroForm field + widget construction (text, checkbox, radio, combo, list, push button) | 38–100% (field.rs is 41% covered — 129 pub items, largest single file besides document/mod.rs and signer.rs) |
| `image/` (mod, xobject) | 478 | write | JPEG/PNG embedding via the `image` crate, builds `Image XObject`s | 18–92% (`image/mod.rs` at 17.5% is the second-worst-covered non-FFI file) |
| `encryption/` (mod, config, key_derivation, permissions) | 1,186 | write only | Builds `/Encrypt` dictionaries + RC4/AES-128/AES-256 (rev 2/3/4/5/6) key derivation, **encrypts new documents only — there is no code path to open/decrypt an existing encrypted PDF** | 84–94% |
| `signatures/` (mod, config, certificate, pkcs7, signer, verifier) | 3,238 | write + read | X.509/PKCS#7 detached signing (RSA/ECDSA), verification of existing signatures. Two very different implementation strategies inside this one module — see §4.3 | 47–86% |
| `parser/` (mod, lexer, objects, trailer, xref) | 1,556 | read | nom-based structural PDF reader | 48–84% (parser/mod.rs, the orchestrator, is 48%) |
| `error.rs` | 313 | shared | `thiserror`-based error taxonomy, one enum per subsystem | not separately measured (trivial) |
| `ffi.rs` | 126 | C ABI | 4 `unsafe extern "C"` functions exposing the *write* path only (`pdf_create_simple`, `pdf_get_data`, `pdf_save_to_file`, `pdf_free`) | **0%** — no test exercises the FFI layer at all |
| `lib.rs` | 310 | — | Crate root, `prelude` re-exports, doctests | 100% (doctested) |

Public API surface (functions/structs/enums/traits marked `pub`, rough grep count, not counting
re-exports): **forms/field.rs (129)** > content/graphics.rs (41) > page/mod.rs (39) >
content/mod.rs (38) > signatures/signer.rs (27) > object/stream.rs (23) ≈ object/name.rs (23) >
… Total public items across `src/` ≈ 700+. This is a large surface for a 0.1.0 crate; most of
it (forms, content, graphics) is builder-pattern fluent API and reasonably self-consistent.

## 3. Write path (the part that works)

```
DocumentBuilder → Document { pages: Vec<Page>, info, version, encryption? }
Page/PageBuilder → content stream (ContentBuilder/GraphicsBuilder/TextBuilder → Operator list → bytes)
Document::save_to_bytes()/write_to()
  → allocates ObjectIds
  → optionally compresses each stream (Flate, feature "compression")
  → optionally encrypts each string/stream (feature "encryption", RC4/AES per EncryptionConfig)
  → PdfWriter serializes objects sequentially, tracks byte offsets
  → writes a classic (%…%%EOF, `xref` keyword, plain trailer) cross-reference table
```

This path is exercised by 68 integration tests in `tests/integration_tests.rs` covering text,
graphics, images, compression, encryption, multi-page documents, and by round-trips through the
`parser` feature for the crate's *own* output. It produces PDF 1.7/2.0 files that were manually
spot-checked against `qpdf` in the encryption test suite (`test_verify_against_qpdf_encrypted_pdf`).

**Not implemented on the write side:** xref streams (`/Type /XRef`), compressed object streams
(`/Type /ObjStm`), linearization, embedded fonts (only the 14 standard PostScript fonts are
supported — no TrueType/OpenType/CFF embedding, no CJK/Unicode text beyond WinAnsi-range glyphs
implied by Standard-14 metrics), tagged/accessible PDF (no `/StructTree`), optional content
(layers), or annotations beyond form-field widgets and signature widgets.

## 4. Read path (`parser` feature) — detailed findings

`PdfReader` (src/parser/mod.rs) is a **from-scratch, hand-rolled** nom parser. It has never been
exercised against a real-world PDF produced by Acrobat, Chrome, LibreOffice, pdflatex, Word, or
any scanner/printer driver — every test in the repo that calls `PdfReader::from_bytes` feeds it
bytes that `rust-pdf`'s own writer just produced (`tests/integration_tests.rs`,
`src/parser/mod.rs` unit tests). There is **no fixture corpus of third-party PDFs**.

### 4.1 Confirmed hidden assumptions / spec gaps

References are to ISO 32000-1:2008 unless noted.

1. **Encrypted PDFs are unconditionally rejected.** `PdfReader::from_bytes` (src/parser/mod.rs:69)
   returns `ParserError::EncryptedPdf` the moment `trailer.encrypt.is_some()`, with no attempt at
   even the empty-user-password case (§7.6.4, extremely common for "protect from editing but not
   from viewing" PDFs). The crypto primitives to derive keys exist in `encryption/key_derivation.rs`
   (including a working AES-256/R6 hash function verified against a qpdf-encrypted fixture) but
   are **not wired to the reader** — `verify_user_password` is dead code (`#[warn(dead_code)]`
   in the build log).
2. **Hybrid-reference files are not supported.** Per §7.5.8.4, a PDF can have a classic `trailer`
   dictionary with an `/XRefStm` entry pointing to a supplemental cross-reference *stream* for
   readers that understand it. `Trailer::from_dictionary` (src/parser/trailer.rs) never looks at
   `/XRefStm`; only `/Prev` is followed. Files written this way (common for
   backward-compatibility with pre-1.5 readers) will silently miss any objects only present in
   the stream.
3. **No recursion/iteration bound on the `/Prev` chain** (src/parser/mod.rs:109-166,
   `parse_xref_and_trailer`). A PDF whose trailer chain cycles back on itself (corrupt or
   adversarial) causes an infinite loop, not an error — the process never returns. This is a DoS
   vector on untrusted input (rule 2 in the task brief).
4. **No recursion depth limit in the object grammar.** `parse_object` (src/parser/objects.rs) is
   directly recursive through `parse_array_object` / `parse_dictionary_or_stream` with no depth
   counter. A deeply nested array/dictionary (`[[[[[…]]]]]`, megabytes of `[`) will overflow the
   Rust call stack and abort the process (not a catchable `Result` error). Classic recursive-
   descent-parser DoS.
5. **Unbounded `xref` subsection `count` field.** `parse_xref_table`
   (src/parser/xref.rs:108-141) trusts the `count` value read straight from the file and loops
   `for i in 0..count` inserting into a `HashMap`, with `first_obj + i as u32` unchecked for
   overflow. A crafted subsection header claiming a huge count (up to `u32::MAX`) will either
   panic on integer overflow (debug builds) or attempt to allocate/insert billions of HashMap
   entries (release builds) before the truncated input causes a parse error — memory/CPU DoS
   with a tiny input file.
6. **Zlib decompression has no output-size cap.** `PdfStream::decompress()` (src/object/stream.rs:154)
   calls `ZlibDecoder::read_to_end` with no limit — a classic decompression-bomb vector (a few KB
   of input can expand to gigabytes). This code path is reached both from user code calling
   `decompress()` directly and internally from `parse_xref_stream` /
   `resolve_compressed_object` while reading a file.
7. **Unbounded, panicking byte-slicing on offsets taken from the file.** `resolve_reference`
   (src/parser/mod.rs:365-380) does `&self.data[*offset as usize..]` where `offset` comes
   verbatim from an xref entry with no bounds check against `self.data.len()`; a corrupt/adversarial
   offset panics (`slice index out of range`) instead of returning an `Err`. Same pattern in
   `resolve_compressed_object` (src/parser/mod.rs:388, 419, 443) for the object-stream `First`
   offset and computed `obj_offset`/`next_offset` bounds. This directly violates the "no panics
   on untrusted file data" requirement.
8. **Only `/FlateDecode` is understood; every other filter is silently treated as raw data.**
   `PdfStream::decompress()` returns `self.data.clone()` unchanged for `ASCIIHexDecode`,
   `ASCII85Decode`, `LZWDecode`, `RunLengthDecode`, `CCITTFaxDecode`, `DCTDecode`,
   `JBIG2Decode`, `JPXDecode`, or a `Filter` array (§7.4) — there is no error, so a caller
   iterating "decompressed" bytes gets silently wrong (still-encoded) data with no signal
   anything went wrong.
9. **`PdfReader::get_object()` never returns anything.** `object_cache` (src/parser/mod.rs:47) is
   populated nowhere in the codebase — `resolve_reference` takes `&self` and never inserts into
   the cache, so `get_object()` (src/parser/mod.rs:343-352) always returns `None` after
   construction, and every `resolve_reference` call *re-parses the object from raw bytes from
   scratch* (no caching despite the cache field existing). This looks like an unfinished
   refactor, not intentional behavior.
10. **`Length` must be a direct integer.** `parse_dictionary_or_stream`
    (src/parser/objects.rs:130-143) errors out if a stream's `/Length` is an indirect reference
    (`5 0 R`), which is extremely common in real-world PDF producers (Length is written as a
    reference so it can be patched after the stream is emitted, per the informal convention many
    writers use). This alone will make many real-world files unparsable, independent of the
    xref-stream/object-stream gaps above.
11. **Page count is trusted, not verified.** `get_page_count_from_tree`
    (src/parser/mod.rs:301-321) reads `/Count` from the immediate `/Pages` dictionary and returns
    it as-is; it never walks `/Kids` to confirm the tree is well-formed, detect cycles, or handle
    a `/Type /Pages` intermediate node. There is no page-content or page-tree traversal API at
    all beyond this single integer.

### 4.2 What *is* implemented and tested on the read side

Classic (non-stream) xref table parsing, classic trailer parsing (`/Root`, `/Size`, `/Info`,
`/Encrypt`-detection-only, `/ID`, `/Prev`), the core object grammar (booleans, integers, reals,
literal/hex strings with escape handling, names, arrays, dictionaries, indirect references,
`null`), indirect-object framing (`N G obj … endobj`), and a header-version check. Xref-*stream*
parsing (`parse_xref_stream`) and compressed-object-stream resolution
(`resolve_compressed_object`) are **implemented** (§4.1 code exists) but have **zero test
coverage** — no test in the repository constructs or feeds a `/Type /XRef` or `/Type /ObjStm`
object through the reader, so their correctness against the spec is unverified.

### 4.3 Signing path uses a second, independent, ad-hoc "parser"

`src/signatures/signer.rs`'s `IncrementalSigner` (used to add a signature to an *existing*,
externally-supplied PDF byte buffer — as opposed to `DocumentSigner`, which signs a freshly-built
`Document`) does **not** use `src/parser/` at all. Instead it does its own plain-text scanning of
`String::from_utf8_lossy(pdf_bytes)` with `.find("/Root")`, `.find("{id} 0 obj")`,
`.rfind(">>")`, `content.lines().rev()`, etc. (over 20 call sites, e.g.
src/signatures/signer.rs:216, 249-250, 469, 520, 1015, 1097). This works only for PDFs whose
catalog/page/AcroForm objects are:
- plain classic-xref (not xref-stream) files, and
- **not** stored inside a compressed object stream (`/Type /ObjStm`) — since compressed objects
  have no `N G obj`/`endobj` markers in the byte stream at all, `content.find("{id} 0 obj")` can
  never locate them, and
- pure-ASCII dictionaries without the exact target byte sequence `"{id} 0 obj"` coincidentally
  appearing inside earlier binary/compressed stream data (which would misdirect the byte-offset
  scan).

Concretely: `IncrementalSigner` will very likely **fail or silently misbehave** on any PDF
produced by a modern producer that defaults to compressed cross-reference/object streams (a
large fraction of PDFs from recent Acrobat, many PDF/A generators, etc.). This is a maintenance
and correctness risk: two independent, inconsistent implementations of "find object N in this
PDF" exist in the same crate, and the newer one (signer) is a regression from the structured
parser rather than building on it. No `.unwrap()` in this file was found to be reachable without
a preceding existence check (i.e., not an immediate panic risk), but the *logic* itself is
fragile against real-world files.

## 5. Encryption & Signatures — what's real

- **Encryption** (`encryption` feature): builds `/Encrypt` dictionaries for RC4 (V1/V2, R2/R3),
  AES-128 (V4, R4) and AES-256 (V5, R6) per the algorithms in ISO 32000-2:2020 Annex C /
  PDF 2.0 §7.6, with key derivation validated against a real `qpdf`-encrypted fixture
  (`test_verify_against_qpdf_encrypted_pdf`). This is write-only: there is no `Document::open`
  that takes a password and decrypts an existing file.
- **Signatures** (`signatures` feature): builds detached PKCS#7 (`/SubFilter
  /adbe.pkcs7.detached`) signatures over RSA (PKCS#1v1.5, via `rsa` crate) and ECDSA P-256
  (via `p256`/`ecdsa`), with a hand-rolled minimal DER/PKCS#7 encoder
  (`src/signatures/pkcs7.rs`) rather than relying on the `cms`/`der` crates' encoders for the
  outer structure (those crates are pulled in as dependencies but the PKCS#7 `SignedData` bytes
  appear to be built manually — worth double-checking against the `cms` crate's own encoder in a
  later phase for spec fidelity). Verification (`SignatureVerifier`) parses `/ByteRange`,
  extracts the hex `/Contents`, and validates the signature over the byte range — this part *does*
  reuse plain byte-offset math rather than a second lossy-string scan, and is covered by
  `tests/signature_verification_tests.rs` (7 tests, including a tamper-detection test).
- Timestamping (RFC 3161 `/TS`) and Long-Term Validation (`/DSS`, PAdES) are not implemented.

## 6. Test coverage (measured, not estimated)

Tool: `cargo-llvm-cov` (installed for this audit; was not previously part of the toolchain).

```
cargo llvm-cov --all-features --summary-only
```

Aggregate (see full per-file table in the raw output, reproducible with the command above; HTML
report generated at `target/llvm-cov/html/index.html`, not committed — `target/` is gitignored):

| Metric | Covered / Total | % |
|---|---|---|
| Regions | 10,893 / 15,766 | **69.09%** |
| Functions | 863 / 1,295 | **66.64%** |
| Lines | 6,259 / 9,220 | **67.89%** |

Notably low files (line coverage):
- `ffi.rs` — **0%** (58/58 lines missed). The C ABI boundary is completely untested.
- `image/mod.rs` — 26.20% line coverage (17.54% region coverage) — image loading/decoding error
  paths largely untested.
- `document/mod.rs` — **30.51%** line coverage on the single largest file (1,674 regions,
  1,293 lines) — the document-build/write orchestrator, including most encryption/signature
  integration branches, is under-tested relative to its size.
- `forms/field.rs` — **37.75%** line coverage on the largest form-field file (129 public items).
- `parser/mod.rs` — **47.56%** line coverage — and, per §4.3, the *covered* 47% does not include
  the xref-stream / object-stream code paths at all (those functions do execute during coverage
  collection only if a test calls them, and none does — meaning their apparent partial coverage,
  if any, is incidental basic-block overlap, not a real exercise of that logic).

Test counts (`cargo test --all-features`): 235 unit tests (`src/`) + 68 integration tests
(`tests/integration_tests.rs`) + 7 signature-verification tests
(`tests/signature_verification_tests.rs`) + 2 executed doctests (6 more are `ignore`d) = **312
passing, 0 failing**.

## 7. `cargo audit` results

```
$ cargo audit
    Fetching advisory database from `https://github.com/RustSec/advisory-db.git`
      Loaded 1149 security advisories
    Scanning Cargo.lock for vulnerabilities (102 crate dependencies)
```

**1 vulnerability, 1 warning:**

| Advisory | Crate | Severity | Detail | Fix available? |
|---|---|---|---|---|
| [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071) | `rsa 0.9.10` | Medium (5.9) | "Marvin Attack": potential RSA private-key recovery via timing side channel | **No fixed version exists upstream** as of this audit |
| [RUSTSEC-2026-0097](https://rustsec.org/advisories/RUSTSEC-2026-0097) (warning, not a vuln) | `rand 0.8.5` (direct dep + transitive via `num-bigint-dig` ← `rsa`) | — | "Rand is unsound with a custom logger using `rand::rng()`" | Not evaluated whether applicable to this crate's usage pattern |

**Implication:** `rsa 0.9.10` (used for RSA PKCS#1v1.5 signing in the `signatures` feature) is the
only maintained pure-Rust RSA implementation with `cms`/`x509-cert` ecosystem compatibility at
the time of writing, and it carries a known, currently-unpatched timing side-channel advisory.
For an enterprise/legal-document-signing product this needs an explicit risk decision (accept +
document, mitigate with blinding/constant-time key ops if not already default, or swap to an
HSM/external-signing model where the private key never touches this process). Not evaluated
further in this phase — flagged for a security-focused follow-up.

## 8. `cargo clippy --all-features --all-targets -- -D warnings`

**Current status: fails (13 distinct lint categories, 0 unsafe-related).** Full log captured
during this audit; none were fixed (audit phase — "JANGAN ubah kode fungsional di fase ini").
Plain `cargo clippy --all-features` (warnings not promoted to errors) succeeds with the same 13
warnings and no build failure — the crate is clippy-clean under default severity, only fails the
stricter `-D warnings` gate the team wants to enforce going forward. Breakdown:

| Count | Lint | Example location |
|---|---|---|
| 5 | `approx_constant` (literal `3.14` flagged as "close to π") | `object/mod.rs`, `parser/lexer.rs`, `parser/objects.rs` (all in `#[test]` code, using 3.14 as an arbitrary test value) |
| 2 | `derivable_impls` (manual `impl Default` that could be `#[derive(Default)]`) | `signatures/mod.rs:130`, one more |
| 2 | `manual_is_multiple_of` (`x % 16 != 0` → `!x.is_multiple_of(16)`) | `encryption/key_derivation.rs:310,420` |
| 1 | `single_match` (`match` used for a single equality check) | `forms/widget.rs:101` |
| 1 | `needless_borrow` | `writer/mod.rs:130` |
| 1 | `if_same_then_else` (two identical `if`/`else` arms) | `signatures/pkcs7.rs:382-386` |
| 1 | `len_zero` (`.len() > 0` → `!.is_empty()`) | `object/stream.rs:318` (test code) |
| 1 | `unnecessary_unwrap` (`.is_some()` check followed by `.unwrap()` instead of `if let`) | `signatures/signer.rs:1169-1170` |
| 1 | `unnecessary_cast` (`u8 as u8`) | `parser/lexer.rs:200` |
| 1 | `unnecessary_cast` (`u32 as u32`) | `parser/xref.rs:133` |
| 1 | `doc_lazy_continuation` (rustdoc formatting) | — |
| 2 | `dead_code` (`verify_user_password`, `aes_cbc_decrypt_no_padding` never called — see §4.1 finding #1) | `encryption/key_derivation.rs:375,414` |

None of these are correctness-critical on their own; they are cleanliness/idiom issues plus two
`dead_code` warnings that corroborate the "encrypted-PDF read path was never finished" finding.
Recommended to fix in a dedicated lint-cleanup commit before turning on `-D warnings` in CI.

## 9. Untrusted-input risk register (summary)

Per the task's mandatory rule ("semua kode yang menangani input file PDF adalah untrusted
input"), the following are the concrete, reproducible risk points found in `parser`/`object`
during this audit (details in §4.1):

| # | Risk | Location | Trigger |
|---|---|---|---|
| 1 | Infinite loop | `parser/mod.rs` `parse_xref_and_trailer` | `/Prev` cycle in trailer chain |
| 2 | Stack overflow (process abort, not `Result::Err`) | `parser/objects.rs` `parse_object`/`parse_array_object`/`parse_dictionary_or_stream` | deeply nested `[`/`<<` |
| 3 | Unbounded allocation / integer overflow | `parser/xref.rs` `parse_xref_table` | huge subsection `count` |
| 4 | Decompression bomb (unbounded memory) | `object/stream.rs` `decompress()` | small Flate stream expanding to GBs |
| 5 | Slice-index panic | `parser/mod.rs` `resolve_reference`, `resolve_compressed_object` | xref offset / object-stream `First`/computed offsets beyond `data.len()` |
| 6 | Silent wrong data (not a crash, but a correctness/security issue) | `object/stream.rs` `decompress()` | non-FlateDecode filter treated as passthrough |

None of these were fixed in this phase (out of scope per the task's "audit only" instruction);
they are the primary input for the next hardening phase.

## 10. Gap vs. a mature C/C++ PDF engine (pdfium / MuPDF / poppler / qpdf)

Being explicit per the task brief: reaching feature parity with a mature, battle-tested C/C++
PDF engine (rendering to raster/vector output, arbitrary third-party file compatibility including
malformed/repairable files, full filter set, font subsetting/embedding + CJK, tagged PDF,
annotations, redaction, OCR-adjacent text extraction, incremental-save correctness against every
producer quirk in the wild) is a **multi-year, multi-person effort**, not a phase or two. Those
libraries encode roughly two decades of "PDF producers are wrong in every possible way" bug
fixes that cannot be derived from the spec alone — they were learned from huge real-world test
corpora (pdfium alone has tens of thousands of regression-test PDFs). Concretely, for
*this* crate to become "setara Adobe Acrobat" as a read/edit engine (not just write), realistic
missing pieces include:
- A real content-stream interpreter + rasterizer/vector-renderer (does not exist at all today —
  this crate has no rendering code whatsoever, only generation and structural inspection).
- Font program parsing/subsetting (TrueType/OpenType/CFF/Type1) and embedding, plus a
  text-layout/shaping engine for anything beyond the 14 standard fonts.
  Estimated effort: several person-months minimum for a correct embedded-font pipeline alone.
- A "repair mode" parser tolerant of the many ways real files violate the spec (broken xref,
  missing `endobj`, wrong `/Length`, byte-offset drift after third-party edits) — qpdf/mupdf
  invest heavily here; this crate currently has none (§4.1).
- Filter completeness (LZW, CCITT G3/G4, JBIG2, JPX, ASCII85, RunLength) — currently only
  FlateDecode.
- Encrypted-file *opening* (all revisions, empty/owner/user password logic) — currently write-only.
- Redaction, tagged/accessible PDF, forms with full JavaScript-triggered calculations (AcroForm
  *data model* exists here; the interactive/calculation engine does not).
- A rendering surface Tauri could actually put pixels on screen with (this crate produces PDF
  bytes; something else — pdfium via FFI, or a from-scratch renderer — is needed to *display*
  a PDF, which is usually the majority of an "Adobe Acrobat-class" desktop app's engineering).

Given the above, a realistic estimate for closing the gap to a genuinely mature, enterprise-grade
engine (matching, not necessarily exceeding, pdfium/mupdf-class robustness across rendering +
editing + interop) is **on the order of 12–24+ person-months** of focused work, assuming the
write-side (already fairly solid) is kept and extended rather than rewritten. Treating this
crate as "the PDF engine" for a desktop app that must open arbitrary user-supplied PDFs (as
opposed to only generating/signing documents the app itself created) is the highest-risk framing
and should be validated against actual product requirements before further investment — it may be
more realistic to keep `rust-pdf` for generation/signing/forms and use a mature C/C++ engine
(e.g., pdfium) via FFI for viewing/rendering/interop with arbitrary third-party files, at least
in the near term.

## 11. What this audit did *not* change

Per the task's explicit "audit phase" scope, no functional source file under `src/` was modified.
The only artifacts added by this task are this file (`ARCHITECTURE.md`) and local, gitignored
tooling output (`target/llvm-cov/**`). `cargo-llvm-cov` was installed into the local cargo
toolchain (`~/.cargo/bin`) to produce the coverage numbers in §6; it was not present before this
audit.
