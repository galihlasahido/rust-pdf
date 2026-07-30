//! PDF parsing module.
//!
//! This module provides functionality for reading and parsing existing PDF documents.
//!
//! # Example
//!
//! ```ignore
//! use rust_pdf::parser::PdfReader;
//!
//! let reader = PdfReader::from_file("document.pdf")?;
//! println!("Page count: {}", reader.page_count());
//! ```
//!
//! # Large-file streaming
//!
//! [`PdfReader`] never materializes the whole object graph. Opening a
//! document ([`PdfReader::from_file`]/[`PdfReader::from_bytes`]) only parses
//! the header, the cross-reference table/stream chain (ISO 32000-1 7.5.4 /
//! 7.5.8), and the trailer -- all small relative to file size. Every actual
//! PDF object (page dictionaries, content streams, fonts, images, ...) is
//! parsed lazily, on first access, straight from its xref-table byte offset
//! ([`PdfReader::resolve_reference`]), and cached (see "Thread safety"
//! below) so repeat access doesn't re-parse.
//!
//! [`PdfReader::from_file`] additionally memory-maps the file (via
//! [`memmap2`]) instead of reading it into a heap buffer, so opening a
//! multi-gigabyte PDF does not require multi-gigabyte process memory up
//! front; unread regions of the file are backed by the OS page cache, not
//! this process's private memory.
//!
//! [`PdfReader::page_ref`]/[`PdfReader::get_page`] locate the Nth page by
//! descending the page tree using each `/Pages` node's `/Count` entry (ISO
//! 32000-1 7.7.3.2, Table 29) to skip whole sibling subtrees without
//! resolving anything inside them, rather than walking/parsing every page
//! from 0 to N-1. For the common case of a single, flat `/Kids` array (every
//! child is itself a leaf page) this is a direct array index -- no other
//! object is touched at all.
//!
//! # Thread safety
//!
//! [`PdfReader`] is `Send + Sync` and cheap to share: wrap it in an
//! [`std::sync::Arc`] and call [`PdfReader::resolve_reference`]/
//! [`PdfReader::get_page`]/etc. from as many threads as you like (e.g. one
//! per page being rendered/exported concurrently). Its object cache is an
//! [`std::sync::RwLock`], not a per-thread copy, so a page resolved by one
//! thread is visible (and not re-parsed) by another.

#[cfg(feature = "encryption")]
mod decrypt;
mod inline_image;
mod lexer;
mod objects;
mod recovery;
mod trailer;
mod xref;

pub use inline_image::{parse_inline_image, InlineImage};
pub use trailer::Trailer;
pub use xref::{XrefEntry, XrefTable};

// Re-exported for `crate::editor`: incremental save (ISO 32000-1 7.5.6)
// needs to chain a new update's `/Prev` to the byte offset of the base
// file's own final `startxref`, and this is the already-tested routine
// that locates it. Not part of the public API.
pub(crate) use xref::find_startxref;

use crate::document::PdfVersion;
use crate::error::{ParserError, PdfResult};
use crate::object::{Object, PdfDictionary};
use crate::types::ObjectId;
use memmap2::Mmap;
use objects::parse_indirect_object;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Arc, RwLock};
use trailer::parse_trailer;
use xref::parse_xref_table;

/// Hard cap on the number of `/Prev`/`/XRefStm`-linked cross-reference
/// sections that will be followed. Real files rarely have more than a
/// handful of incremental updates; this bounds work (and, combined with
/// the visited-offset check, guarantees termination) even on a file whose
/// `/Prev` chain is corrupt or maliciously long.
const MAX_XREF_SECTIONS: usize = 4096;

/// Extracts a hybrid-reference file's `/XRefStm` offset (ISO 32000-1
/// 7.5.8.4) from a trailer dictionary, if present.
fn xref_stm_offset(dict: &PdfDictionary) -> Option<u64> {
    match dict.get("XRefStm") {
        Some(Object::Integer(n)) if *n >= 0 => Some(*n as u64),
        _ => None,
    }
}

/// Bound on the total number of node dictionaries resolved while locating a
/// single page by index ([`PdfReader::page_ref`]/[`PdfReader::get_page`]).
/// This is a *total* budget shared across the whole walk (not a per-branch
/// counter), so even an adversarial `/Kids` structure (deliberately-wrong
/// `/Count`, pathological fan-out) cannot force unbounded work -- it just
/// makes the lookup fail (return `None`) once the budget is exhausted,
/// rather than hang or allocate without bound. Mirrors the bound
/// `crate::editor::graph::EditableDocument` uses for its own (full,
/// eager) page-tree walk.
const MAX_PAGE_TREE_WALK_NODES: usize = 500_000;

/// Bound on `/Kids` nesting depth followed while locating a page by index.
const MAX_PAGE_TREE_WALK_DEPTH: u32 = 64;

/// Backing storage for a [`PdfReader`]'s raw file bytes.
///
/// [`PdfReader::from_file`] memory-maps the file so opening even a
/// multi-gigabyte PDF does not require reading the whole file into the
/// process's heap up front, and so that the OS page cache -- not this
/// process's private memory -- backs any region of the file this reader
/// never actually touches (e.g. objects on pages the caller never visits).
/// [`PdfReader::from_bytes`] instead keeps ownership of a caller-supplied
/// in-memory buffer, for callers that already have the bytes (e.g.
/// downloaded content, tests).
///
/// Both variants deref to `&[u8]`, so every existing offset-based parsing
/// routine in this module works unchanged regardless of which backing is in
/// use.
enum DataSource {
    /// Bytes owned directly by this process (from [`PdfReader::from_bytes`]).
    Owned(Vec<u8>),
    /// A read-only memory mapping of a file (from [`PdfReader::from_file`]).
    Mapped(Mmap),
}

impl std::ops::Deref for DataSource {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            DataSource::Owned(v) => v,
            DataSource::Mapped(m) => m,
        }
    }
}

impl std::fmt::Debug for DataSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataSource::Owned(v) => f.debug_tuple("Owned").field(&v.len()).finish(),
            DataSource::Mapped(m) => f.debug_tuple("Mapped").field(&m.len()).finish(),
        }
    }
}

/// A fully decoded object stream (ISO 32000-1 7.5.7), cached by
/// [`PdfReader::resolve_compressed_object`] so that resolving several
/// compressed objects out of the same `/ObjStm` only pays the Flate
/// decompression cost (and the cost of re-parsing the `/ObjStm`'s own
/// indirect-object header) once, not once per compressed object requested.
#[derive(Debug)]
struct DecodedObjectStream {
    /// Fully filter-decoded stream data (header pairs followed by the N
    /// object bodies, per 7.5.7).
    data: Vec<u8>,
    /// Byte offset within `data` where the first object body begins
    /// (`/First`).
    first: usize,
    /// Number of objects in the stream (`/N`).
    num_objects: usize,
}

/// A PDF document reader.
///
/// Provides read-only access to PDF document structure and content.
///
/// See the [module docs](crate::parser) for the lazy-loading/memory-mapping
/// design and thread-safety guarantees: this type is `Send + Sync` and is
/// meant to be shared behind an [`Arc`] across multiple rendering/export
/// threads.
#[derive(Debug)]
pub struct PdfReader {
    /// Raw PDF data (owned buffer or memory-mapped file).
    data: DataSource,
    /// PDF version.
    version: PdfVersion,
    /// Cross-reference table.
    xref: XrefTable,
    /// Trailer information.
    trailer: Trailer,
    /// Cache of already-resolved top-level indirect objects, keyed by id.
    /// An [`RwLock`] (rather than requiring `&mut self`) so many threads
    /// can call [`PdfReader::resolve_reference`] concurrently on a single
    /// shared `Arc<PdfReader>` -- see the module-level "Thread safety"
    /// docs.
    object_cache: RwLock<HashMap<ObjectId, Object>>,
    /// Cache of already-decoded object streams (ISO 32000-1 7.5.7), keyed
    /// by the stream's own object number. See [`DecodedObjectStream`].
    object_stream_cache: RwLock<HashMap<u32, Arc<DecodedObjectStream>>>,
    /// The recovered file encryption key/algorithm for an encrypted
    /// document opened via [`PdfReader::from_bytes_with_password`]/
    /// [`PdfReader::from_file_with_password`], or `None` for an
    /// unencrypted document (or one opened via the plain
    /// [`PdfReader::from_bytes`]/[`PdfReader::from_file`] constructors,
    /// which reject any `/Encrypt` trailer entry outright -- see
    /// [`PdfReader::finish`]). When set, every object handed back by
    /// [`PdfReader::resolve_reference`]/[`PdfReader::resolve_compressed_object`]
    /// has already been transparently decrypted (see `decrypt` module
    /// docs for the two supported algorithms and why this can't reuse
    /// `crate::encryption::EncryptionHandler` directly).
    #[cfg(feature = "encryption")]
    decryptor: Option<decrypt::Decryptor>,
}

// Compile-time guarantee that `PdfReader` can be shared across threads
// (e.g. wrapped in `Arc<PdfReader>` and used to render/export multiple
// pages concurrently). If a future field addition silently breaks this
// (e.g. reintroducing an `Rc`/`Cell`), this fails to compile rather than
// only failing at a call site far from the actual cause.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PdfReader>();
};

impl PdfReader {
    /// Opens a PDF file for reading via a memory-mapped, read-only view of
    /// its bytes (see the [module docs](crate::parser) "Large-file
    /// streaming" section) rather than reading the whole file into a heap
    /// buffer. This is the entry point large-file/desktop-viewer callers
    /// should use.
    pub fn from_file(path: impl AsRef<Path>) -> PdfResult<Self> {
        let file = fs::File::open(path)?;

        // SAFETY: `memmap2::Mmap::map` is `unsafe` because the OS gives no
        // guarantee the mapped file won't be mutated or truncated by
        // another process/thread while it's mapped, which could in
        // principle change the bytes a concurrent reader observes mid-read.
        // This is the standard, documented caveat of memory-mapped I/O (and
        // applies equally to every production PDF viewer that mmaps its
        // input). We only ever read through the mapping (never write), and
        // every parsing routine in this module already treats file bytes
        // as untrusted and bounds-checks every offset/length taken from
        // them (per this crate's mandatory rules) -- so a mid-read external
        // mutation can at worst produce a `ParserError`/garbled parse of
        // already-untrusted data, not a Rust-level memory-safety violation:
        // the OS still guarantees the mapped address range stays backed by
        // *some* valid (if possibly stale/changed) memory for the mapping's
        // whole lifetime.
        let mmap = unsafe { Mmap::map(&file)? };

        Self::open(DataSource::Mapped(mmap))
    }

    /// Opens a PDF already held in memory for reading.
    ///
    /// Prefer [`PdfReader::from_file`] for large files: it avoids copying
    /// the whole file into this buffer up front.
    pub fn from_bytes(data: Vec<u8>) -> PdfResult<Self> {
        Self::open(DataSource::Owned(data))
    }

    /// Like [`PdfReader::from_file`], but for an encrypted document: `password`
    /// is used to derive the file encryption key from the trailer's
    /// `/Encrypt` dictionary (ISO 32000-1 §7.6 / ISO 32000-2 §7.6), and every
    /// string/stream subsequently resolved is transparently decrypted.
    ///
    /// Only the two algorithms [`crate::editor::EditableDocument::save_encrypted_to_bytes`]
    /// can itself produce are supported: AES-128 (`/V 4 /R 4`, `AESV2`) and
    /// AES-256 (`/V 5 /R 6`, `AESV3`) -- see the crate-internal `decrypt`
    /// module's docs for why, and for the exact scope. Anything else fails
    /// with [`ParserError::UnsupportedEncryption`] rather than being
    /// silently mis-decrypted.
    ///
    /// If the document is **not** actually encrypted, `password` is simply
    /// ignored and this behaves exactly like [`PdfReader::from_file`].
    ///
    /// # Errors
    /// - [`ParserError::IncorrectPassword`] if the document *is* encrypted
    ///   and `password` does not match.
    /// - [`ParserError::UnsupportedEncryption`] if the document uses an
    ///   encryption scheme this crate does not implement (e.g. legacy RC4,
    ///   `/V 1`/`/V 2`).
    #[cfg(feature = "encryption")]
    pub fn from_file_with_password(path: impl AsRef<Path>, password: &str) -> PdfResult<Self> {
        let file = fs::File::open(path)?;
        // SAFETY: see [`PdfReader::from_file`]'s identical `Mmap::map` call.
        let mmap = unsafe { Mmap::map(&file)? };
        Self::open_with_password(DataSource::Mapped(mmap), password)
    }

    /// Like [`PdfReader::from_bytes`], but for an encrypted document -- see
    /// [`PdfReader::from_file_with_password`]'s docs for the full contract
    /// (supported algorithms, error cases, and the no-op behavior on an
    /// unencrypted document).
    #[cfg(feature = "encryption")]
    pub fn from_bytes_with_password(data: Vec<u8>, password: &str) -> PdfResult<Self> {
        Self::open_with_password(DataSource::Owned(data), password)
    }

    /// Shared implementation of [`PdfReader::from_file`]/
    /// [`PdfReader::from_bytes`].
    ///
    /// If the cross-reference table cannot be located or parsed (a
    /// corrupt/truncated file, or one with a broken `/Prev` chain), this
    /// falls back to recovery mode: a linear scan of the file for `obj`
    /// headers (see the crate-internal `recovery` module for details).
    /// This mirrors the behaviour of production PDF readers, which never
    /// simply give up on a broken xref table when the objects themselves
    /// are still present in the file.
    fn open(data: DataSource) -> PdfResult<Self> {
        let (version, xref, trailer) = Self::parse_structure(&data)?;
        Self::finish(data, version, xref, trailer)
    }

    /// Shared implementation of [`PdfReader::from_file_with_password`]/
    /// [`PdfReader::from_bytes_with_password`]. See [`Self::open`] for the
    /// xref/trailer-parsing (with recovery-mode fallback) this shares.
    #[cfg(feature = "encryption")]
    fn open_with_password(data: DataSource, password: &str) -> PdfResult<Self> {
        let (version, xref, trailer) = Self::parse_structure(&data)?;
        Self::finish_with_password(data, version, xref, trailer, password)
    }

    /// Locates and parses the header/cross-reference table/trailer shared
    /// by every constructor (falling back to recovery mode -- see
    /// [`Self::open`]'s docs -- when the xref table itself can't be
    /// located/parsed).
    fn parse_structure(data: &DataSource) -> PdfResult<(PdfVersion, XrefTable, Trailer)> {
        let version = Self::parse_header(data)?;

        let parsed = find_startxref(data)
            .ok()
            .and_then(|offset| Self::parse_xref_and_trailer(data, offset).ok());

        let (xref, trailer) = match parsed {
            Some(result) => result,
            None => recovery::recover(data).ok_or(ParserError::InvalidTrailer)?,
        };

        Ok((version, xref, trailer))
    }

    /// Like [`PdfReader::from_bytes`], but always uses recovery-mode
    /// reconstruction (ignoring any cross-reference table present),
    /// regardless of whether the file's own xref table would parse. This
    /// is useful for diagnosing/repairing files whose xref table parses
    /// "successfully" but points at the wrong offsets.
    pub fn from_bytes_recovery_only(data: Vec<u8>) -> PdfResult<Self> {
        let data = DataSource::Owned(data);
        let version = Self::parse_header(&data)?;
        let (xref, trailer) = recovery::recover(&data).ok_or(ParserError::InvalidTrailer)?;

        Self::finish(data, version, xref, trailer)
    }

    /// Final validation (rejects encrypted documents -- no password was
    /// supplied to decrypt them) and struct construction shared by every
    /// no-password constructor ([`PdfReader::from_file`]/
    /// [`PdfReader::from_bytes`]/[`PdfReader::from_bytes_recovery_only`]).
    /// Unchanged from before password support existed: these constructors'
    /// behavior on an encrypted document is deliberately left exactly as
    /// it was (fail with [`ParserError::EncryptedPdf`]) -- see
    /// [`PdfReader::finish_with_password`] for the new password-aware path.
    fn finish(
        data: DataSource,
        version: PdfVersion,
        xref: XrefTable,
        trailer: Trailer,
    ) -> PdfResult<Self> {
        if trailer.encrypt.is_some() {
            return Err(ParserError::EncryptedPdf.into());
        }

        Ok(Self {
            data,
            version,
            xref,
            trailer,
            object_cache: RwLock::new(HashMap::new()),
            object_stream_cache: RwLock::new(HashMap::new()),
            #[cfg(feature = "encryption")]
            decryptor: None,
        })
    }

    /// Struct construction for the password-aware constructors
    /// ([`PdfReader::from_file_with_password`]/
    /// [`PdfReader::from_bytes_with_password`]).
    ///
    /// If the trailer has no `/Encrypt` entry at all, `password` is simply
    /// unused (matches [`PdfReader::finish`]'s behavior for an unencrypted
    /// document, just without rejecting anything). Otherwise, resolves the
    /// `/Encrypt` dictionary itself (never encrypted, so this is safe to do
    /// with `decryptor` still unset) and attempts to recover the file
    /// encryption key for `password` via [`decrypt::Decryptor::from_encrypt_dict`].
    #[cfg(feature = "encryption")]
    fn finish_with_password(
        data: DataSource,
        version: PdfVersion,
        xref: XrefTable,
        trailer: Trailer,
        password: &str,
    ) -> PdfResult<Self> {
        let mut reader = Self {
            data,
            version,
            xref,
            trailer,
            object_cache: RwLock::new(HashMap::new()),
            object_stream_cache: RwLock::new(HashMap::new()),
            decryptor: None,
        };

        let Some(encrypt_id) = reader.trailer.encrypt else {
            return Ok(reader);
        };

        // Safe to resolve normally: `decryptor` is still `None`, so this
        // one call returns the dictionary's raw (correctly, always
        // unencrypted -- ISO 32000-1 7.6.1) bytes, and the result is
        // cached under `encrypt_id` for the lifetime of `reader`, so no
        // later `resolve_reference(encrypt_id)` call can accidentally run
        // it back through a (by-then-populated) decryptor either.
        let encrypt_obj = reader
            .resolve_reference(encrypt_id)
            .ok_or(ParserError::InvalidTrailer)?;
        let dict = match encrypt_obj {
            Object::Dictionary(d) => d,
            _ => return Err(ParserError::InvalidTrailer.into()),
        };

        let file_id = reader
            .trailer
            .id
            .as_ref()
            .map(|(first, _)| first.as_slice());
        let decryptor = decrypt::Decryptor::from_encrypt_dict(&dict, file_id, password)?;
        reader.decryptor = Some(decryptor);

        Ok(reader)
    }

    /// Parses the PDF header to get the version.
    fn parse_header(data: &[u8]) -> Result<PdfVersion, ParserError> {
        if data.len() < 8 {
            return Err(ParserError::InvalidHeader);
        }

        // Header format: %PDF-X.Y
        if !data.starts_with(b"%PDF-") {
            return Err(ParserError::InvalidHeader);
        }

        let version_str =
            std::str::from_utf8(&data[5..8]).map_err(|_| ParserError::InvalidHeader)?;

        PdfVersion::try_from(version_str).map_err(|_| ParserError::InvalidHeader)
    }

    /// Parses the xref table and trailer, following the chain of
    /// incremental updates via `/Prev` (ISO 32000-1 7.5.4) and, for
    /// hybrid-reference files, the supplementary cross-reference stream
    /// named by `/XRefStm` (7.5.8.4).
    ///
    /// Entries from more recent revisions always take precedence over
    /// older ones (each section is merged with "first write wins", and
    /// sections are visited newest-first).
    fn parse_xref_and_trailer(
        data: &[u8],
        start_offset: u64,
    ) -> Result<(XrefTable, Trailer), ParserError> {
        let mut combined_xref = XrefTable::new();
        let mut final_trailer: Option<Trailer> = None;
        let mut visited: HashSet<u64> = HashSet::new();
        let mut xref_offset = start_offset;
        let mut sections = 0usize;

        loop {
            sections += 1;
            if sections > MAX_XREF_SECTIONS {
                break;
            }
            // Guard against a `/Prev` cycle (self-referential or looping
            // chain), which would otherwise never terminate.
            if !visited.insert(xref_offset) {
                break;
            }

            let Some(xref_data) = data.get(xref_offset as usize..) else {
                // Offset points outside the file. If we already have a
                // trailer from a newer revision, stop here and use what we
                // have rather than failing the whole document.
                break;
            };

            let section = Self::parse_one_xref_section(xref_data);
            let Ok((section_xref, section_trailer)) = section else {
                break;
            };

            combined_xref.merge(section_xref);

            let is_first = final_trailer.is_none();
            let prev = section_trailer.prev;
            let xref_stm = xref_stm_offset(&section_trailer.dict);

            if is_first {
                final_trailer = Some(section_trailer);
            }

            // Hybrid-reference file: the traditional table's trailer may
            // point at a supplementary xref *stream* covering the same
            // revision (needed so pre-1.5 readers can still find the
            // basic entries while 1.5+ readers also get entries for
            // objects stored in object streams). Its entries are lower
            // priority than the traditional section we just merged, but
            // higher priority than anything from `/Prev`.
            if let Some(stm_offset) = xref_stm {
                if let Some(stm_data) = data.get(stm_offset as usize..) {
                    if let Ok((_, (_, _, Object::Stream(stream)))) = parse_indirect_object(stm_data)
                    {
                        if let Ok(stm_xref) = Self::parse_xref_stream(&stream) {
                            combined_xref.merge(stm_xref);
                        }
                    }
                }
            }

            match prev {
                Some(prev_offset) => xref_offset = prev_offset,
                None => break,
            }
        }

        let trailer = final_trailer.ok_or(ParserError::InvalidTrailer)?;
        Ok((combined_xref, trailer))
    }

    /// Parses a single xref section (traditional table+trailer, or xref
    /// stream) starting at `xref_data` (which is `data[offset..]`).
    fn parse_one_xref_section(xref_data: &[u8]) -> Result<(XrefTable, Trailer), ParserError> {
        if xref_data.starts_with(b"xref") {
            let (remaining, xref) =
                parse_xref_table(xref_data).map_err(|_| ParserError::InvalidXref)?;
            let (_, trailer_dict) =
                parse_trailer(remaining).map_err(|_| ParserError::InvalidTrailer)?;
            let trailer = Trailer::from_dictionary(trailer_dict)?;
            Ok((xref, trailer))
        } else {
            let (_, (_, _, obj)) =
                parse_indirect_object(xref_data).map_err(|_| ParserError::InvalidXrefStream)?;
            match obj {
                Object::Stream(stream) => {
                    let xref = Self::parse_xref_stream(&stream)?;
                    let trailer = Trailer::from_dictionary(stream.dictionary.clone())?;
                    Ok((xref, trailer))
                }
                _ => Err(ParserError::InvalidXrefStream),
            }
        }
    }

    /// Parses a cross-reference stream (ISO 32000-1:2008 Section 7.5.8).
    fn parse_xref_stream(stream: &crate::object::PdfStream) -> Result<XrefTable, ParserError> {
        let dict = &stream.dictionary;

        // Get W array (field widths).
        let w = match dict.get("W") {
            Some(Object::Array(arr)) => arr,
            _ => return Err(ParserError::InvalidXrefStream),
        };
        if w.len() != 3 {
            return Err(ParserError::InvalidXrefStream);
        }

        // Field widths are byte counts; the spec doesn't bound them, but
        // `read_int` folds bytes into a u64 (8 bytes), and no legitimate
        // xref stream needs wider fields than that. Reject anything
        // larger rather than silently truncating high-order bytes.
        let widths: Vec<usize> = w
            .iter()
            .map(|o| match o {
                Object::Integer(n) if *n >= 0 && *n <= 8 => Ok(*n as usize),
                _ => Err(ParserError::InvalidXrefStream),
            })
            .collect::<Result<_, _>>()?;
        let (w1, w2, w3) = (widths[0], widths[1], widths[2]);
        let entry_size = w1 + w2 + w3;
        if entry_size == 0 {
            // W = [0 0 0] would otherwise make every entry zero-width,
            // turning the loop below into an unbounded insert loop.
            return Err(ParserError::InvalidXrefStream);
        }

        // Get Index array (optional, defaults to [0 Size]).
        let size = match dict.get("Size") {
            Some(Object::Integer(n)) if *n >= 0 => *n as u64,
            _ => return Err(ParserError::InvalidXrefStream),
        };

        let index: Vec<(u64, u64)> = match dict.get("Index") {
            Some(Object::Array(arr)) => {
                let mut pairs = Vec::new();
                let mut iter = arr.iter();
                while let (Some(start), Some(count)) = (iter.next(), iter.next()) {
                    match (start, count) {
                        (Object::Integer(s), Object::Integer(c)) if *s >= 0 && *c >= 0 => {
                            pairs.push((*s as u64, *c as u64));
                        }
                        _ => return Err(ParserError::InvalidXrefStream),
                    }
                }
                pairs
            }
            _ => vec![(0, size)],
        };

        // Decode stream data, honouring the full filter chain and any
        // PNG/TIFF predictor (most real-world xref streams use
        // FlateDecode with Predictor 12).
        #[cfg(feature = "compression")]
        let data = stream.decode_all()?;
        #[cfg(not(feature = "compression"))]
        let data = stream.data().to_vec();

        // Parse entries. The `data_offset + entry_size > data.len()` bound
        // check below already guarantees termination in a number of steps
        // proportional to `data.len() / entry_size` (a real byte count),
        // regardless of how large a malicious `count` claims to be.
        let mut table = XrefTable::new();
        let mut data_offset = 0usize;

        for (start, count) in index {
            let mut obj_num = start;
            for _ in 0..count {
                if data_offset + entry_size > data.len() {
                    return Err(ParserError::InvalidXrefStream);
                }
                if obj_num > u32::MAX as u64 {
                    return Err(ParserError::InvalidXrefStream);
                }

                let entry_type = if w1 == 0 {
                    1 // Default type is 1 (in use), per 7.5.8.2 Table 17.
                } else {
                    Self::read_int(&data[data_offset..data_offset + w1])
                };
                let field2 = Self::read_int(&data[data_offset + w1..data_offset + w1 + w2]);
                let field3 = Self::read_int(&data[data_offset + w1 + w2..data_offset + entry_size]);

                let entry = match entry_type {
                    0 => XrefEntry::Free {
                        next_free: field2 as u32,
                        generation: field3 as u16,
                    },
                    1 => XrefEntry::InUse {
                        offset: field2,
                        generation: field3 as u16,
                    },
                    2 => XrefEntry::Compressed {
                        object_stream: field2 as u32,
                        index: field3 as u32,
                    },
                    _ => return Err(ParserError::InvalidXrefStream),
                };

                table.insert(obj_num as u32, entry);
                data_offset += entry_size;
                obj_num = obj_num.saturating_add(1);
            }
        }

        Ok(table)
    }

    /// Reads an integer from bytes (big-endian).
    fn read_int(bytes: &[u8]) -> u64 {
        let mut result = 0u64;
        for &b in bytes {
            result = (result << 8) | (b as u64);
        }
        result
    }

    /// Returns the PDF version.
    pub fn version(&self) -> PdfVersion {
        self.version
    }

    /// Returns the number of pages in the document.
    pub fn page_count(&self) -> usize {
        self.get_page_count_from_tree().unwrap_or(0)
    }

    /// Gets the page count from the page tree.
    fn get_page_count_from_tree(&self) -> Option<usize> {
        let root = self.resolve_reference(self.trailer.root)?;

        let pages_ref = match root {
            Object::Dictionary(dict) => match dict.get("Pages") {
                Some(Object::Reference(id)) => *id,
                _ => return None,
            },
            _ => return None,
        };

        let pages = self.resolve_reference(pages_ref)?;

        match pages {
            Object::Dictionary(dict) => match dict.get("Count") {
                Some(Object::Integer(count)) => Some(*count as usize),
                _ => None,
            },
            _ => None,
        }
    }

    /// Returns the catalog (root) dictionary.
    pub fn catalog(&self) -> Option<PdfDictionary> {
        let obj = self.resolve_reference(self.trailer.root)?;
        match obj {
            Object::Dictionary(dict) => Some(dict),
            _ => None,
        }
    }

    /// Returns the document info dictionary if present.
    pub fn info(&self) -> Option<PdfDictionary> {
        let info_id = self.trailer.info?;
        let obj = self.resolve_reference(info_id)?;
        match obj {
            Object::Dictionary(dict) => Some(dict),
            _ => None,
        }
    }

    /// Returns an already-resolved object from the cache, if present.
    ///
    /// This is a cache-only lookup: it does **not** parse the object if it
    /// hasn't been resolved yet (use [`PdfReader::resolve_reference`] for
    /// that -- which also populates this cache).
    pub fn get_object(&self, id: ObjectId) -> Option<Object> {
        self.object_cache.read().ok()?.get(&id).cloned()
    }

    /// Resolves an object reference, returning the referenced object.
    ///
    /// The first call for a given `id` parses it lazily, straight from its
    /// xref-table byte offset (see the [module docs](crate::parser)); the
    /// result is cached (behind an [`RwLock`], safe to call concurrently
    /// from multiple threads sharing one `Arc<PdfReader>`) so subsequent
    /// calls for the same `id` are a cache hit.
    ///
    /// Never panics on a malformed/adversarial xref table: every byte
    /// offset taken from the (untrusted) xref data is validated against
    /// the actual file length before use.
    pub fn resolve_reference(&self, id: ObjectId) -> Option<Object> {
        // Check cache first.
        if let Some(obj) = self.get_object(id) {
            return Some(obj);
        }

        // Get xref entry
        let entry = self.xref.get(id.number)?;

        let obj = match entry {
            // `_generation` (not `generation`): only actually read when the
            // `encryption` feature is enabled (below); the leading
            // underscore keeps it from being an `unused_variables` warning
            // when that feature is off, while still being usable when it's
            // on.
            XrefEntry::InUse {
                offset,
                generation: _generation,
            } => {
                // Parse object at offset. `offset` comes straight from the
                // (untrusted) xref table/stream, so it must be checked
                // against the file length rather than indexed directly.
                let data = self.data.get(*offset as usize..)?;
                let (_, (_, _, obj)) = parse_indirect_object(data).ok()?;

                // Decrypt (ISO 32000-1 §7.6): every string/stream must be
                // transparently decrypted using *this* object's own
                // number/generation, except the `/Encrypt` dictionary
                // itself, which is never encrypted (7.6.1) -- and is only
                // ever resolved through this exact path once, while
                // `decryptor` is still `None` (see
                // `PdfReader::finish_with_password`), so this check is
                // mostly documentation of that invariant rather than
                // something expected to actually trigger here.
                #[cfg(feature = "encryption")]
                let obj = match &self.decryptor {
                    Some(d) if self.trailer.encrypt != Some(id) => {
                        d.decrypt_object(obj, id.number, *_generation)
                    }
                    _ => obj,
                };

                obj
            }
            XrefEntry::Compressed {
                object_stream,
                index,
            } => self.resolve_compressed_object(*object_stream, *index)?,
            XrefEntry::Free { .. } => return None,
        };

        // Populate the cache. `or_insert_with` (rather than an
        // unconditional `insert`) avoids clobbering a value another thread
        // may have raced in and inserted first with a redundant clone; both
        // are equal (same `id`, same underlying file), so either winner is
        // correct to return.
        if let Ok(mut cache) = self.object_cache.write() {
            let cached = cache.entry(id).or_insert_with(|| obj.clone());
            return Some(cached.clone());
        }

        Some(obj)
    }

    /// Resolves an object from a compressed object stream (ISO 32000-1
    /// Section 7.5.7). All indices/offsets taken from the object stream's
    /// header (itself untrusted data from the file) are bounds-checked
    /// before use; this function returns `None` rather than panicking on
    /// any malformed value.
    fn resolve_compressed_object(&self, stream_num: u32, index: u32) -> Option<Object> {
        let decoded = self.decoded_object_stream(stream_num)?;
        let index = index as usize;

        // Parse the header (N pairs of obj_num, offset). Cheap relative to
        // decompression: `/First` bounds this to the header region only,
        // not the whole (already-decoded, already-cached) stream.
        let header = decoded.data.get(..decoded.first)?;
        let objects_data = decoded.data.get(decoded.first..)?;

        let header_str = std::str::from_utf8(header).ok()?;
        let nums: Vec<i64> = header_str
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();

        if nums.len() < (index + 1) * 2 {
            return None;
        }

        let obj_offset = *nums.get(index * 2 + 1)?;
        if obj_offset < 0 {
            return None;
        }
        let obj_offset = obj_offset as usize;

        // Find end offset (next object's offset or end of data). Guard
        // against a corrupt/adversarial header claiming an out-of-range
        // `next_offset` too.
        let next_offset = if index + 1 < decoded.num_objects && nums.len() >= (index + 2) * 2 {
            let n = *nums.get((index + 1) * 2 + 1)?;
            if n < 0 {
                return None;
            }
            (n as usize).min(objects_data.len())
        } else {
            objects_data.len()
        };

        if obj_offset > next_offset {
            return None;
        }

        // Parse the object
        let obj_data = objects_data.get(obj_offset..next_offset)?;
        let (_, obj) = objects::parse_object(obj_data).ok()?;

        Some(obj)
    }

    /// Returns the fully filter-decoded contents of object stream
    /// `stream_num` (ISO 32000-1 7.5.7), decoding (and parsing its
    /// `/N`/`/First` header) at most once per stream regardless of how many
    /// compressed objects within it are subsequently requested, or how many
    /// threads request them concurrently.
    fn decoded_object_stream(&self, stream_num: u32) -> Option<Arc<DecodedObjectStream>> {
        if let Some(cached) = self
            .object_stream_cache
            .read()
            .ok()
            .and_then(|c| c.get(&stream_num).cloned())
        {
            return Some(cached);
        }

        // Get the object stream.
        let stream_entry = self.xref.get(stream_num)?;
        let offset = stream_entry.offset()?;

        let data = self.data.get(offset as usize..)?;
        let (_, (_, _, stream_obj)) = parse_indirect_object(data).ok()?;

        #[cfg_attr(not(feature = "encryption"), allow(unused_mut))]
        let mut stream = match stream_obj {
            Object::Stream(s) => s,
            _ => return None,
        };

        // Decrypt the object stream's own raw (still filter-encoded) bytes
        // *before* decompressing (ISO 32000-1 7.6.2: encryption is the
        // outermost transformation, applied after filtering when writing,
        // so it must be undone before filtering when reading). The
        // individual compressed objects extracted from `stream_data` below
        // are *not* separately re-decrypted -- per 7.5.7, an object stream
        // may not itself contain streams, and its member objects' strings
        // are already plaintext once the containing `/ObjStm` has been
        // decrypted here.
        #[cfg(feature = "encryption")]
        if let Some(d) = &self.decryptor {
            let generation = match stream_entry {
                XrefEntry::InUse { generation, .. } => *generation,
                _ => 0,
            };
            stream.data = d.decrypt_stream_data(&stream.data, stream_num, generation);
        }

        // Get N (number of objects) and First (offset to first object)
        let dict = &stream.dictionary;
        let num_objects = match dict.get("N") {
            Some(Object::Integer(n)) if *n >= 0 => *n as usize,
            _ => return None,
        };

        // Decode stream (full filter chain, with predictor support).
        #[cfg(feature = "compression")]
        let stream_data = stream.decode_all().ok()?;
        #[cfg(not(feature = "compression"))]
        let stream_data = stream.data().to_vec();

        let first = match dict.get("First") {
            Some(Object::Integer(f)) if *f >= 0 && (*f as usize) <= stream_data.len() => {
                *f as usize
            }
            _ => return None,
        };

        let decoded = Arc::new(DecodedObjectStream {
            data: stream_data,
            first,
            num_objects,
        });

        if let Ok(mut cache) = self.object_stream_cache.write() {
            let cached = cache.entry(stream_num).or_insert_with(|| decoded.clone());
            return Some(cached.clone());
        }

        Some(decoded)
    }

    /// Returns the object id of the page at `index` (0-based, ISO 32000-1
    /// 7.7.3.2 page-tree order), without resolving pages `0..index` first.
    ///
    /// See the [module docs](crate::parser) "Large-file streaming" section
    /// for how this stays cheap even deep into a very large page tree.
    /// Returns `None` if `index` is out of range, the catalog/page tree is
    /// malformed, or the walk hits [`MAX_PAGE_TREE_WALK_NODES`]/
    /// [`MAX_PAGE_TREE_WALK_DEPTH`] (an adversarial or badly corrupt
    /// `/Kids` structure).
    pub fn page_ref(&self, index: usize) -> Option<ObjectId> {
        let catalog = match self.resolve_reference(self.trailer.root)? {
            Object::Dictionary(d) => d,
            _ => return None,
        };
        let pages_id = match catalog.get("Pages") {
            Some(Object::Reference(id)) => *id,
            _ => return None,
        };

        let mut budget = MAX_PAGE_TREE_WALK_NODES;
        self.page_ref_at(pages_id, index, 0, &mut budget)
    }

    /// Returns the page dictionary at `index` (0-based). See
    /// [`PdfReader::page_ref`].
    pub fn get_page(&self, index: usize) -> Option<PdfDictionary> {
        match self.resolve_reference(self.page_ref(index)?)? {
            Object::Dictionary(d) => Some(d),
            _ => None,
        }
    }

    /// Recursive step of [`PdfReader::page_ref`]. `node_id` is either a
    /// `/Pages` intermediate node or a `/Page` leaf; `index` is relative to
    /// the first leaf under `node_id`.
    fn page_ref_at(
        &self,
        node_id: ObjectId,
        index: usize,
        depth: u32,
        budget: &mut usize,
    ) -> Option<ObjectId> {
        if depth > MAX_PAGE_TREE_WALK_DEPTH {
            return None;
        }
        if *budget == 0 {
            return None;
        }
        *budget -= 1;

        let dict = match self.resolve_reference(node_id)? {
            Object::Dictionary(d) => d,
            _ => return None,
        };

        let kids = match dict.get("Kids") {
            Some(Object::Array(k)) => k,
            // Leaf `/Page` node (or something with no `/Kids` at all,
            // leniently treated as a leaf like `crate::editor::graph`
            // does): it *is* index 0 relative to itself, nothing else.
            _ => return if index == 0 { Some(node_id) } else { None },
        };

        // Fast path: if this node's declared `/Count` exactly equals its
        // number of direct `/Kids`, every kid must itself be a single leaf
        // page. Proof: `/Count` (ISO 32000-1 Table 29) is the sum of each
        // kid's own leaf count, each of which is >= 1; N kids summing to
        // exactly N forces every one of them to contribute exactly 1. That
        // lets us index straight into `kids` without resolving *any* of
        // them -- the common case of a single, flat page tree (e.g. every
        // page directly under the root `/Pages` node) is therefore O(1)
        // here, not O(index).
        if let Some(Object::Integer(count)) = dict.get("Count") {
            if *count >= 0 && *count as usize == kids.len() {
                return match kids.get(index) {
                    Some(Object::Reference(id)) => Some(*id),
                    _ => None,
                };
            }
        }

        // General case (nested/unbalanced tree, or a flat-looking node
        // whose Count didn't match its Kids length): walk siblings in
        // order, using each one's own subtree leaf count to skip whole
        // subtrees without descending into them.
        let mut remaining = index;
        for kid in kids.iter() {
            let Object::Reference(kid_id) = kid else {
                continue;
            };
            let count = self.subtree_leaf_count(*kid_id, depth + 1, budget)?;
            if remaining < count {
                return self.page_ref_at(*kid_id, remaining, depth + 1, budget);
            }
            remaining -= count;
        }

        None
    }

    /// Returns the number of leaf pages under `node_id` *without*
    /// descending into it when possible: an intermediate `/Pages` node
    /// carries a `/Count` entry (ISO 32000-1 Table 29, required) that
    /// states this directly, and a node without `/Kids` is itself exactly
    /// one leaf.
    ///
    /// Falls back to actually counting a subtree's leaves (bounded by
    /// `depth`/`budget`, same as the caller) only when `/Count` is missing
    /// or not a valid non-negative integer -- i.e. only for a
    /// non-conformant producer, never for a spec-conformant file.
    fn subtree_leaf_count(
        &self,
        node_id: ObjectId,
        depth: u32,
        budget: &mut usize,
    ) -> Option<usize> {
        if depth > MAX_PAGE_TREE_WALK_DEPTH {
            return None;
        }
        if *budget == 0 {
            return None;
        }
        *budget -= 1;

        let dict = match self.resolve_reference(node_id)? {
            Object::Dictionary(d) => d,
            _ => return None,
        };

        match dict.get("Kids") {
            Some(Object::Array(kids)) => match dict.get("Count") {
                Some(Object::Integer(n)) if *n >= 0 => Some(*n as usize),
                _ => {
                    let mut total = 0usize;
                    for kid in kids.iter() {
                        if let Object::Reference(kid_id) = kid {
                            total = total.checked_add(self.subtree_leaf_count(
                                *kid_id,
                                depth + 1,
                                budget,
                            )?)?;
                        }
                    }
                    Some(total)
                }
            },
            _ => Some(1),
        }
    }

    /// Returns the raw PDF data.
    pub fn raw_data(&self) -> &[u8] {
        &self.data
    }

    /// Returns the xref table.
    pub fn xref(&self) -> &XrefTable {
        &self.xref
    }

    /// Returns the trailer.
    pub fn trailer(&self) -> &Trailer {
        &self.trailer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::PdfName;

    fn create_simple_pdf() -> Vec<u8> {
        use crate::prelude::*;

        let page = PageBuilder::a4().build();
        let doc = DocumentBuilder::new()
            .title("Test Document")
            .page(page)
            .build()
            .unwrap();

        doc.save_to_bytes().unwrap()
    }

    #[test]
    fn test_parse_simple_pdf() {
        let pdf_bytes = create_simple_pdf();
        let reader = PdfReader::from_bytes(pdf_bytes).unwrap();

        assert_eq!(reader.version(), PdfVersion::V1_7);
        assert_eq!(reader.page_count(), 1);
    }

    #[test]
    fn test_parse_header() {
        let data = b"%PDF-1.7\nrest of document";
        let version = PdfReader::parse_header(data).unwrap();
        assert_eq!(version, PdfVersion::V1_7);
    }

    #[test]
    fn test_parse_header_v2() {
        let data = b"%PDF-2.0\nrest of document";
        let version = PdfReader::parse_header(data).unwrap();
        assert_eq!(version, PdfVersion::V2_0);
    }

    #[test]
    fn test_invalid_header() {
        let data = b"not a pdf";
        let result = PdfReader::parse_header(data);
        assert!(result.is_err());
    }

    /// Regression test for a `cargo fuzz` finding (`parse_pdf` target):
    /// a corrupt file with no valid xref falls into recovery mode, which
    /// scans for `obj`/`endobj` and re-parses each candidate object -
    /// including literal strings containing an octal escape greater than
    /// `\377` (e.g. `\553`), which must wrap per ISO 32000-1 7.3.4.2
    /// rather than panic on integer overflow.
    #[test]
    fn test_from_bytes_does_not_panic_on_octal_escape_overflow_fuzz_finding() {
        let data: Vec<u8> = vec![
            37, 80, 68, 70, 45, 49, 46, 55, 32, 48, 32, 111, 98, 106, 10, 60, 60, 32, 47, 84, 80,
            54, 40, 92, 53, 53, 51, 32, 10,
        ];
        // Must not panic; whether it successfully opens or returns an
        // error is not the point of this regression test.
        let _ = PdfReader::from_bytes(data);
    }

    /// Minimal hand-rolled builder for constructing byte-exact PDF fixtures
    /// (multi-revision xref chains, hybrid-reference files, object
    /// streams, ...) that exercise structures the high-level
    /// `DocumentBuilder` doesn't produce.
    struct RawPdfBuilder {
        buf: Vec<u8>,
    }

    impl RawPdfBuilder {
        fn new() -> Self {
            let mut buf = Vec::new();
            buf.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
            Self { buf }
        }

        fn offset(&self) -> u64 {
            self.buf.len() as u64
        }

        /// Appends a simple (non-stream) indirect object and returns its
        /// byte offset.
        fn object(&mut self, num: u32, body: &str) -> u64 {
            let off = self.offset();
            self.buf
                .extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", num, body).as_bytes());
            off
        }

        /// Appends a stream object (with the given extra dictionary
        /// entries, e.g. `"/Type /ObjStm /N 2 /First 8"`) and returns its
        /// byte offset.
        fn stream_object(&mut self, num: u32, extra_dict: &str, data: &[u8]) -> u64 {
            let off = self.offset();
            self.buf.extend_from_slice(
                format!(
                    "{} 0 obj\n<< /Length {} {} >>\nstream\n",
                    num,
                    data.len(),
                    extra_dict
                )
                .as_bytes(),
            );
            self.buf.extend_from_slice(data);
            self.buf.extend_from_slice(b"\nendstream\nendobj\n");
            off
        }

        /// Appends a traditional `xref` table (a single `0 <size>`
        /// subsection covering object numbers `0..size`, using `Free` for
        /// any gaps) plus a `trailer` dictionary, `startxref`, and
        /// `%%EOF`. Returns the byte offset of the `xref` keyword (what
        /// `startxref` in a subsequent revision, or the file's own
        /// trailing `startxref`, should point at).
        fn xref_and_trailer(
            &mut self,
            entries: &[(u32, u64)],
            size: u32,
            trailer_extra: &str,
        ) -> u64 {
            let xref_off = self.offset();
            let mut table = std::collections::HashMap::new();
            for &(num, off) in entries {
                table.insert(num, off);
            }

            self.buf
                .extend_from_slice(format!("xref\n0 {}\n", size).as_bytes());
            for n in 0..size {
                if n == 0 {
                    self.buf.extend_from_slice(b"0000000000 65535 f \n");
                } else if let Some(&off) = table.get(&n) {
                    self.buf
                        .extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
                } else {
                    self.buf.extend_from_slice(b"0000000000 00000 f \n");
                }
            }

            self.buf.extend_from_slice(
                format!("trailer\n<< /Size {} {} >>\n", size, trailer_extra).as_bytes(),
            );
            self.buf
                .extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_off).as_bytes());

            xref_off
        }

        fn finish(self) -> Vec<u8> {
            self.buf
        }
    }

    #[test]
    fn test_incremental_update_prev_chain() {
        let mut b = RawPdfBuilder::new();

        // Revision 1: object 1 is a Catalog whose /Foo is "old".
        let obj1_off = b.object(1, "<< /Type /Catalog /Foo (old) >>");
        let rev1_xref = b.xref_and_trailer(&[(1, obj1_off)], 2, "/Root 1 0 R");

        // Revision 2 (incremental update): object 1 is redefined with
        // /Foo = "new"; the new xref section only lists the changed
        // object and chains back via /Prev to revision 1's xref.
        let obj1_new_off = b.object(1, "<< /Type /Catalog /Foo (new) >>");
        b.xref_and_trailer(
            &[(1, obj1_new_off)],
            2,
            &format!("/Root 1 0 R /Prev {}", rev1_xref),
        );

        let reader = PdfReader::from_bytes(b.finish()).unwrap();
        let catalog = reader.catalog().unwrap();
        match catalog.get("Foo") {
            Some(Object::String(s)) => assert_eq!(s.as_bytes(), b"new"),
            other => panic!("expected the most recent revision's value, got {:?}", other),
        }
    }

    #[test]
    fn test_xref_prev_cycle_does_not_hang() {
        let mut b = RawPdfBuilder::new();
        let obj1_off = b.object(1, "<< /Type /Catalog >>");

        // First write a placeholder xref/trailer so we know its offset,
        // then rewrite trailer_extra to point /Prev at itself once we
        // know that offset (a malicious/corrupt self-referential chain).
        let xref_off = b.offset();
        b.buf.extend_from_slice(b"xref\n0 2\n0000000000 65535 f \n");
        b.buf
            .extend_from_slice(format!("{:010} 00000 n \n", obj1_off).as_bytes());
        b.buf.extend_from_slice(
            format!("trailer\n<< /Size 2 /Root 1 0 R /Prev {} >>\n", xref_off).as_bytes(),
        );
        b.buf
            .extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_off).as_bytes());

        // Must terminate (not hang) and still recover a usable trailer.
        let result = PdfReader::from_bytes(b.finish());
        assert!(result.is_ok());
    }

    #[test]
    fn test_hybrid_reference_file_xrefstm() {
        let mut b = RawPdfBuilder::new();

        let obj1_off = b.object(1, "<< /Type /Catalog >>");
        let obj2_off = b.object(2, "<< /Type /Pages /Kids [] /Count 0 >>");

        // A minimal uncompressed xref stream (W = [1 4 1], one entry for
        // object 2) supplementing the traditional table below via
        // /XRefStm, per ISO 32000-1 7.5.8.4.
        let mut xref_stream_data = Vec::new();
        xref_stream_data.push(1u8); // type = in use
        xref_stream_data.extend_from_slice(&(obj2_off as u32).to_be_bytes());
        xref_stream_data.push(0); // generation
        let xrefstm_off = b.stream_object(
            3,
            "/Type /XRef /W [1 4 1] /Index [2 1] /Size 4",
            &xref_stream_data,
        );

        // Traditional table only lists object 1; object 2 is only
        // reachable via the hybrid /XRefStm.
        b.xref_and_trailer(
            &[(1, obj1_off)],
            4,
            &format!("/Root 1 0 R /XRefStm {}", xrefstm_off),
        );

        let reader = PdfReader::from_bytes(b.finish()).unwrap();
        assert!(reader.xref().get(2).is_some());
        assert_eq!(reader.page_count(), 0);
    }

    #[test]
    fn test_object_stream_resolution_via_xref_stream() {
        let mut b = RawPdfBuilder::new();

        let obj1_off = b.object(1, "<< /Type /Catalog /Pages 2 0 R >>");

        // Object stream (object 3) containing object 2 (an empty Pages
        // dict) at relative offset 0. Object 2 has *no* standalone
        // indirect definition anywhere else in the file - it is only
        // reachable via the object stream, exactly like real-world
        // producers that compress most objects into ObjStms.
        let objstm_body = b"<< /Type /Pages /Kids [] /Count 0 >>";
        let header = b"2 0"; // "obj_num offset" pairs, offset relative to /First
        let mut full = header.to_vec();
        full.push(b' ');
        full.extend_from_slice(objstm_body);
        let first = header.len() + 1;
        let objstm_off = b.stream_object(3, &format!("/Type /ObjStm /N 1 /First {}", first), &full);

        // Cross-reference stream (object 4, but it does not need to
        // describe itself: parse_xref_and_trailer locates it directly via
        // startxref, not through the table). W = [1 4 2].
        let (w1, w2, w3) = (1usize, 4usize, 2usize);
        let mut entries = Vec::new();
        let mut push_entry = |t: u8, f2: u32, f3: u16| {
            entries.push(t);
            entries.extend_from_slice(&f2.to_be_bytes());
            entries.extend_from_slice(&f3.to_be_bytes());
        };
        push_entry(0, 0, 65535); // object 0: free (required head entry)
        push_entry(1, obj1_off as u32, 0); // object 1: in use (Catalog)
        push_entry(2, 3, 0); // object 2: compressed in stream 3, index 0
        push_entry(1, objstm_off as u32, 0); // object 3: in use (the ObjStm)
        debug_assert_eq!(entries.len(), 4 * (w1 + w2 + w3));

        b.stream_object(
            4,
            &format!(
                "/Type /XRef /W [{} {} {}] /Index [0 4] /Size 4 /Root 1 0 R",
                w1, w2, w3
            ),
            &entries,
        );
        let xref_stream_off = {
            // The stream we just wrote starts right before its own
            // "4 0 obj" header; recompute that start offset.
            let marker = b"4 0 obj";
            let hay = &b.buf[..];
            hay.windows(marker.len())
                .rposition(|w| w == marker)
                .expect("xref stream object header must be present") as u64
        };
        b.buf
            .extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_stream_off).as_bytes());

        let reader = PdfReader::from_bytes(b.finish()).unwrap();
        assert_eq!(reader.page_count(), 0);
        let pages = reader.catalog().unwrap();
        match pages.get("Pages") {
            Some(Object::Reference(id)) => {
                let resolved = reader.resolve_reference(*id).unwrap();
                match resolved {
                    Object::Dictionary(d) => {
                        assert_eq!(
                            d.get("Type"),
                            Some(&Object::Name(PdfName::new_unchecked("Pages")))
                        );
                    }
                    _ => panic!("expected Pages dictionary resolved from object stream"),
                }
            }
            other => panic!("expected /Pages reference, got {:?}", other),
        }
    }

    #[test]
    fn test_recovery_mode_when_xref_is_corrupted() {
        let mut b = RawPdfBuilder::new();
        let obj1_off = b.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
        let obj2_off = b.object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
        let obj3_off = b.object(
            3,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>",
        );
        let real_xref_off = b.xref_and_trailer(
            &[(1, obj1_off), (2, obj2_off), (3, obj3_off)],
            4,
            "/Root 1 0 R",
        );

        let mut data = b.finish();
        // Corrupt the xref table in place: overwrite the "xref" keyword
        // with garbage so it can neither be parsed as a traditional table
        // nor as an xref stream/indirect object.
        let start = real_xref_off as usize;
        for (i, byte) in data[start..start + 4].iter_mut().enumerate() {
            *byte = b'X' + i as u8;
        }

        // A naive parser would now fail outright; recovery mode must
        // reconstruct the structure by scanning for `obj`/`endobj`.
        let reader = PdfReader::from_bytes(data).expect("recovery mode should succeed");
        assert_eq!(reader.page_count(), 1);
        assert!(reader.catalog().is_some());
    }

    #[test]
    fn test_recovery_mode_explicit_entry_point() {
        let mut b = RawPdfBuilder::new();
        let obj1_off = b.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
        let obj2_off = b.object(2, "<< /Type /Pages /Kids [] /Count 0 >>");
        b.xref_and_trailer(&[(1, obj1_off), (2, obj2_off)], 3, "/Root 1 0 R");

        let reader = PdfReader::from_bytes_recovery_only(b.finish()).unwrap();
        assert!(reader.catalog().is_some());
        assert_eq!(reader.page_count(), 0);
    }

    #[test]
    fn test_catalog_access() {
        let pdf_bytes = create_simple_pdf();
        let reader = PdfReader::from_bytes(pdf_bytes).unwrap();

        let catalog = reader.catalog().unwrap();
        assert!(catalog.get("Type").is_some());
        assert!(catalog.get("Pages").is_some());
    }

    #[test]
    fn test_info_access() {
        let pdf_bytes = create_simple_pdf();
        let reader = PdfReader::from_bytes(pdf_bytes).unwrap();

        let info = reader.info().unwrap();
        assert!(info.get("Title").is_some());
    }

    #[test]
    fn test_roundtrip_multi_page() {
        use crate::prelude::*;

        let page1 = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Page 1"))
            .build();

        let page2 = PageBuilder::a4()
            .font("F1", Standard14Font::Helvetica)
            .content(ContentBuilder::new().text("F1", 12.0, 72.0, 750.0, "Page 2"))
            .build();

        let doc = DocumentBuilder::new()
            .page(page1)
            .page(page2)
            .build()
            .unwrap();

        let pdf_bytes = doc.save_to_bytes().unwrap();
        let reader = PdfReader::from_bytes(pdf_bytes).unwrap();

        assert_eq!(reader.page_count(), 2);
    }

    // ---------------------------------------------------------------
    // Large-file streaming: memory-mapped I/O, object caching, and
    // random-access page lookup (see the module docs).
    // ---------------------------------------------------------------

    /// [`PdfReader::from_file`] must open successfully via the
    /// memory-mapped path (not just `from_bytes`) and behave identically
    /// to a buffer-backed reader for ordinary structural access.
    #[test]
    fn test_from_file_uses_memory_mapped_backing() {
        let pdf_bytes = create_simple_pdf();
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), &pdf_bytes).unwrap();

        let reader = PdfReader::from_file(file.path()).unwrap();
        assert!(matches!(reader.data, DataSource::Mapped(_)));
        assert_eq!(reader.page_count(), 1);
        assert!(reader.catalog().is_some());
    }

    /// `PdfReader::from_file` on a nonexistent path must return an error,
    /// not panic (the underlying `File::open`/`Mmap::map` failure must
    /// propagate through `?`, not `.unwrap()`).
    #[test]
    fn test_from_file_missing_path_errors_not_panics() {
        let result = PdfReader::from_file("/no/such/path/does-not-exist.pdf");
        assert!(result.is_err());
    }

    /// A freshly opened reader has resolved nothing yet, but the *first*
    /// [`PdfReader::resolve_reference`] call for a given id populates the
    /// cache so a subsequent [`PdfReader::get_object`] (cache-only) call
    /// finds it -- this is what makes repeated access to the same object
    /// (e.g. a page visited twice) cheap.
    #[test]
    fn test_resolve_reference_populates_get_object_cache() {
        let reader = PdfReader::from_bytes(create_simple_pdf()).unwrap();
        let root = reader.trailer().root;

        assert!(reader.get_object(root).is_none(), "nothing resolved yet");
        let resolved = reader.resolve_reference(root).unwrap();
        assert_eq!(reader.get_object(root), Some(resolved));
    }

    /// Builds a document whose page tree is a single, flat `/Kids` array
    /// (every kid is directly a `/Page` leaf) with `count` pages, each
    /// carrying a distinct `/Idx` marker so a test can confirm exactly
    /// which page object a lookup returned without needing to decode any
    /// content stream.
    fn flat_page_tree_pdf(count: u32) -> Vec<u8> {
        let mut b = RawPdfBuilder::new();
        let pages_obj_num = 2u32;
        let mut page_offsets = Vec::new();
        for i in 0..count {
            let off = b.object(
                3 + i,
                &format!(
                    "<< /Type /Page /Parent {pages_obj_num} 0 R /Idx {i} \
                     /MediaBox [0 0 612 792] /Resources << >> >>"
                ),
            );
            page_offsets.push(off);
        }
        let catalog_off = b.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
        let kids = (0..count)
            .map(|i| format!("{} 0 R", 3 + i))
            .collect::<Vec<_>>()
            .join(" ");
        let pages_off = b.object(
            pages_obj_num,
            &format!("<< /Type /Pages /Kids [{kids}] /Count {count} >>"),
        );

        let mut entries = vec![(1, catalog_off), (pages_obj_num, pages_off)];
        for (i, off) in page_offsets.iter().enumerate() {
            entries.push((3 + i as u32, *off));
        }
        b.xref_and_trailer(&entries, 3 + count, "/Root 1 0 R");
        b.finish()
    }

    #[test]
    fn test_get_page_flat_tree_direct_index() {
        let reader = PdfReader::from_bytes(flat_page_tree_pdf(2_000)).unwrap();
        assert_eq!(reader.page_count(), 2_000);

        for &i in &[0usize, 1, 999, 1_999] {
            let page = reader.get_page(i).unwrap_or_else(|| panic!("page {i}"));
            assert_eq!(page.get("Idx"), Some(&Object::Integer(i as i64)));
        }
        assert!(reader.get_page(2_000).is_none());
    }

    /// The flat-tree fast path in `page_ref_at` must never resolve any
    /// page dictionary other than the ones on the direct path to the
    /// target (here: the catalog, the `/Pages` root, and the target leaf
    /// itself) -- i.e. looking up the last of 2,000 pages must not touch
    /// the other 1,999. This is the concrete "no O(N) parse" guarantee
    /// behind the random-access DoD requirement.
    #[test]
    fn test_get_page_flat_tree_does_not_resolve_other_pages() {
        let reader = PdfReader::from_bytes(flat_page_tree_pdf(5_000)).unwrap();
        let page = reader.get_page(4_999).unwrap();
        assert_eq!(page.get("Idx"), Some(&Object::Integer(4_999)));

        let cached = reader.object_cache.read().unwrap().len();
        // Catalog + /Pages root + the one target leaf page.
        assert!(
            cached <= 3,
            "expected only the path-to-target objects cached, got {cached}"
        );
    }

    /// Builds a two-level balanced tree: a root `/Pages` node with two
    /// intermediate `/Pages` kids, each owning `per_branch` leaf pages.
    /// Root `/Count` != root `/Kids.len()` here (2 kids, but
    /// `2 * per_branch` total leaves), so this exercises the general
    /// (`/Count`-guided sibling-skipping) descent path rather than the
    /// flat-tree fast path.
    fn nested_page_tree_pdf(per_branch: u32) -> Vec<u8> {
        let mut b = RawPdfBuilder::new();
        let total = per_branch * 2;

        // Leaf pages for branch A (indices 0..per_branch) then branch B
        // (indices per_branch..2*per_branch). Object numbers: leaves start
        // at 4 (1=Catalog, 2=root Pages, 3.. branch Pages nodes).
        let mut leaf_offsets = Vec::new();
        for i in 0..total {
            let off = b.object(
                4 + i,
                &format!("<< /Type /Page /Idx {i} /MediaBox [0 0 612 792] /Resources << >> >>"),
            );
            leaf_offsets.push(off);
        }

        let branch_a_kids: String = (0..per_branch)
            .map(|i| format!("{} 0 R", 4 + i))
            .collect::<Vec<_>>()
            .join(" ");
        let branch_b_kids: String = (per_branch..total)
            .map(|i| format!("{} 0 R", 4 + i))
            .collect::<Vec<_>>()
            .join(" ");

        let branch_a_num = 4 + total;
        let branch_b_num = branch_a_num + 1;
        let branch_a_off = b.object(
            branch_a_num,
            &format!(
                "<< /Type /Pages /Parent 2 0 R /Kids [{branch_a_kids}] /Count {per_branch} >>"
            ),
        );
        let branch_b_off = b.object(
            branch_b_num,
            &format!(
                "<< /Type /Pages /Parent 2 0 R /Kids [{branch_b_kids}] /Count {per_branch} >>"
            ),
        );

        let catalog_off = b.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
        let root_off = b.object(
            2,
            &format!(
                "<< /Type /Pages /Kids [{branch_a_num} 0 R {branch_b_num} 0 R] /Count {total} >>"
            ),
        );

        let mut entries = vec![
            (1, catalog_off),
            (2, root_off),
            (branch_a_num, branch_a_off),
            (branch_b_num, branch_b_off),
        ];
        for (i, off) in leaf_offsets.iter().enumerate() {
            entries.push((4 + i as u32, *off));
        }
        b.xref_and_trailer(&entries, branch_b_num + 1, "/Root 1 0 R");
        b.finish()
    }

    #[test]
    fn test_get_page_nested_tree_count_guided_descent() {
        let reader = PdfReader::from_bytes(nested_page_tree_pdf(50)).unwrap();
        assert_eq!(reader.page_count(), 100);

        for &i in &[0usize, 1, 49, 50, 51, 99] {
            let page = reader.get_page(i).unwrap_or_else(|| panic!("page {i}"));
            assert_eq!(page.get("Idx"), Some(&Object::Integer(i as i64)));
        }
        assert!(reader.get_page(100).is_none());
    }

    /// Same shape as [`test_get_page_nested_tree_count_guided_descent`],
    /// but branch A's `/Pages` node itself omits `/Count` (non-conformant).
    /// This specifically exercises `subtree_leaf_count`'s recursive
    /// fallback (an intermediate node *with* `/Kids` but no valid
    /// `/Count`), not just the "no `/Kids` => 1 leaf" base case.
    #[test]
    fn test_get_page_nested_tree_missing_count_on_intermediate_node_falls_back() {
        let mut b = RawPdfBuilder::new();

        let leaf_a0 = b.object(10, "<< /Type /Page /Idx 0 >>");
        let leaf_a1 = b.object(11, "<< /Type /Page /Idx 1 >>");
        let leaf_b0 = b.object(12, "<< /Type /Page /Idx 2 >>");

        // Branch A has /Kids but *no* /Count.
        let branch_a_off = b.object(5, "<< /Type /Pages /Kids [10 0 R 11 0 R] >>");
        let branch_b_off = b.object(6, "<< /Type /Pages /Kids [12 0 R] /Count 1 >>");

        let catalog_off = b.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
        let root_off = b.object(2, "<< /Type /Pages /Kids [5 0 R 6 0 R] /Count 3 >>");

        b.xref_and_trailer(
            &[
                (1, catalog_off),
                (2, root_off),
                (5, branch_a_off),
                (6, branch_b_off),
                (10, leaf_a0),
                (11, leaf_a1),
                (12, leaf_b0),
            ],
            13,
            "/Root 1 0 R",
        );

        let reader = PdfReader::from_bytes(b.finish()).unwrap();
        assert_eq!(
            reader.get_page(0).unwrap().get("Idx"),
            Some(&Object::Integer(0))
        );
        assert_eq!(
            reader.get_page(1).unwrap().get("Idx"),
            Some(&Object::Integer(1))
        );
        assert_eq!(
            reader.get_page(2).unwrap().get("Idx"),
            Some(&Object::Integer(2))
        );
        assert!(reader.get_page(3).is_none());
    }

    /// A `/Pages` node with `/Kids` but no (or an invalid) `/Count` is
    /// not spec-conformant (`/Count` is required, ISO 32000-1 Table 29),
    /// but `page_ref`/`get_page` must still find the right leaf via the
    /// bounded fallback in `subtree_leaf_count` rather than simply
    /// failing the whole lookup.
    #[test]
    fn test_get_page_falls_back_when_count_missing() {
        let mut b = RawPdfBuilder::new();
        let leaf0_off = b.object(3, "<< /Type /Page /Idx 0 >>");
        let leaf1_off = b.object(4, "<< /Type /Page /Idx 1 >>");
        let catalog_off = b.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
        // No /Count at all on the Pages node.
        let pages_off = b.object(2, "<< /Type /Pages /Kids [3 0 R 4 0 R] >>");
        b.xref_and_trailer(
            &[
                (1, catalog_off),
                (2, pages_off),
                (3, leaf0_off),
                (4, leaf1_off),
            ],
            5,
            "/Root 1 0 R",
        );

        let reader = PdfReader::from_bytes(b.finish()).unwrap();
        assert_eq!(
            reader.get_page(0).unwrap().get("Idx"),
            Some(&Object::Integer(0))
        );
        assert_eq!(
            reader.get_page(1).unwrap().get("Idx"),
            Some(&Object::Integer(1))
        );
        assert!(reader.get_page(2).is_none());
    }

    /// A `/Pages` node whose `/Kids` lists itself must not hang
    /// `page_ref`/`get_page` (mirrors
    /// `crate::editor::graph`'s equivalent cyclic-tree regression test).
    /// No `/Count` is set, so this also exercises the recursive fallback
    /// path's own cycle/budget bound, not just the depth cap on the
    /// direct-descent path.
    #[test]
    fn test_get_page_cyclic_kids_does_not_hang() {
        let mut b = RawPdfBuilder::new();
        let catalog_off = b.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
        let pages_off = b.object(2, "<< /Type /Pages /Kids [2 0 R] >>");
        b.xref_and_trailer(&[(1, catalog_off), (2, pages_off)], 3, "/Root 1 0 R");

        let reader = PdfReader::from_bytes(b.finish()).unwrap();
        // Must terminate; a self-referential Pages node has no leaves.
        assert!(reader.get_page(0).is_none());
    }

    /// Resolving two different compressed objects out of the same
    /// `/ObjStm` must decode/parse that stream only once (see
    /// `decoded_object_stream`), not once per compressed object.
    #[test]
    fn test_compressed_object_stream_decoded_once_for_multiple_objects() {
        let mut b = RawPdfBuilder::new();
        let obj1_off = b.object(1, "<< /Type /Catalog /Pages 2 0 R >>");

        let objstm_body = b"<< /Type /Pages /Kids [] /Count 0 >> << /Foo (bar) >>";
        let header = b"2 0 5 37"; // "obj_num offset" pairs relative to /First
        let mut full = header.to_vec();
        full.push(b' ');
        full.extend_from_slice(objstm_body);
        let first = header.len() + 1;
        let objstm_off = b.stream_object(3, &format!("/Type /ObjStm /N 2 /First {}", first), &full);

        let (w1, w2, w3) = (1usize, 4usize, 2usize);
        let mut entries = Vec::new();
        let mut push_entry = |t: u8, f2: u32, f3: u16| {
            entries.push(t);
            entries.extend_from_slice(&f2.to_be_bytes());
            entries.extend_from_slice(&f3.to_be_bytes());
        };
        push_entry(0, 0, 65535); // object 0: free (required head entry)
        push_entry(1, obj1_off as u32, 0); // object 1: Catalog
        push_entry(2, 3, 0); // object 2: compressed in stream 3, index 0
        push_entry(1, objstm_off as u32, 0); // object 3: the ObjStm itself
        push_entry(0, 0, 0); // object 4: unused/free filler
        push_entry(2, 3, 1); // object 5: compressed in stream 3, index 1

        b.stream_object(
            6,
            &format!(
                "/Type /XRef /W [{} {} {}] /Index [0 6] /Size 6 /Root 1 0 R",
                w1, w2, w3
            ),
            &entries,
        );
        let xref_stream_off = {
            let marker = b"6 0 obj";
            b.buf
                .windows(marker.len())
                .rposition(|w| w == marker)
                .expect("xref stream object header must be present") as u64
        };
        b.buf
            .extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_stream_off).as_bytes());

        let reader = PdfReader::from_bytes(b.finish()).unwrap();
        assert!(reader.resolve_reference(ObjectId::new(2)).is_some());
        assert!(reader.resolve_reference(ObjectId::new(5)).is_some());
        assert_eq!(
            reader.object_stream_cache.read().unwrap().len(),
            1,
            "both compressed objects came from the same ObjStm, which should be decoded once"
        );
    }

    /// Simple concurrent-access stress test (per this crate's DoD: a
    /// straightforward multi-thread stress run is acceptable in lieu of
    /// loom/miri for this module). Many threads share one `Arc<PdfReader>`
    /// and repeatedly resolve pages/objects at once; this must complete
    /// without panicking, deadlocking, or any thread observing a wrong
    /// result -- `RwLock`-guarded caches are the only shared mutable
    /// state, so this is exercising that they hand out correct,
    /// consistent values under real contention.
    #[test]
    fn test_concurrent_page_access_from_multiple_threads_does_not_race_or_deadlock() {
        use std::sync::Arc;
        use std::thread;

        let reader = Arc::new(PdfReader::from_bytes(flat_page_tree_pdf(200)).unwrap());
        let mut handles = Vec::new();

        for t in 0..8 {
            let reader = Arc::clone(&reader);
            handles.push(thread::spawn(move || {
                for iter in 0..500 {
                    let index = (t * 37 + iter) % 200;
                    let page = reader
                        .get_page(index)
                        .unwrap_or_else(|| panic!("thread {t} iter {iter}: page {index}"));
                    assert_eq!(
                        page.get("Idx"),
                        Some(&Object::Integer(index as i64)),
                        "thread {t} iter {iter}: wrong page returned for index {index}"
                    );
                }
            }));
        }

        for h in handles {
            h.join().expect("worker thread panicked");
        }
    }
}
