//! Redaction audit trail: **who** redacted **what area of what page**,
//! and **when** (ISO 32000-1:2008 does not define a redaction-audit
//! object type, so this is a documented, namespaced private extension -
//! see [`AUDIT_LOG_CATALOG_KEY`]).
//!
//! Deliberately does **not** retain the redacted content itself (the
//! matched text, the image bytes, ...) - logging the removed content
//! back into the file would defeat the point of redacting it. An entry
//! only ever records the actor, a caller-supplied reason, a timestamp,
//! which page was touched, the rectangular area (in the page's default
//! user space, ISO 32000-1 8.3.2.2 - the same convention
//! [`crate::editor::EditableDocument`]'s annotation helpers use) and
//! *counts* of what was removed.
//!
//! The log is persisted as a private stream object referenced from the
//! document catalog under [`AUDIT_LOG_CATALOG_KEY`] so it round-trips
//! through both [`crate::editor::EditableDocument::save_incremental`] and
//! [`crate::editor::EditableDocument::save_full_rewrite`] and can be read
//! back (`EditableDocument::audit_log`) after reopening the saved file -
//! this is what makes the log "readable again", not just an in-memory
//! `Vec` that disappears when the process exits. Adding a private,
//! non-standard key to the catalog dictionary is permitted by ISO
//! 32000-1 7.3.7 (dictionaries may contain any entries; conforming
//! readers must ignore keys they don't recognize) and is the same
//! technique already used by [`crate::editor::structure`] for
//! `/StructTreeRoot` bookkeeping.
//!
//! # On-disk format
//!
//! The stream body is a small, hand-rolled, length-prefixed binary
//! format (deliberately not JSON/serde: this crate has no JSON
//! dependency, and every field here is already a fixed-width number or a
//! length-prefixed byte string, so a bespoke format is both simpler and
//! avoids pulling in a new dependency for one small internal use). All
//! multi-byte integers are big-endian. Layout:
//!
//! ```text
//! magic:        8 bytes   b"RPDFAUD1"
//! entry_count:  u32
//! entry* (entry_count times):
//!   page_index:              u32  (0xFFFF_FFFF means "no specific page")
//!   has_area:                u8   (0 or 1)
//!   area (if has_area == 1): f64 llx, f64 lly, f64 urx, f64 ury
//!   text_runs_removed:       u32
//!   images_removed:          u32
//!   tounicode_entries_pruned:u32
//!   actor_len:     u32; actor bytes (UTF-8)
//!   reason_len:    u32; reason bytes (UTF-8)
//!   timestamp_len: u32; timestamp bytes (UTF-8, PDF date format ISO
//!                  32000-1 7.9.4 by convention, but not enforced on read)
//! ```
//!
//! Reading this back is **untrusted-input parsing** (a hostile or
//! corrupted file could claim an enormous `entry_count` or field length):
//! [`parse_log`] bounds total input size ([`MAX_AUDIT_LOG_BYTES`]), the
//! number of entries ([`MAX_AUDIT_ENTRIES`]), and every individual
//! length-prefixed field ([`MAX_FIELD_BYTES`]), and never panics on
//! truncated/malformed input - it stops and returns whatever entries were
//! successfully parsed so far rather than erroring out or over-reading.

use crate::object::{Object, PdfDictionary, PdfName, PdfStream};
use crate::types::{ObjectId, Rectangle};

/// Catalog key the audit log stream is referenced under (ISO 32000-1
/// 7.3.7 permits application-private dictionary entries).
pub(crate) const AUDIT_LOG_CATALOG_KEY: &str = "RustPdfRedactionLog";

const MAGIC: &[u8; 8] = b"RPDFAUD1";

/// Hard cap on the decoded audit-log stream size accepted when reopening
/// a document. Guards against a crafted file forcing unbounded
/// allocation while parsing (untrusted-input rule).
const MAX_AUDIT_LOG_BYTES: usize = 8 * 1024 * 1024;

/// Hard cap on the number of entries parsed from a single log stream,
/// independent of byte size.
const MAX_AUDIT_ENTRIES: usize = 100_000;

/// Hard cap on any single length-prefixed string field within an entry.
const MAX_FIELD_BYTES: u32 = 64 * 1024;

/// One recorded redaction action. See the [module docs](self) for why
/// this intentionally never carries the redacted content itself.
#[derive(Debug, Clone, PartialEq)]
pub struct RedactionAuditEntry {
    /// Who performed the redaction (caller-supplied; this crate has no
    /// notion of an authenticated identity, so it trusts the caller's
    /// application to pass a meaningful value, e.g. a logged-in user's
    /// email or account id).
    pub actor: String,
    /// Why the redaction was performed (caller-supplied free text, e.g.
    /// "PII removal - SSN" or "privileged/attorney-client material").
    pub reason: String,
    /// When the redaction was performed, in PDF date format (ISO 32000-1
    /// 7.9.4, `D:YYYYMMDDHHmmSSZ`) unless the caller supplied a
    /// different string explicitly.
    pub timestamp: String,
    /// Which page (0-based) was affected, or `None` for a
    /// whole-document action (e.g. [`crate::editor::EditableDocument::strip_document_metadata`]).
    pub page_index: Option<usize>,
    /// The redacted rectangle, in the page's default user space, or
    /// `None` for actions that are not area-scoped (e.g. literal-text or
    /// whole-document redaction).
    pub area: Option<Rectangle>,
    /// Number of content-stream text-showing operators removed.
    pub text_runs_removed: usize,
    /// Number of image XObjects (and inline images) removed.
    pub images_removed: usize,
    /// Number of now-orphaned `/ToUnicode` CMap entries pruned as a
    /// result of this action (ISO 32000-1 9.10.3).
    pub tounicode_entries_pruned: usize,
}

/// Serializes `entries` to the on-disk format described in the
/// [module docs](self).
pub(crate) fn serialize_log(entries: &[RedactionAuditEntry]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + entries.len() * 64);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for e in entries {
        out.extend_from_slice(&e.page_index.map(|p| p as u32).unwrap_or(u32::MAX).to_be_bytes());
        match e.area {
            Some(r) => {
                out.push(1);
                for v in [r.llx, r.lly, r.urx, r.ury] {
                    out.extend_from_slice(&v.to_be_bytes());
                }
            }
            None => out.push(0),
        }
        out.extend_from_slice(&(e.text_runs_removed as u32).to_be_bytes());
        out.extend_from_slice(&(e.images_removed as u32).to_be_bytes());
        out.extend_from_slice(&(e.tounicode_entries_pruned as u32).to_be_bytes());
        write_field(&mut out, e.actor.as_bytes());
        write_field(&mut out, e.reason.as_bytes());
        write_field(&mut out, e.timestamp.as_bytes());
    }
    out
}

fn write_field(out: &mut Vec<u8>, bytes: &[u8]) {
    let len = (bytes.len() as u32).min(MAX_FIELD_BYTES);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&bytes[..len as usize]);
}

/// Parses the on-disk format described in the [module docs](self).
///
/// Best-effort over untrusted input: stops and returns whatever entries
/// parsed successfully so far as soon as the magic/header is missing, a
/// declared length would run past the end of `data`, or any of the
/// bounds above is exceeded - it never panics or over-reads.
pub(crate) fn parse_log(data: &[u8]) -> Vec<RedactionAuditEntry> {
    let mut out = Vec::new();
    if data.len() > MAX_AUDIT_LOG_BYTES {
        return out;
    }
    let mut r = Reader { data, pos: 0 };
    let Some(magic) = r.take(MAGIC.len()) else { return out };
    if magic != MAGIC {
        return out;
    }
    let Some(count) = r.take_u32() else { return out };
    let count = (count as usize).min(MAX_AUDIT_ENTRIES);
    for _ in 0..count {
        let Some(entry) = parse_entry(&mut r) else { break };
        out.push(entry);
    }
    out
}

fn parse_entry(r: &mut Reader) -> Option<RedactionAuditEntry> {
    let page_raw = r.take_u32()?;
    let page_index = if page_raw == u32::MAX { None } else { Some(page_raw as usize) };
    let has_area = r.take_u8()?;
    let area = match has_area {
        1 => {
            let llx = r.take_f64()?;
            let lly = r.take_f64()?;
            let urx = r.take_f64()?;
            let ury = r.take_f64()?;
            Some(Rectangle::new(llx, lly, urx, ury))
        }
        0 => None,
        _ => return None,
    };
    let text_runs_removed = r.take_u32()? as usize;
    let images_removed = r.take_u32()? as usize;
    let tounicode_entries_pruned = r.take_u32()? as usize;
    let actor = r.take_field_string()?;
    let reason = r.take_field_string()?;
    let timestamp = r.take_field_string()?;
    Some(RedactionAuditEntry {
        actor,
        reason,
        timestamp,
        page_index,
        area,
        text_runs_removed,
        images_removed,
        tounicode_entries_pruned,
    })
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn take_u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn take_u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_be_bytes(b.try_into().unwrap_or_default()))
    }

    fn take_f64(&mut self) -> Option<f64> {
        self.take(8).map(|b| f64::from_be_bytes(b.try_into().unwrap_or_default()))
    }

    fn take_field_string(&mut self) -> Option<String> {
        let len = self.take_u32()?;
        if len > MAX_FIELD_BYTES {
            return None;
        }
        let bytes = self.take(len as usize)?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Reads the audit log currently stored (if any) in `catalog`'s
/// [`AUDIT_LOG_CATALOG_KEY`] entry.
pub(crate) fn load_from_catalog(
    catalog: &PdfDictionary,
    resolve_stream: impl Fn(ObjectId) -> Option<Vec<u8>>,
) -> Vec<RedactionAuditEntry> {
    match catalog.get(AUDIT_LOG_CATALOG_KEY) {
        Some(Object::Reference(id)) => match resolve_stream(*id) {
            Some(decoded) => parse_log(&decoded),
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Builds the (uncompressed - compression is applied by the caller, which
/// already has the `compression` feature available) stream object to
/// persist `entries` as the document's audit log.
pub(crate) fn build_log_stream(entries: &[RedactionAuditEntry]) -> PdfStream {
    let mut dict = PdfDictionary::new();
    dict.set("Type", Object::Name(PdfName::new_unchecked("RustPdfRedactionLog")));
    PdfStream::with_dictionary(dict, serialize_log(entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> RedactionAuditEntry {
        RedactionAuditEntry {
            actor: "alice@example.com".to_string(),
            reason: "PII removal".to_string(),
            timestamp: "D:20260702120000Z".to_string(),
            page_index: Some(2),
            area: Some(Rectangle::new(10.0, 20.0, 110.0, 60.0)),
            text_runs_removed: 3,
            images_removed: 1,
            tounicode_entries_pruned: 5,
        }
    }

    #[test]
    fn round_trips_a_single_entry() {
        let entries = vec![sample_entry()];
        let bytes = serialize_log(&entries);
        let parsed = parse_log(&bytes);
        assert_eq!(parsed, entries);
    }

    #[test]
    fn round_trips_multiple_entries_and_whole_document_scope() {
        let mut e2 = sample_entry();
        e2.page_index = None;
        e2.area = None;
        e2.actor = "bob".to_string();
        let entries = vec![sample_entry(), e2];
        let bytes = serialize_log(&entries);
        let parsed = parse_log(&bytes);
        assert_eq!(parsed, entries);
    }

    #[test]
    fn empty_log_round_trips() {
        let bytes = serialize_log(&[]);
        assert!(parse_log(&bytes).is_empty());
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(parse_log(b"NOTALOG!\x00\x00\x00\x00").is_empty());
    }

    #[test]
    fn truncated_input_does_not_panic_and_returns_partial_results() {
        let entries = vec![sample_entry(), sample_entry()];
        let bytes = serialize_log(&entries);
        // Cut the buffer off partway through the second entry: must not
        // panic, and must still recover the first, complete entry.
        let truncated = &bytes[..bytes.len() - 5];
        let parsed = parse_log(truncated);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], entries[0]);
    }

    #[test]
    fn claimed_entry_count_far_beyond_actual_data_does_not_panic_or_hang() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&u32::MAX.to_be_bytes());
        // No entry data follows at all.
        let parsed = parse_log(&bytes);
        assert!(parsed.is_empty());
    }

    #[test]
    fn oversized_input_is_rejected_outright() {
        let huge = vec![0u8; MAX_AUDIT_LOG_BYTES + 1];
        assert!(parse_log(&huge).is_empty());
    }
}
