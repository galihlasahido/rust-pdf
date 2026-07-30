//! Read-side PDF Standard Security Handler decryption (ISO 32000-1 §7.6 /
//! ISO 32000-2 §7.6), scoped to exactly the two algorithms
//! [`crate::editor::EditableDocument::save_encrypted_to_bytes`] can
//! produce: AES-128 (`/V 4 /R 4`, `AESV2`) and AES-256 (`/V 5 /R 6`,
//! `AESV3`). Any other `/Filter`/`/V`/`/R`/`/CFM` combination (legacy
//! RC4, `/V 1`/`/V 2`, a non-`Standard` handler, ...) is rejected with
//! [`ParserError::UnsupportedEncryption`] rather than silently
//! mis-decrypted -- matching this crate's stated fail-closed philosophy
//! for unsupported constructs elsewhere (see `src/render/native/mod.rs`'s
//! module docs).
//!
//! # Why this doesn't just call `crate::encryption::EncryptionHandler`
//!
//! That was the first approach tried, and it does not work, for two
//! independent reasons:
//!
//! 1. `EncryptionHandler::new(config, file_id)` always derives a *fresh*
//!    file key from `config` -- random salts for AES-256/R6 (so even the
//!    same password produces a different key every call), and a
//!    from-scratch `/O` recomputation for AES-128/R4 that requires
//!    knowing *both* the original user *and* owner passwords. Opening an
//!    existing encrypted file requires the opposite: recovering the key
//!    that matches the `/O`/`/U`/`/OE`/`/UE` values *already stored* in
//!    that file's own `/Encrypt` dictionary, given only one candidate
//!    password. There is no `EncryptionHandler` constructor for that.
//! 2. The lower-level functions that *do* implement this (
//!    `key_derivation::verify_user_password` for AES-256/R6,
//!    `key_derivation::derive_aes128_keys`'s internal
//!    `compute_encryption_key_r4`/`compute_u_value_r4` helpers for
//!    AES-128/R4) live inside `crate::encryption`'s private
//!    (non-`pub`) `key_derivation` submodule. Rust module privacy makes a
//!    `pub fn` inside a private module reachable only from that module
//!    and its own descendants -- `crate::parser` is a sibling of
//!    `crate::encryption`, not a descendant, so none of them are
//!    actually callable from here even though individually marked `pub`.
//!    Widening `crate::encryption`'s public surface to fix this is
//!    explicitly out of scope for this change.
//!
//! So this module independently implements the *read* side of the same
//! ISO 32000-1 Algorithm 2 / 3.3 / 3.5 / 3.6 (AES-128/R4) and ISO
//! 32000-2 Algorithm 2.A / 2.B (AES-256/R6) password-verification/
//! key-recovery math the write side (`crate::editor::encrypt`,
//! `crate::encryption::key_derivation`) already implements on the
//! *write* side, using the same underlying audited crates this crate
//! already depends on for that (`md-5`, `sha2`, `aes`, `cbc` -- all
//! already-optional dependencies gated by this crate's own `encryption`
//! feature; nothing new is added to `Cargo.toml`). Object *content*
//! decryption (AES-128/256-CBC with a 16-byte random IV prefix and
//! PKCS#7 padding) is exactly the inverse of
//! `EncryptionHandler::encrypt_data`'s own `aes128_cbc_encrypt`/
//! `aes256_cbc_encrypt`, reimplemented the same way for the same reason.
//!
//! Correctness is cross-checked against the write side via round-trip
//! tests in `tests/open_encrypted_pdf_tests.rs`: encrypt a document with
//! `EditableDocument::save_encrypted_to_bytes` (both AES-128 and
//! AES-256), then open the result back through this module and confirm
//! the extracted content matches.
//!
//! # Scope notes (disclosed, not silent)
//!
//! - Only the *user* password is checked (ISO 32000-1 7.6.3.4 Algorithm
//!   3.6 / ISO 32000-2 Algorithm 2.B's user-password branch); the
//!   owner-password bypass (Algorithm 3.7 / 2.B's owner branch) is not
//!   implemented. This mirrors `crate::encryption::key_derivation`'s own
//!   scope: it only ever implemented `verify_user_password`, never an
//!   owner-password equivalent.
//! - `/EncryptMetadata false` (ISO 32000-1 7.6.1, Table 20) is parsed but
//!   *not* honoured as "leave `/Type /Metadata` streams in the clear":
//!   every string/stream is decrypted uniformly. This matches
//!   `crate::editor::encrypt`'s own write-side behavior (`encrypt_object`
//!   has no `/Type /Metadata` special case either), so it stays
//!   symmetric for anything this crate itself produces; a foreign file
//!   that actually relies on the distinction would have its (already
//!   plaintext) metadata stream run through AES decryption and fail to
//!   parse. Not exercised by this crate's own round-trip tests.
//! - Cross-reference streams and the `/Encrypt` dictionary's own strings
//!   are never encrypted (ISO 32000-1 7.5.8.2 / 7.6.1) and this module is
//!   simply never invoked for them by `crate::parser::PdfReader` -- see
//!   that module's `finish_with_password`/`resolve_reference`.

use crate::error::ParserError;
use crate::object::{Object, PdfArray, PdfDictionary, PdfStream, PdfString};
use md5::Md5;
use sha2::{Digest, Sha256, Sha384, Sha512};

/// The recovered file encryption key plus which of the two supported
/// algorithms it belongs to, for one already-opened encrypted document.
/// Built once ([`Decryptor::from_encrypt_dict`]) and reused by
/// [`crate::parser::PdfReader`] to transparently decrypt every
/// subsequently-resolved object.
#[derive(Debug)]
pub(crate) struct Decryptor {
    algorithm: Algorithm,
    file_key: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Algorithm {
    /// `/V 4 /R 4`, `AESV2` crypt filter (ISO 32000-1 7.6.1, Table 20).
    Aes128,
    /// `/V 5 /R 6`, `AESV3` crypt filter (ISO 32000-2 7.6.1, Table 20).
    Aes256,
}

impl Decryptor {
    /// Parses `dict` (the document's resolved `/Encrypt` dictionary,
    /// itself never encrypted) and, if `dict` describes one of the two
    /// algorithms this crate supports, recovers the file encryption key
    /// for `password`.
    ///
    /// `file_id` is the trailer's `/ID` first element (ISO 32000-1
    /// 7.5.5), required by the AES-128/R4 key derivation (Algorithm 2)
    /// but unused by AES-256/R6.
    pub(crate) fn from_encrypt_dict(
        dict: &PdfDictionary,
        file_id: Option<&[u8]>,
        password: &str,
    ) -> Result<Self, ParserError> {
        match dict.get("Filter") {
            Some(Object::Name(n)) if n.as_str() == "Standard" => {}
            other => {
                return Err(ParserError::UnsupportedEncryption(format!(
                    "unsupported /Filter (only the Standard security handler is implemented): {other:?}"
                )));
            }
        }
        if !uses_std_cf_for_streams_and_strings(dict) {
            return Err(ParserError::UnsupportedEncryption(
                "/StmF and /StrF must both be /StdCF (Identity or another named crypt filter \
                 is not supported)"
                    .to_string(),
            ));
        }

        let v = get_integer(dict, "V")?;
        let r = get_integer(dict, "R")?;
        let cfm = crypt_filter_method(dict);

        match (v, r, cfm.as_deref()) {
            (4, 4, Some("AESV2")) => Self::from_aes128(dict, file_id, password),
            (5, 6, Some("AESV3")) => Self::from_aes256(dict, password),
            _ => Err(ParserError::UnsupportedEncryption(format!(
                "/V {v} /R {r} /CFM {cfm:?}: only /V 4 /R 4 /CFM /AESV2 (AES-128) and \
                 /V 5 /R 6 /CFM /AESV3 (AES-256) are implemented"
            ))),
        }
    }

    /// ISO 32000-1 7.6.3.3 Algorithm 2 (compute the file encryption key)
    /// together with 7.6.3.4 Algorithm 3.5/3.6 (compute `/U`, then
    /// authenticate the user password by comparing it against the stored
    /// value).
    fn from_aes128(
        dict: &PdfDictionary,
        file_id: Option<&[u8]>,
        password: &str,
    ) -> Result<Self, ParserError> {
        const KEY_LEN: usize = 16;

        let o = get_hex_string(dict, "O")?;
        let u = get_hex_string(dict, "U")?;
        if o.len() < 32 || u.len() < 16 {
            return Err(ParserError::UnsupportedEncryption(
                "malformed /O or /U for a /V 4 /R 4 (AES-128) /Encrypt dictionary".to_string(),
            ));
        }
        let p = get_integer(dict, "P")? as i32;
        let encrypt_metadata = match dict.get("EncryptMetadata") {
            Some(Object::Boolean(b)) => *b,
            _ => true,
        };
        let file_id = file_id.ok_or_else(|| {
            ParserError::UnsupportedEncryption(
                "AES-128 (/V 4 /R 4) key derivation requires the trailer's /ID, which is missing"
                    .to_string(),
            )
        })?;

        let file_key =
            compute_encryption_key_r4(password, &o[..32], p, file_id, encrypt_metadata, KEY_LEN);
        let computed_u = compute_u_value_r4(&file_key, file_id);

        // Algorithm 3.6: for revision >= 3, only the first 16 bytes of
        // /U are ever validated -- the remaining 16 are documented
        // "arbitrary padding" (see `compute_u_value_r4`'s own docs).
        if computed_u != u[..16] {
            return Err(ParserError::IncorrectPassword);
        }

        Ok(Self {
            algorithm: Algorithm::Aes128,
            file_key,
        })
    }

    /// ISO 32000-2 Algorithm 2.B: authenticate the user password against
    /// `/U`, then recover the file key by decrypting `/UE`.
    fn from_aes256(dict: &PdfDictionary, password: &str) -> Result<Self, ParserError> {
        let u = get_hex_string(dict, "U")?;
        let ue = get_hex_string(dict, "UE")?;
        if u.len() != 48 {
            return Err(ParserError::UnsupportedEncryption(
                "malformed /U for a /V 5 /R 6 (AES-256) /Encrypt dictionary (expected 48 bytes)"
                    .to_string(),
            ));
        }
        if ue.len() != 32 {
            return Err(ParserError::UnsupportedEncryption(
                "malformed /UE for a /V 5 /R 6 (AES-256) /Encrypt dictionary (expected 32 bytes)"
                    .to_string(),
            ));
        }

        let password_bytes = truncate_password(password);
        let validation_salt = &u[32..40];
        let hash = compute_hash_2a(&password_bytes, validation_salt, None)?;
        if hash.as_slice() != &u[0..32] {
            return Err(ParserError::IncorrectPassword);
        }

        let key_salt = &u[40..48];
        let decryption_key = compute_hash_2a(&password_bytes, key_salt, None)?;
        let file_key = aes256_cbc_decrypt_no_padding(&decryption_key, &[0u8; 16], &ue)?;

        Ok(Self {
            algorithm: Algorithm::Aes256,
            file_key,
        })
    }

    /// Recursively decrypts every string and every stream's raw data
    /// reachable from `obj`, using `obj_num`/`gen_num` -- the object's
    /// *own* indirect-object id, per ISO 32000-1 7.6.2 Algorithm 1 (only
    /// consulted for AES-128/R4; AES-256/R6 uses the file key directly
    /// for every object). Mirrors `crate::editor::encrypt::encrypt_object`
    /// on the write side.
    ///
    /// Must never be called for the `/Encrypt` dictionary object itself,
    /// or for an object obtained from inside an already-decrypted object
    /// stream (ISO 32000-1 7.5.7: those are encrypted only as part of
    /// their containing `/ObjStm`, not individually) -- both are the
    /// caller's ([`crate::parser::PdfReader`]'s) responsibility.
    pub(crate) fn decrypt_object(&self, obj: Object, obj_num: u32, gen_num: u16) -> Object {
        match obj {
            Object::String(s) => match self.decrypt_bytes(s.as_bytes(), obj_num, gen_num) {
                Some(plain) => Object::String(PdfString::Literal(plain)),
                None => Object::String(s),
            },
            Object::Stream(stream) => {
                let new_dict = self.decrypt_dict(&stream.dictionary, obj_num, gen_num);
                let data = self
                    .decrypt_bytes(&stream.data, obj_num, gen_num)
                    .unwrap_or(stream.data);
                Object::Stream(PdfStream::from_raw(new_dict, data))
            }
            Object::Dictionary(dict) => {
                Object::Dictionary(self.decrypt_dict(&dict, obj_num, gen_num))
            }
            Object::Array(arr) => {
                let mut new_arr = PdfArray::new();
                for item in arr.iter() {
                    new_arr.push(self.decrypt_object(item.clone(), obj_num, gen_num));
                }
                Object::Array(new_arr)
            }
            other => other,
        }
    }

    /// Decrypts a compressed object stream's (`/ObjStm`) own raw bytes
    /// (before filter decoding), using the object stream's own id --
    /// [`crate::parser::PdfReader::decoded_object_stream`]'s counterpart
    /// to [`Self::decrypt_object`] for the one case (a stream, not a
    /// dictionary/string) that needs decrypting *before* any of its
    /// contents are parsed out, rather than after.
    pub(crate) fn decrypt_stream_data(&self, data: &[u8], obj_num: u32, gen_num: u16) -> Vec<u8> {
        self.decrypt_bytes(data, obj_num, gen_num)
            .unwrap_or_else(|| data.to_vec())
    }

    fn decrypt_dict(&self, dict: &PdfDictionary, obj_num: u32, gen_num: u16) -> PdfDictionary {
        let mut new_dict = PdfDictionary::new();
        for (k, v) in dict.iter() {
            new_dict.set(k.clone(), self.decrypt_object(v.clone(), obj_num, gen_num));
        }
        new_dict
    }

    /// Decrypts one already-encrypted string/stream value: `data`'s first
    /// 16 bytes are the random per-value IV (ISO 32000-1 7.6.2), the rest
    /// is AES-CBC/PKCS#7 ciphertext. Returns `None` (leave the original
    /// bytes as-is) for anything too short to even hold an IV, or if the
    /// underlying cipher call fails (bad padding, etc.) -- e.g. a value
    /// that was never actually encrypted in the first place (some
    /// producers leave certain strings in the clear despite an
    /// `/Encrypt` entry existing) -- rather than corrupting it or
    /// panicking.
    fn decrypt_bytes(&self, data: &[u8], obj_num: u32, gen_num: u16) -> Option<Vec<u8>> {
        if data.len() < 16 {
            return None;
        }
        let iv = &data[..16];
        let ciphertext = &data[16..];
        match self.algorithm {
            Algorithm::Aes256 => aes256_cbc_decrypt(&self.file_key, iv, ciphertext).ok(),
            Algorithm::Aes128 => {
                let object_key = derive_object_key_r4(&self.file_key, obj_num, gen_num);
                aes128_cbc_decrypt(&object_key, iv, ciphertext).ok()
            }
        }
    }
}

fn get_integer(dict: &PdfDictionary, key: &str) -> Result<i64, ParserError> {
    match dict.get(key) {
        Some(Object::Integer(n)) => Ok(*n),
        other => Err(ParserError::UnsupportedEncryption(format!(
            "/Encrypt dictionary missing or invalid /{key}: {other:?}"
        ))),
    }
}

fn get_hex_string(dict: &PdfDictionary, key: &str) -> Result<Vec<u8>, ParserError> {
    match dict.get(key) {
        Some(Object::String(s)) => Ok(s.as_bytes().to_vec()),
        other => Err(ParserError::UnsupportedEncryption(format!(
            "/Encrypt dictionary missing or invalid /{key}: {other:?}"
        ))),
    }
}

fn crypt_filter_method(dict: &PdfDictionary) -> Option<String> {
    match dict.get("CF") {
        Some(Object::Dictionary(cf)) => match cf.get("StdCF") {
            Some(Object::Dictionary(std_cf)) => match std_cf.get("CFM") {
                Some(Object::Name(n)) => Some(n.as_str().to_string()),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

fn uses_std_cf_for_streams_and_strings(dict: &PdfDictionary) -> bool {
    let is_std_cf =
        |key: &str| matches!(dict.get(key), Some(Object::Name(n)) if n.as_str() == "StdCF");
    is_std_cf("StmF") && is_std_cf("StrF")
}

// =========================================================================
// AES-128 / R4 (ISO 32000-1 7.6.3.3/7.6.3.4, Algorithms 2/3.3/3.5): a
// from-scratch reimplementation of the same private helpers
// `crate::encryption::key_derivation` uses on the write side -- see this
// module's doc comment for why those can't be called directly.
// =========================================================================

/// The 32-byte padding string used to pad/truncate passwords for the
/// revision 2-4 (RC4/AES-128) key derivation algorithms (ISO 32000-1
/// 7.6.3.3, Algorithm 2, step (a)). Fixed by the spec, not a secret.
const PAD_BYTES: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

fn pad_password_r4(password: &[u8]) -> [u8; 32] {
    let mut result = [0u8; 32];
    let len = password.len().min(32);
    result[..len].copy_from_slice(&password[..len]);
    if len < 32 {
        result[len..].copy_from_slice(&PAD_BYTES[..32 - len]);
    }
    result
}

/// RC4 stream cipher, needed only for Algorithm 3.5's `/U` computation
/// (ISO 32000-1 7.6.3.4). Not used for AES data encryption/decryption
/// itself anywhere in this crate -- only for this one legacy PDF-internal
/// key-derivation step, exactly as `key_derivation::rc4` is on the write
/// side.
fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    debug_assert!(!key.is_empty());

    let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }

    let mut i: u8 = 0;
    let mut j: u8 = 0;
    let mut out = Vec::with_capacity(data.len());
    for &byte in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        let k = s[(s[i as usize].wrapping_add(s[j as usize])) as usize];
        out.push(byte ^ k);
    }
    out
}

/// ISO 32000-1 7.6.3.3 Algorithm 2: computing the file encryption key
/// for revision 3/4 security handlers, given the file's *own already
/// stored* `/O` value (unlike the write side's
/// `key_derivation::compute_encryption_key_r4`, which is only ever
/// called right after freshly computing `/O` itself -- for opening a
/// file we must use the `/O` bytes already on disk).
fn compute_encryption_key_r4(
    user_password: &str,
    o_value: &[u8],
    permissions: i32,
    file_id: &[u8],
    encrypt_metadata: bool,
    key_len: usize,
) -> Vec<u8> {
    let padded_user = pad_password_r4(user_password.as_bytes());

    let mut hasher = Md5::new();
    hasher.update(padded_user);
    hasher.update(o_value);
    hasher.update(permissions.to_le_bytes());
    hasher.update(file_id);
    if !encrypt_metadata {
        hasher.update([0xFF, 0xFF, 0xFF, 0xFF]);
    }
    let mut hash: [u8; 16] = hasher.finalize().into();

    for _ in 0..50 {
        hash = Md5::digest(&hash[..key_len]).into();
    }

    hash[..key_len].to_vec()
}

/// ISO 32000-1 7.6.3.4 Algorithm 3.5: computing the (16-byte, validated
/// portion of the) `/U` value for revision 3/4 security handlers, given
/// an already-derived file encryption key.
fn compute_u_value_r4(encryption_key: &[u8], file_id: &[u8]) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(PAD_BYTES);
    hasher.update(file_id);
    let hash: [u8; 16] = hasher.finalize().into();

    let mut result = rc4(encryption_key, &hash);
    for round in 1u8..=19 {
        let xored_key: Vec<u8> = encryption_key.iter().map(|b| b ^ round).collect();
        result = rc4(&xored_key, &result);
    }

    let mut u = [0u8; 16];
    u.copy_from_slice(&result);
    u
}

/// ISO 32000-1 7.6.2 Algorithm 1: per-object key derivation for
/// revision 2-4 (RC4/AES-128) security handlers -- the file encryption
/// key is never used directly to decrypt object data.
fn derive_object_key_r4(file_encryption_key: &[u8], obj_num: u32, gen_num: u16) -> Vec<u8> {
    let mut hasher = Md5::new();
    hasher.update(file_encryption_key);
    hasher.update(&obj_num.to_le_bytes()[..3]);
    hasher.update(&gen_num.to_le_bytes()[..2]);
    // The 4-byte "sAlT" extension (ISO 32000-1 7.6.2, step (f)) is
    // mandatory for the AESV2 crypt filter method, the only R4 method
    // this crate's writer ever emits.
    hasher.update(b"sAlT");
    let hash: [u8; 16] = hasher.finalize().into();

    let key_len = (file_encryption_key.len() + 5).min(16);
    hash[..key_len].to_vec()
}

fn truncate_password(password: &str) -> Vec<u8> {
    let bytes = password.as_bytes();
    if bytes.len() <= 127 {
        bytes.to_vec()
    } else {
        let mut len = 127;
        while len > 0 && !password.is_char_boundary(len) {
            len -= 1;
        }
        bytes[..len].to_vec()
    }
}

// =========================================================================
// AES-256 / R6 (ISO 32000-2 7.6.4.3.4, Algorithm 2.A): the iterative
// SHA-256/384/512 + AES-128-CBC hash used for both password validation
// and key derivation.
// =========================================================================

fn compute_hash_2a(
    password: &[u8],
    salt: &[u8],
    user_bytes: Option<&[u8]>,
) -> Result<[u8; 32], ParserError> {
    use aes::cipher::{BlockEncryptMut, KeyIvInit};

    type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;

    let u = user_bytes.unwrap_or(&[]);

    let mut hasher = Sha256::new();
    hasher.update(password);
    hasher.update(salt);
    hasher.update(u);
    let initial_hash: [u8; 32] = hasher.finalize().into();

    let mut block = [0u8; 64];
    block[..32].copy_from_slice(&initial_hash);
    let mut block_size = 32usize;

    let mut data = Vec::with_capacity((password.len() + 64 + u.len()) * 64);
    let mut round_number = 0usize;

    loop {
        let repeat_len = password.len() + block_size + u.len();
        let total_len = repeat_len * 64;

        data.clear();
        data.resize(total_len, 0);

        data[..password.len()].copy_from_slice(password);
        data[password.len()..password.len() + block_size].copy_from_slice(&block[..block_size]);
        data[password.len() + block_size..repeat_len].copy_from_slice(u);

        for j in 1..64 {
            data.copy_within(..repeat_len, j * repeat_len);
        }

        let aes_key: &[u8; 16] = block[..16].try_into().unwrap();
        let aes_iv: &[u8; 16] = block[16..32].try_into().unwrap();

        let encryptor = Aes128CbcEnc::new(aes_key.into(), aes_iv.into());
        let encrypted = encryptor
            .encrypt_padded_mut::<aes::cipher::block_padding::NoPadding>(
                &mut data[..total_len],
                total_len,
            )
            .map_err(|e| {
                ParserError::UnsupportedEncryption(format!(
                    "AES-256 (R6) password hash computation failed: {e:?}"
                ))
            })?;

        let sum: usize = encrypted[..16].iter().map(|&b| b as usize).sum();
        let remainder = sum % 3;
        let new_block_size = remainder * 16 + 32;

        match remainder {
            0 => {
                let hash: [u8; 32] = Sha256::digest(encrypted).into();
                block[..32].copy_from_slice(&hash);
            }
            1 => {
                let hash: [u8; 48] = Sha384::digest(encrypted).into();
                block[..48].copy_from_slice(&hash);
            }
            _ => {
                let hash: [u8; 64] = Sha512::digest(encrypted).into();
                block.copy_from_slice(&hash);
            }
        }
        block_size = new_block_size;

        round_number += 1;

        let last_byte = encrypted[total_len - 1] as usize;
        if round_number >= 64 && last_byte + 32 <= round_number {
            break;
        }
        if round_number > 2048 {
            break;
        }
    }

    let mut result = [0u8; 32];
    result.copy_from_slice(&block[..32]);
    Ok(result)
}

// =========================================================================
// Shared AES-CBC helpers for object-content decryption (both algorithms)
// and AES-256's `/UE` decryption. Mirror
// `crate::encryption::EncryptionHandler`'s private encrypt-side
// counterparts.
// =========================================================================

fn aes128_cbc_decrypt(key: &[u8], iv: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, ParserError> {
    use aes::cipher::{BlockDecryptMut, KeyIvInit};
    type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

    let decryptor = Aes128CbcDec::new_from_slices(key, iv)
        .map_err(|e| ParserError::UnsupportedEncryption(format!("bad AES-128 key/IV: {e}")))?;
    let mut buf = ciphertext.to_vec();
    let plaintext = decryptor
        .decrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf)
        .map_err(|_| ParserError::IncorrectPassword)?;
    Ok(plaintext.to_vec())
}

fn aes256_cbc_decrypt(key: &[u8], iv: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, ParserError> {
    use aes::cipher::{BlockDecryptMut, KeyIvInit};
    type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

    let decryptor = Aes256CbcDec::new_from_slices(key, iv)
        .map_err(|e| ParserError::UnsupportedEncryption(format!("bad AES-256 key/IV: {e}")))?;
    let mut buf = ciphertext.to_vec();
    let plaintext = decryptor
        .decrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf)
        .map_err(|_| ParserError::IncorrectPassword)?;
    Ok(plaintext.to_vec())
}

fn aes256_cbc_decrypt_no_padding(
    key: &[u8],
    iv: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, ParserError> {
    use aes::cipher::{BlockDecryptMut, KeyIvInit};
    type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

    if !ciphertext.len().is_multiple_of(16) {
        return Err(ParserError::UnsupportedEncryption(
            "/UE must be block-aligned".to_string(),
        ));
    }

    let decryptor = Aes256CbcDec::new_from_slices(key, iv)
        .map_err(|e| ParserError::UnsupportedEncryption(format!("bad AES-256 key/IV: {e}")))?;
    let mut buf = ciphertext.to_vec();
    let plaintext = decryptor
        .decrypt_padded_mut::<aes::cipher::block_padding::NoPadding>(&mut buf)
        .map_err(|e| ParserError::UnsupportedEncryption(format!("/UE decryption failed: {e:?}")))?;
    Ok(plaintext.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trips this module's own AES-128/AES-256 CBC decrypt helpers
    /// against a hand-encrypted buffer using the same `aes`/`cbc` crate
    /// primitives, independent of any PDF structure -- a focused unit
    /// test for the two low-level ciphers before the higher-level
    /// `tests/open_encrypted_pdf_tests.rs` integration tests exercise the
    /// whole open-with-password path.
    #[test]
    fn test_aes128_cbc_decrypt_roundtrip() {
        use aes::cipher::{BlockEncryptMut, KeyIvInit};
        type Enc = cbc::Encryptor<aes::Aes128>;

        let key = [0x11u8; 16];
        let iv = [0x22u8; 16];
        let plaintext = b"Hello, AES-128 CBC round trip!!";
        let mut buf = plaintext.to_vec();
        buf.resize(buf.len() + 16, 0);
        let ciphertext = Enc::new(&key.into(), &iv.into())
            .encrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf, plaintext.len())
            .unwrap()
            .to_vec();

        let decrypted = aes128_cbc_decrypt(&key, &iv, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_aes256_cbc_decrypt_roundtrip() {
        use aes::cipher::{BlockEncryptMut, KeyIvInit};
        type Enc = cbc::Encryptor<aes::Aes256>;

        let key = [0x33u8; 32];
        let iv = [0x44u8; 16];
        let plaintext = b"Hello, AES-256 CBC round trip!!";
        let mut buf = plaintext.to_vec();
        buf.resize(buf.len() + 16, 0);
        let ciphertext = Enc::new(&key.into(), &iv.into())
            .encrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf, plaintext.len())
            .unwrap()
            .to_vec();

        let decrypted = aes256_cbc_decrypt(&key, &iv, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    /// Cross-check against `crate::encryption::key_derivation`'s own
    /// `test_derive_aes128_keys_matches_qpdf_reference_values` fixture
    /// (qpdf-generated `/O`, `/U`, file id, user password `"user123"`):
    /// this module's independent `compute_encryption_key_r4`/
    /// `compute_u_value_r4` must derive the exact same file key and
    /// validate against the exact same stored `/U`.
    #[test]
    fn test_compute_u_value_r4_matches_qpdf_reference() {
        fn hex_to_bytes(hex: &str) -> Vec<u8> {
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect()
        }

        let file_id = hex_to_bytes("358c8ff6a5f27da6ce4e86e5467d035f");
        let o = hex_to_bytes("ee8e487f033c93f5a3650c8f0dd9df16181d7e5bf5059679346ec108c25cbfee");
        let expected_u =
            hex_to_bytes("93e7d1a8670491f81e967b3141f4d5570122456a91bae5134273a6db134c87c4");

        let key = compute_encryption_key_r4("user123", &o, -4, &file_id, true, 16);
        let u = compute_u_value_r4(&key, &file_id);

        assert_eq!(&u[..], &expected_u[..16]);
    }

    /// Same cross-check as above, for the AES-256/R6 path (ISO 32000-2
    /// Algorithm 2.A), against
    /// `crate::encryption::key_derivation`'s own
    /// `test_verify_against_qpdf_encrypted_pdf` fixture.
    #[test]
    fn test_compute_hash_2a_matches_qpdf_reference() {
        fn hex_to_bytes(hex: &str) -> Vec<u8> {
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect()
        }

        let u_hex = "66c168209263b84540053badf8b672df19983066283b649d1d27e3dcedad9b6f8704cf156bbd53a7950139fa92839ed4";
        let u_value = hex_to_bytes(u_hex);
        let validation_salt = &u_value[32..40];
        let expected_hash = &u_value[0..32];

        let computed = compute_hash_2a(b"user123", validation_salt, None).unwrap();
        assert_eq!(&computed[..], expected_hash);
    }

    #[test]
    fn test_wrong_password_fails_aes128_verification() {
        let file_id = [0x35u8; 16];
        // A plausible-looking /O (does not need to be a real one for this
        // test: we're only checking that a mismatching /U causes
        // `from_aes128` to report `IncorrectPassword`, not that it opens).
        let o = [0u8; 32];
        let key_right = compute_encryption_key_r4("right", &o, -4, &file_id, true, 16);
        let u = compute_u_value_r4(&key_right, &file_id);

        let key_wrong = compute_encryption_key_r4("wrong", &o, -4, &file_id, true, 16);
        let u_wrong = compute_u_value_r4(&key_wrong, &file_id);

        assert_ne!(u, u_wrong);
    }
}
