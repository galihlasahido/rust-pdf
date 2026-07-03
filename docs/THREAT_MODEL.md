# Threat model — `rust-pdf`

Status: living document, written for the "Security Hardening" phase (2026-07).
Scope: this document covers the library as it exists in this repository. It
does **not** cover how a downstream application (e.g. a Tauri desktop app)
handles process isolation, sandboxing, or UI-level trust decisions — those
are the embedding application's responsibility and out of scope here.

## 1. Purpose

Give a reviewer (security, or a future maintainer) a single place that
answers:

1. What can an attacker who controls a PDF file (or a font/ICC profile
   embedded in one) actually do to a process linking this crate?
2. Where does this crate draw its trust boundaries, and what mitigates each
   crossing?
3. What residual risk is accepted, and why?
4. Why is each currently-ignored `cargo audit` advisory safe to ignore, on a
   per-crate basis (not a blanket category suppression)?

This replaces the informal "risk register" in `ARCHITECTURE.md` §9, which was
an audit-phase snapshot ("None of these were fixed in this phase … out of
scope") rather than a threat model, and is now superseded by this document
for anything overlapping. `ARCHITECTURE.md` §9/§10 are left as-is as
historical record of that earlier audit; do not treat them as the current
mitigation status.

## 2. Attacker model

The primary attacker is **whoever produced the PDF file (or an embedded
font/ICC/JPEG payload inside it) that this process opens** — a file
downloaded from email, the web, a shared drive, etc. This attacker:

- Fully controls every byte of the input file, including all structural
  metadata (`/Length`, xref offsets/counts, `/Columns`/`/Colors` filter
  parameters, object counts, nesting) and every embedded binary blob
  (`FontFile2`/`FontFile3` font programs, `DCTDecode`/`CCITTFaxDecode` image
  data, ICC profiles, XMP metadata).
- Does **not** have arbitrary code execution or memory-corruption
  capability a priori — that is exactly the class of bug this document is
  about preventing. The goal is to keep it that way (memory safety) and to
  bound the attacker to, at worst, a graceful `Err` or a bounded-time/bounded-
  memory failure — never an unbounded allocation, infinite loop, stack
  overflow, or process abort.
- Has no network access to this process (the crate does no networking
  itself, except the optional `signatures::timestamp` RFC 3161 client,
  which talks to a TSA server the *caller* configures — that server is a
  separate, narrower trust boundary, see §4.7).

A secondary, weaker attacker is a **malicious FFI caller** through
`src/ffi.rs` (e.g. a compromised or buggy C/Python host process embedding
this crate via the C ABI). Unlike the file-content attacker, this one is
assumed to already be running code in the same address space, so the bar
here is "don't make an already-compromised caller's job easier via UB"
rather than "prevent exploitation" — see §4.5.

## 3. Trust boundaries

```
[Untrusted PDF bytes on disk/network] ──▶ PdfReader / parser (parser/*)
                                              │ (bytes only, no exec)
                                              ▼
                                       filter::decode_filter (filter/*)
                                              │ (decoded bytes only)
                                              ▼
                              object model (object::Object / PdfStream)
                                              │
                        ┌─────────────────────┼───────────────────────┐
                        ▼                     ▼                       ▼
              editor:: (content-stream    font::truetype (ttf-parser  render:: (pure-Rust
              rewriting, redaction,       over untrusted FontFile2/3) content-stream interp-
              forms, structure)                                       reter, optional feature,
                                                                        decodes to raster pixels)
```

Every arrow above is a trust boundary: data crosses from "fully attacker
controlled" to "this crate's code, which must not panic/hang/over-allocate
on it". `src/ffi.rs` is a separate, orthogonal boundary (caller-controlled
pointers/lifetimes rather than file bytes) — see §4.5.

## 4. Component risk register

Each entry: **risk → mitigation (file:const/function) → residual risk**.

### 4.1 Parser (`src/parser/*`, ISO 32000-1:2008 §7.5, §7.3)

| Risk | Mitigation | Residual risk |
|---|---|---|
| Infinite loop via a cyclic `/Prev` chain in incremental-update trailers (§7.5.4) | `parse_xref_and_trailer` tracks visited offsets in a `HashSet` and caps total sections at `MAX_XREF_SECTIONS = 4096` (`parser/mod.rs:83`) | None known — cycle detection is unconditional, not just a count cap |
| Stack overflow from unbounded `[`/`<<` nesting in a hand-rolled recursive-descent parser (§7.3.6, §7.3.7) | `MAX_NESTING_DEPTH = 64` enforced in `parse_object_at`/array/dict parsing (`parser/objects.rs:15`) | Low — 64 is far beyond any legitimate document structure; not exhaustively fuzz-proven against every recursive call site in the parser (content-stream operator nesting, page-tree walks) individually, though page-tree walks have their own separate depth cap (`MAX_PAGE_TREE_WALK_DEPTH`, `parser/mod.rs:106`) |
| Unbounded loop / integer overflow from a huge xref subsection `count` header | `checked_add` on `first_obj + i` (`parser/xref.rs:141`); the loop itself is naturally bounded by remaining input length because each entry parse consumes bytes from the (finite) file and fails fast on truncation — no `Vec::with_capacity(count)` pre-allocation exists | None known |
| Decompression bomb (small stream, huge decoded size) | `filter::MAX_DECODED_SIZE = 512 MiB` (`filter/mod.rs:47`) bounds every filter's output via `Read::take`; **also** now enforced in the legacy `PdfStream::decompress()` path (`object/stream.rs`), which had no cap prior to this hardening pass — see §6 | None known for the filter layer itself; predictor post-processing (below) is a separate bypass that is now also capped |
| Predictor-stage bomb: a 2-byte stream declaring `/Columns 2000000000` forces a multi-GB allocation in TIFF-predictor unpacking, *after* `MAX_DECODED_SIZE` has already been enforced on the compressed→decompressed step | `unpack_bits`/`pack_bits` cap sample counts against the actual observed row length (`filter/predictor.rs`, `unpack_bits`) instead of trusting `/Columns`×`/Colors` directly; `saturating_mul` used throughout to avoid overflow panics | None known — covered by dedicated regression tests (`tiff_predictor_huge_declared_columns_does_not_bomb`, `tiff_predictor_huge_declared_colors_does_not_overflow_or_bomb`) |
| Slice-index panic from an xref offset / object-stream `First`/computed offset beyond the actual file length | `resolve_reference`/`resolve_compressed_object` use `.get(..)` (never direct indexing) and propagate `None` on any out-of-range offset (`parser/mod.rs:620-711`) | None known |
| Silent wrong data: a stream naming an unrecognized filter silently treated as identity/passthrough | `filter::decode_filter` (`filter/mod.rs`) returns `Err(CompressionError::DecompressionFailed(...))` for any name it doesn't recognize; only the semantically-correct `/Crypt` identity case (no encryption applied yet) intentionally passes through | The still-public legacy `PdfStream::decompress()` intentionally treats *any* non-`FlateDecode` filter as "return original bytes unchanged" — this is documented on the method itself as a legacy/limited-filter API; `decode_all()` is the modern replacement and does not have this behavior. Not removed (would be an API break outside this task's scope) but explicitly discouraged in its rustdoc |
| Corrupt/unrepairable structure (broken xref, missing `endobj`) causing an outright parse failure rather than a crash | `parser::recovery` implements a bounded best-effort object scan (`MAX_RECOVERED_OBJECTS = 2_000_000`, `parser/recovery.rs:30`) | A full "repair mode" on par with qpdf/mupdf (handling every real-world producer quirk) is not attempted — see §7 |

### 4.2 Filter decoders (`src/filter/*`, ISO 32000-1 §7.4)

Every decoder (`ascii85`, `ascii_hex`, `run_length`, `lzw`, the `flate2`
wrapper, `ccitt`, the optional `dct`) takes only `&[u8]` and returns
`Result<Vec<u8>, CompressionError>` — no panics on malformed input by
contract (module-level rustdoc in `filter/mod.rs`). LZW additionally bounds
its code-table growth (`MAX_TABLE_SIZE = 4096`, `filter/lzw.rs:16`), which
is a spec-mandated bound (12-bit codes), not just a safety margin. Residual
risk: `ccitt`/`dct` delegate to less-fuzzed code paths than `flate2`/`lzw`
(see §5 — `decode_filters` fuzz target exercises all of these, but longer
continuous fuzzing time would increase confidence for the image codecs
specifically).

### 4.3 Font loader (`src/font/truetype.rs`, ISO 32000-1 §9.9 "Embedded Font
Programs")

This module deliberately does not hand-roll a font parser; it delegates to
`ttf-parser` (chosen originally for being battle-tested against malformed
real-world fonts) and only extracts the small set of facts the PDF writer
needs.

| Risk | Mitigation | Residual risk |
|---|---|---|
| Unbounded memory from an oversized font file | `MAX_FONT_SIZE_BYTES = 64 MiB` checked before parsing (`truetype.rs:32`) | None known |
| Unbounded work from a font declaring an absurd glyph count | `MAX_GLYPH_COUNT = 200_000` checked against `face.number_of_glyphs()` (`truetype.rs:39`) | None known |
| **`ttf-parser` itself panicking (process abort) on malformed input instead of returning `Err`** | **Found during this phase** via the `font_load` fuzz target: a malformed `ttcf` (TrueType Collection) header whose table offset overflows `u32` trips an internal `assert!` in `ttf-parser::parser::Stream::read_bytes` rather than failing gracefully. `TrueTypeFont::load` now isolates the `ttf_parser::Face::parse` call in `std::panic::catch_unwind` (`AssertUnwindSafe` is sound here because the closure only reads `data`/`face_index`, never mutates them, so a panic mid-parse cannot leave anything in an observably-inconsistent state) and surfaces the panic as `FontLoadError::Panicked` instead of aborting the process. Regression test: `malformed_ttc_offset_overflow_does_not_panic`. | **Medium, accepted for now.** `ttf-parser` is unmaintained (RUSTSEC-2026-0192, §7.2) so more panicking edge cases likely exist and will not be fixed upstream; `catch_unwind` converts *any* panic in this call into a graceful error, so the residual risk is bounded to "malformed font → `Err`", not "malformed font → crash" — but each occurrence still prints a diagnostic to stderr via the default panic hook (not overridden, since doing so is process-global state that would affect unrelated panics elsewhere in a host application). Note: `cargo fuzz run` for `font_load` will continue to report this exact input class as a libFuzzer "crash" even after this fix — `libfuzzer-sys` installs its own panic hook that calls `abort()` *before* any unwinding/`catch_unwind` can run, specifically so fuzzing always treats panics as findings regardless of a target's own panic-handling code. That is expected, is not a regression, and does not indicate the production mitigation is ineffective (verified instead via the `cargo test` regression test, which runs without `libfuzzer-sys`'s hook installed). |

### 4.4 Object streams (`src/parser/mod.rs`, ISO 32000-1 §7.5.7)

Compressed-object resolution (`resolve_compressed_object`) bounds-checks
every header-derived offset/index against the actual decoded-stream length
before slicing (`.get(..)` throughout), and decoding an object stream goes
through the same `MAX_DECODED_SIZE`-bounded filter path as any other
stream. No recursion back into `resolve_reference` occurs here, so this is
not an additional stack-depth risk beyond §4.1.

### 4.5 FFI boundary (`src/ffi.rs`)

All four `extern "C"` functions (`pdf_create_simple`, `pdf_get_data`,
`pdf_save_to_file`, `pdf_free`) are `unsafe fn` with a documented `# Safety`
contract (non-null pointers, valid null-terminated C strings, handle
lifecycle) and each `unsafe` block carries a `SAFETY:` comment justifying
why the operation is sound *given that contract*. This boundary's threat
model is different from the file-parsing one: the caller here is assumed
to already be running native code in the same process, so the goal is
contract clarity (so a correct caller cannot trigger UB) rather than
defending against a malicious caller (a caller that violates documented
preconditions, e.g. passing a dangling `handle`, can already do arbitrary
damage from native code regardless of what this crate does). Residual
risk: none beyond "caller must honor the contract", which is the accepted
nature of any `unsafe extern "C"` API.

### 4.6 Renderer (`src/render/*`, optional `render`/`native-render` features)

**Updated: this renderer was migrated off an earlier FFI binding to a
third-party native rendering engine (Google's Pdfium, via `pdfium-render`)
to a from-scratch, pure-Rust content-stream interpreter and rasterizer
(`render::native`, backed by `tiny-skia` for rasterization and
`ttf-parser` for font outlines); see `src/render/mod.rs`'s module docs for
the migration history. `pdfium-render` and every other FFI/native-binary
dependency have been fully removed from this crate -- there is no
`unsafe extern "C"` boundary in the renderer at all anymore, and no
process-wide native-library singleton/lock.**

`PdfRenderer` (`render::PdfRenderer`) resolves a page's effective
`/MediaBox`/`/Rotate`/`/Resources`/content streams/`/Annots` through this
crate's own `EditableDocument` parser and hands the content stream to
`render::native::render_content_stream`, a pure-Rust interpreter with no
`unsafe` blocks of its own. `MAX_RENDER_PIXELS = 64_000_000`
(`render/mod.rs`) still bounds output-buffer allocation against a page
requesting an absurd raster target size, computed *before* any raster is
allocated, for both full-page and tiled/viewport renders; a similar
`MAX_RESOLVE_REFERENCES`/`MAX_RESOLVE_DEPTH` budget
(`render/renderer.rs`) bounds the work done resolving a page's
`/Resources`/`/Annots` indirect-reference graph against a crafted
reference cycle or a pathologically wide/deep fan-out.

Residual risk shifts accordingly: instead of "a memory-safety bug inside
a large, separate C++ codebase outside this crate's control" (Pdfium's
old risk profile), the risk is now "a bug inside this crate's own,
newly-written, pure-Rust content-stream interpreter" -- memory-safety
bugs are far less likely (Rust's ownership model rules out the classic
C/C++ buffer-overflow/use-after-free class entirely, modulo the handful
of `unsafe` blocks inside this crate's own dependencies, `tiny-skia`/
`ttf-parser`/`image`, which are outside this document's scope the same
way Pdfium's internals were), but logic bugs (a malformed/adversarial
content stream producing an incorrect, or a panicking, render) are this
crate's own responsibility to fix, not an upstream project's. Every
currently-known compatibility gap (JBIG2/JPX images, Type1/bare-CFF font
programs, non-embedded/system font substitution, true ICC colour
management, Patterns/shadings) is designed to fail closed -- a structured,
documented warning or placeholder, never a panic or a silently blank/
wrong render -- see `src/render/native/mod.rs`'s "Explicit, honest gaps"
section for the exhaustive, currently-accurate list.

**Note for reviewers auditing this document against an older copy:** an
earlier version of this section had a row/entry for `ffi_lock`, the
process-wide `Mutex` the old Pdfium binding required (Pdfium's C API was
not safe to call concurrently) plus the `ManuallyDrop`/custom `Drop`
ordering built around it. That entry is **removed, not merely reworded**
-- it no longer describes anything that exists in this crate. There is no
process-wide native-library lock of any kind in the current renderer (see
the bolded paragraph above): every `PdfRenderer` instance is a plain,
independent, `Send + Sync` Rust value with its own state, and concurrent
renders across multiple instances need no synchronization this crate
imposes. `ARCHITECTURE.md` §8/§8c is the historical record of that earlier
design (per this document's own scope note in §1) and is intentionally
left describing `ffi_lock` as it was; this document, describing the
*current* system, must not.

#### 4.6a Content-stream interpreter attack surface (Security Hardening phase)

**Why this is new attack surface, not just a relocation of Pdfium's old
one.** Pdfium is Google Chrome's PDF engine, fuzzed continuously at Google
for years before this crate ever linked it; whatever this crate's
`render` feature exercised through Pdfium inherited that history. This
crate's own content-stream interpreter (`render::native::interpreter`,
§4.6 above) has none of it -- it is new code, parsing/executing the exact
same untrusted, attacker-controlled operator stream (ISO 32000-1:2008
7.8.2) Pdfium used to, but with only as much adversarial testing as this
crate's own fuzzing has given it so far. This subsection is the risk
register specifically for *that* surface: an attacker who can embed a
crafted content stream (a page's own, or a Form XObject's, or a Type 3
glyph procedure's) into a PDF this process opens for rendering.

| Risk | Mitigation | Residual risk |
|---|---|---|
| Deeply-nested `q`/`Q` save/restore forcing unbounded graphics-state-stack growth | `MAX_GRAPHICS_STATE_DEPTH = 4096` (`interpreter.rs`), enforced on every `q`; returns `NativeRenderError::GraphicsStateStackOverflow` | None known |
| Self-referential or mutually-recursive Form XObjects (a Form's content stream invoking itself, or a cycle of several, via `Do`) | `MAX_FORM_XOBJECT_DEPTH = 12` (`interpreter.rs`), shared across direct `Do`, transparency-group, and ExtGState `/SMask` group recursion; recorded as `RenderWarning::FormXObjectRecursionLimitExceeded`, that branch's rendering stops rather than recursing further | None known -- covered by both a unit test (`self_referential_form_xobject_is_bounded_not_infinite`) and, this phase, the `render_interpreter` fuzz target's fixed hostile `/Resources` (mutually-recursive `/RecA`/`/RecB` Form XObjects, see §5) |
| Self-referential Type 3 glyph procedure (a `CharProc` showing text with its own font, directly or through a cycle) | `MAX_TYPE3_DEPTH = 6` (`font.rs`), independent counter from the Form depth above but checked at every glyph paint; `RenderWarning::Type3RecursionLimitExceeded` | None known -- `type3_self_referential_charproc_is_bounded_not_infinite` unit test, plus the fuzz target's self-referential `/T3` font |
| A content stream that is very long but *never* nested/recursive at all (e.g. millions of flat `q Q` pairs, or millions of path-construction operators), which none of the three depth caps above bound since depth never actually grows | **New this phase:** `MAX_OPERATOR_COUNT = 2_000_000` (`interpreter.rs`), a running total shared across the top-level content stream and every nested Form XObject/Type 3/transparency-group render (one `RenderBudget`, reborrowed through the recursion the same way `pixmap`/`warnings` are); exceeding it aborts the render with `NativeRenderError::OperatorBudgetExceeded` rather than continuing indefinitely | None known -- `operator_budget_exceeded_is_a_structured_error_not_a_hang` unit test |
| Input whose *cost per operator*, not operator count, is what makes it pathological (e.g. a legal, modest operator count that nonetheless does expensive per-operator work) | **New this phase:** `MAX_RENDER_DURATION = 20s` wall-clock budget (`interpreter.rs`), checked alongside the operator count on every content-stream item; `NativeRenderError::RenderTimeBudgetExceeded` | Coarser than the other bounds by nature (wall-clock, not a deterministic count) -- generous enough that no legitimate render should hit it, but a heavily-loaded host process could in principle see it trip closer to the margin than on idle hardware; accepted, since the alternative (no time bound at all) is strictly worse |
| A single path object (between one path-painting operator and the next) accumulating unbounded points from a long run of `l`/`c` construction operators with no intervening paint | **New this phase:** `MAX_PATH_POINTS_PER_PATH = 1_000_000` (`path.rs`), reserved incrementally per path object and reset at the next path-painting operator; further construction on an over-budget path is dropped (not the whole render), `RenderWarning::PathPointBudgetExceeded` recorded once | None known |
| **Found during this phase** via the `render_interpreter` fuzz target: a path with one finite-but-absurdly-large coordinate (reachable from an ordinary content stream via a huge literal operand, or a `cm` scale factor applied to an ordinary one) trips an internal `assert!` in `tiny_skia::scan::path::fill_path_impl` (`edges[curr_idx].last_y >= curr_y as i32`) and aborts the process -- **`tiny-skia` itself does not always gracefully refuse out-of-range geometry**, contradicting what this crate's own `render::native` module docs previously (incorrectly) claimed | `path::sanitize_point` now clamps every device-space coordinate's magnitude to `MAX_COORDINATE_MAGNITUDE = 1_000_000.0` (empirically well below the confirmed-safe `1e10` and confirmed-crashing `1e11` from bisecting this finding), in addition to its pre-existing `NaN`/`Infinity` sanitization -- same architecture as the `ttf-parser` `catch_unwind` mitigation in §4.3: a defensive clamp in this crate's own code at the boundary into a dependency, not a patch to the dependency itself. Regression test: `extreme_finite_path_coordinate_does_not_panic` (interpreter-level) and `sanitize_point_clamps_extreme_finite_magnitude` (unit-level) | **Medium, accepted for now, same posture as the `ttf-parser` entry in §4.3.** `tiny-skia`'s own internal fixed-point/scanline conversion evidently has other undocumented magnitude limits beyond the one bisected here; the clamp closes the *specific* reachable path (device-space coordinates from content-stream operands/CTM), but a coordinate reaching `tiny-skia` through some other, not-yet-audited path in this crate (e.g. a glyph outline scaled by an extreme `/FontMatrix`, or an extreme image transform) could in principle still exceed a similar internal limit through a code path this pass didn't specifically re-verify against the clamp. Re-review trigger: any further `render_interpreter` fuzz crash inside `tiny-skia` itself (as opposed to this crate's own code) should get its coordinate source added to `sanitize_point`'s call sites (already used by `to_device`, glyph outline conversion, `paint_placeholder_rect`, and `form_bbox_mask` -- see `path.rs`'s call sites) or, if genuinely unreachable through that helper, its own dedicated clamp |

### 4.7 Signatures / encryption (`src/signatures/*`, `src/encryption/*`)

`MAX_CHAIN_DEPTH = 16` (certificate chain, `signatures/chain.rs:38`),
`MAX_DSS_OBJECTS = 4096` (`signatures/dss.rs:69`), `MAX_SIGNATURE_SIZE`/
`MAX_RESPONSE_LEN = 1 MiB` (`signer.rs:23`, `timestamp.rs:54`) bound
attacker-influenced structures here too (a malicious PDF can embed a
certificate chain or DSS dictionary; an RFC 3161 TSA response is bounded
even though it comes from a caller-configured, not attacker-configured,
server). The `rsa` crate's known timing side channel is addressed as a
supply-chain item in §7.1, not here (it's a dependency-level accepted risk,
not something this module's code can mitigate on its own given no patched
version exists).

## 5. Fuzzing

Five permanent `cargo fuzz` targets live in `fuzz/fuzz_targets/`, each
gated to the minimal feature set it needs (`fuzz/Cargo.toml`):

| Target | Exercises | Corpus |
|---|---|---|
| `parse_pdf` | Full document parse (`parser` feature) | `fuzz/corpus/parse_pdf/` |
| `parse_inline_image` | Inline-image (`BI`/`ID`/`EI`) content-stream operator parsing | `fuzz/corpus/parse_inline_image/` |
| `decode_filters` | `filter::decode_filter` over all supported filter names/params | `fuzz/corpus/decode_filters/` |
| `font_load` | `font::truetype::TrueTypeFont::load` (added this phase) | `fuzz/corpus/font_load/` |
| `render_interpreter` | `render::native::render_content_stream` (`native-render` feature) -- the content-stream interpreter itself, driven directly rather than via full-document parsing, against a fixed hostile `/Resources` (mutually-recursive Form XObjects, a self-referential Type 3 font, a self-referential ExtGState `/SMask` group) so fuzzer-mutated bytes can reach every recursive construct without wasting mutation budget on dictionary syntax `parse_pdf` already covers. Added in the "Security Hardening" phase specifically because this interpreter is new attack surface Pdfium's own fuzzing history never covered (see §4.6a) | `fuzz/corpus/render_interpreter/` |

Run locally (requires nightly, already installed in this environment):

```
cargo +nightly fuzz build <target>          # smoke-check it still compiles
cargo +nightly fuzz run <target> -- -max_total_time=<seconds>
```

All five targets were confirmed to build cleanly during this phase. Running
`font_load` for even ~20 seconds reproduced a real crash (§4.3), which is
exactly the intended purpose of "permanent" fuzz targets: they are meant to
be re-run periodically (not just built once), since new inputs keep
surfacing new edge cases, especially against unmaintained dependencies like
`ttf-parser`. There is currently no CI job invoking these automatically —
running them is a manual step; wiring a scheduled (not per-PR-blocking,
given crash triage takes human judgement) fuzz job into CI is recommended
follow-up but out of scope for this pass (no `.github/workflows/` exists in
this repository at all yet).

`render_interpreter` also reproduced a real crash within its first ~5,700
executions of its first run (a finite-but-absurdly-large path coordinate
tripping an internal `tiny-skia` panic, §4.6a) -- fixed and covered by a
regression test the same phase it was found. A follow-up run of a clean
250,000 iterations (`cargo +nightly fuzz run render_interpreter --
-runs=250000 -max_len=65536 -timeout=25`, corpus seeded with hand-written
examples of every attack shape named in this phase's task: deep `q`/`Q`
nesting, path-operator floods, `Do`/`Tf`/`gs` invocations of the hostile
resources) completed with **`Done 250000 runs in 145 second(s)`, zero
crashes, zero timeouts, zero new artifacts** after that fix landed --
satisfying this phase's "≥200,000 iterations, no crash" definition of
done. Corpus coverage grew to 5637 edges / 2391 corpus entries over that
run, per libFuzzer's own coverage counters.

Crash triage process: a crash found by any target should (1) get a minimal
reproducer via `cargo fuzz tmin`, (2) get a regression test added at the
library level (see `malformed_ttc_offset_overflow_does_not_panic`,
`tiff_predictor_huge_declared_columns_does_not_bomb`, and (this phase)
`extreme_finite_path_coordinate_does_not_panic` for the pattern), (3) get a
one-line mention added to this document's §4 if it reveals a new
residual-risk category rather than reconfirming an existing one.

## 6. Resource limits — full inventory

Every `MAX_*` constant enforced against attacker-controlled sizes/counts/
depths in this crate, as of this phase (`grep -rn "^pub const MAX_\|^const MAX_" src/`):

| Constant | Value | File |
|---|---|---|
| `MAX_XREF_SECTIONS` | 4096 | `parser/mod.rs` |
| `MAX_NESTING_DEPTH` | 64 | `parser/objects.rs` |
| `MAX_PAGE_TREE_WALK_NODES` | 500,000 | `parser/mod.rs` |
| `MAX_PAGE_TREE_WALK_DEPTH` | 64 | `parser/mod.rs` |
| `MAX_RECOVERED_OBJECTS` | 2,000,000 | `parser/recovery.rs` |
| `MAX_INLINE_DICT_ENTRIES` | 256 | `parser/inline_image.rs` |
| `MAX_DECODED_SIZE` | 512 MiB | `filter/mod.rs` (also now used by `object/stream.rs::decompress`) |
| `MAX_CODE_WIDTH` / `MAX_TABLE_SIZE` | 12 bits / 4096 | `filter/lzw.rs` |
| `MAX_FONT_SIZE_BYTES` | 64 MiB | `font/truetype.rs` |
| `MAX_GLYPH_COUNT` | 200,000 | `font/truetype.rs` |
| `MAX_CMAP_BYTES` / `MAX_CMAP_ENTRIES` | 16 MiB / 2,000,000 | `font/tounicode.rs` |
| `MAX_CONTENT_STREAM_BYTES` | 256 MiB | `editor/content_ops.rs` |
| `MAX_PAGE_TREE_NODES` / `MAX_PAGE_TREE_DEPTH` | 500,000 / 64 | `editor/graph.rs` |
| `MAX_REACHABLE_OBJECTS` | 5,000,000 | `editor/graph.rs` |
| `MAX_IMPORT_OBJECTS` | 2,000,000 | `editor/pages.rs` |
| `MAX_OUTLINE_NODES` | 200,000 | `editor/pages.rs`, `editor/outline.rs` |
| `MAX_PARENT_DEPTH` | 64 / 1,000 | `editor/forms.rs`, `editor/outline.rs` |
| `MAX_ANNOTS_PER_PAGE` | 100,000 | `editor/annotations.rs` |
| `MAX_STRUCT_NODES` | 200,000 | `editor/structure.rs` |
| `MAX_ICC_PROFILE_BYTES` | 32 MiB | `editor/icc.rs` |
| `MAX_XMP_BYTES` | 4 MiB | `editor/xmp.rs` |
| `MAX_AUDIT_LOG_BYTES` / `MAX_AUDIT_ENTRIES` / `MAX_FIELD_BYTES` | 8 MiB / 100,000 / 64 KiB | `editor/audit.rs` |
| `MAX_CID_WIDTH_RANGE` / `MAX_CID_WIDTH_ENTRIES` | 70,000 / 500,000 | `editor/redact.rs` |
| `MAX_RENDER_PIXELS` | 64,000,000 | `render/mod.rs` |
| `MAX_GRAPHICS_STATE_DEPTH` | 4096 | `render/native/interpreter.rs` |
| `MAX_FORM_XOBJECT_DEPTH` | 12 | `render/native/interpreter.rs` |
| `MAX_TYPE3_DEPTH` | 6 | `render/native/font.rs` |
| `MAX_WARNINGS` | 1000 | `render/native/interpreter.rs` |
| `MAX_OPERATOR_COUNT` | 2,000,000 (new this phase) | `render/native/interpreter.rs` |
| `MAX_RENDER_DURATION` | 20s wall-clock (new this phase) | `render/native/interpreter.rs` |
| `MAX_PATH_POINTS_PER_PATH` | 1,000,000 (new this phase) | `render/native/path.rs` |
| `MAX_COORDINATE_MAGNITUDE` | 1,000,000.0 device-space units (new this phase, fuzz-found) | `render/native/path.rs` |
| `MAX_CHAIN_DEPTH` | 16 | `signatures/chain.rs` |
| `MAX_DSS_OBJECTS` | 4096 | `signatures/dss.rs` |
| `MAX_SIGNATURE_SIZE` / `MAX_RESPONSE_LEN` | 1 MiB / 1 MiB | `signatures/signer.rs`, `signatures/timestamp.rs` |

This is not a fixed/perfect list — new attacker-influenced size/count/depth
fields should get their own named `MAX_*` constant with a doc comment
explaining what it bounds, following the pattern above, rather than a
magic number.

## 7. Supply-chain / dependency risk (`cargo audit`)

Every `.cargo/audit.toml` `ignore` entry below is a reviewed, per-crate
exception with an explicit re-review trigger — not a blanket category
suppression. `informational_warnings` lists all three RustSec informational
categories (`unmaintained`, `unsound`, `notice`) explicitly (this is also
`cargo-audit`'s own default when the key is omitted — spelling it out here
is so a future *new* advisory in any of these categories, for a crate not
already reviewed below, still shows up in `cargo audit`'s output rather
than being silently hidden; it does not, by itself, fail the build — no
`-D`/`--deny` flag is currently wired into any CI job).

### 7.1 `RUSTSEC-2023-0071` — `rsa 0.9.10` ("Marvin Attack" timing side channel)

No patched version exists upstream. `rsa` is used only for
verifying/generating detached CMS/PKCS#7 signatures over already-public
document hashes (`src/signatures/*`) — it never performs RSA *decryption*
of secret data, and the private key (when generating a signature) is
supplied by the caller/HSM, not decrypted by this code from attacker
input, so the network-observable timing side channel does not apply to
this crate's usage. Re-review when a patched `rsa` ships.

### 7.2 `RUSTSEC-2026-0192` — `ttf-parser 0.25.1` (unmaintained)

`ttf-parser` is a **real, actively-used production dependency** (the
`fonts` feature's embedded-font loader, `src/font/truetype.rs`) that
parses fully attacker-controlled bytes (`FontFile2`/`FontFile3` from a PDF).
"Unmaintained" here means no further security fixes will land upstream —
this phase's fuzzing already found one instance of exactly the failure
mode that matters (a panic on malformed input, §4.3), confirming the risk
is not hypothetical. Accepted for now because:

- It is still the most complete, spec-compliant sfnt/OpenType parser
  available in the Rust ecosystem (see `font/truetype.rs` module docs on
  why this crate does not hand-roll its own); switching parsers is a
  multi-week effort (re-validating every table this crate reads: `cmap`,
  `hhea`/`hmtx`, `OS/2`, `post`, `name`, `glyf`/`CFF` presence) for a
  benefit (a maintained upstream) that a fork of `ttf-parser` under new
  maintenance could equally provide without a full rewrite.
- The concrete failure mode found (a panic) is now mitigated at this
  crate's call site via `catch_unwind` (§4.3), bounding the *impact* even
  though the *root cause* remains unmaintained.

**Re-review trigger:** any of — (a) a maintained fork/successor emerges
(watch the advisory's "see also" links), (b) a *memory-safety* (not just
panic) issue is reported against `ttf-parser` (a `catch_unwind` cannot
recover from unsound behavior that already corrupted memory before
panicking), (c) `font_load` fuzzing turns up a crash that is not a clean
Rust panic (e.g. a sanitizer-detected out-of-bounds read/write).

### 7.3 `RUSTSEC-2026-0173` — `proc-macro-error2 2.0.1` (unmaintained)

Reachable only via `lopdf` (a **dev-dependency**, used solely for
interoperability/round-trip tests — `Cargo.toml` `[dev-dependencies]`) →
`jiff` → `defmt-macros`. Verified via `cargo tree -i proc-macro-error2
--all-features` and `cargo tree -p jiff` (no active edges printed for
either): `defmt` is one of `jiff`'s *optional* dependencies (`jiff`'s
`Cargo.toml`, `defmt = ["dep:defmt"]`), gated behind a Cargo feature that
neither `lopdf` nor this crate enables. It is present in `Cargo.lock`
purely because Cargo's lock file records a resolvable version for every
optional dependency edge in the graph, not because it is ever compiled
into any artifact this crate produces (library, tests, or fuzz targets).
Consequently `proc-macro-error2` (a proc-macro, so relevant only at
*build* time of whatever enables it) never actually runs in this crate's
build. **Re-review trigger:** if `lopdf` or any other dependency starts
requesting `jiff`'s `defmt` feature (would show up as a new edge in
`cargo tree -i proc-macro-error2`), re-evaluate whether it's still
inert.

### 7.4 `RUSTSEC-2026-0195` / `RUSTSEC-2026-0194` — `quick-xml 0.39.4` (DoS in `NsReader`/attribute parsing)

Both advisories (published 2026-06-29) describe the same class of problem:
`quick-xml < 0.41.0` lets a hostile XML document force pathological cost on
the *parser itself* while it is still parsing — RUSTSEC-2026-0195 is an
unbounded heap allocation per start-tag namespace declaration in
`NsReader`/`NamespaceResolver::push` (no consumer-visible size cap before the
event is even returned), and RUSTSEC-2026-0194 is an `O(N²)`
duplicate-attribute-name scan in `BytesStart::attributes()`/`NsReader` for a
start tag with `N` attributes. Both were reported as reachable by a remote,
unauthenticated attacker in a real-world XML-over-the-network consumer
(NLnet Labs Routinator parsing a crafted RRDP `snapshot.xml`).

**Reachability in this crate.** `quick-xml` is pulled in transitively and
*only* via `tauri` → `tauri-codegen`/`tauri-utils` → `plist 1.9.0` (verified
with `cargo tree -i quick-xml --all-features`, single path, no other edge).
Two things about how `plist` is actually exercised matter here:

- `tauri-utils::config::file_associations_plist` only *builds* a
  `plist::Value` from the Tauri app's own `tauri.conf.json` file-association
  list (developer-authored config) — no XML is parsed on this path at all,
  so neither advisory (both are parse-time cost blow-ups) applies to it.
- The one call that does parse XML, `plist::Value::from_file` in
  `tauri-codegen`'s `context.rs` (feeding `tauri::generate_context!()`),
  runs inside the **proc-macro expansion of the downstream Tauri
  application's own build** — i.e. at `cargo build` time of the app that
  embeds this library, reading a local `Info.plist` file that lives in that
  app's own source tree and is authored by the same developer building the
  app. It never runs against this crate's actual runtime attack surface
  (§2: "whoever produced the PDF file (or an embedded font/ICC/JPEG payload
  inside it) that this process opens"). A build already trusts its own
  build-time inputs and build scripts completely (a malicious `Info.plist`
  committed to the app's own repo is no more dangerous than a malicious
  `build.rs`), so the DoS-via-untrusted-XML scenario both advisories
  describe (a remote/unauthenticated attacker feeding crafted XML into a
  live parsing service) does not exist on this path.
- This crate's own XMP metadata handling (`src/editor/xmp.rs`, which *does*
  parse attacker-controlled bytes from inside a PDF, per §2) does not use
  `quick-xml` at all — confirmed by `grep -rn "quick_xml\|quick-xml"
  src/editor/xmp.rs` returning nothing; it is a separate, from-scratch
  implementation. So the one place this crate parses untrusted XML-like
  content at runtime does not go through the vulnerable code at all.

**No upgrade currently available.** `plist 1.9.0` (the latest version on
crates.io as of this review, published 2026-04-26, predating both
advisories) pins `quick_xml = "^0.39.2"`, which excludes the patched
`>=0.41.0` line by semver (0.x caret rules: `^0.39.2` means `>=0.39.2,
<0.40.0`). Confirmed directly: `cargo update -p quick-xml --precise
0.41.0` fails resolution with "failed to select a version for the
requirement `quick-xml = "^0.39.2"`" required by `plist v1.9.0`. Forcing it
via a `[patch.crates-io]` override would require compiling `plist` against
a `quick-xml` minor version it was never written or tested against — an
unreviewed, unofficial fork-equivalent, out of scope for a dependency
triage.

**Re-review trigger:** `plist` publishes a version whose manifest allows
`quick-xml >= 0.41`, at which point `cargo update -p plist` (or the next
routine `tauri` bump that pulls it in) resolves this normally and the two
`RUSTSEC-2026-0195`/`RUSTSEC-2026-0194` entries in `.cargo/audit.toml`'s
`ignore` list should be removed.

## 8. Known residual risk / explicitly out of scope

Carried over from the prior audit phase (`ARCHITECTURE.md` §9/§10) and
still accurate as of this phase, **except where updated below**:

- **Update: this crate now has its own content-stream interpreter/
  rasterizer.** The optional `render`/`native-render` features were
  migrated off an earlier FFI wrapper around Google's Pdfium to a
  from-scratch, pure-Rust implementation (`render::native` +
  `render::PdfRenderer`; see §4.6). This removes the "a large, separate
  C++ codebase outside this crate's control" residual risk Pdfium carried,
  but introduces the residual risk of bugs in this crate's own,
  newly-written interpreter -- see §4.6 for how that risk is bounded and
  handled (fail-closed on every known compatibility gap, never a panic).
- **No "repair mode" parser** tolerant of the many ways real-world
  producers violate the spec (broken xref, missing `endobj`, byte-offset
  drift). `parser::recovery` provides a bounded best-effort object scan
  (§4.1) but this is not qpdf/mupdf-grade repair.
- **Filter completeness**: LZW, ASCII85/Hex, RunLength, Flate, DCT, and
  CCITT are implemented; JBIG2 and JPX (JPEG2000) are not.
- Reaching genuine feature/robustness parity with a mature C/C++ engine
  (pdfium/MuPDF/poppler/qpdf) — arbitrary third-party file compatibility
  including malformed/repairable files learned from decades of real-world
  test corpora — remains, per the prior audit's estimate, an **order of
  12–24+ person-months** effort, not something addressed by this hardening
  phase. This phase's scope was narrower and concrete: fuzzing, resource
  limits, unsafe-block auditing, dependency-audit hygiene, and this
  document — all of which are now in place.

## 9. Maintenance

Revisit this document when: a new fuzz-found crash reveals a residual-risk
category not listed in §4; a new dependency advisory needs a §7 entry; a
new attacker-controlled size/count/depth field is added anywhere in the
crate (add it to §6); or the `render` trust boundary changes again (e.g.
sandboxing is added at the application level, or a future change
reintroduces a native/FFI dependency).
