//! Password-protect / encrypt-on-export dialog state: collects a user
//! and/or owner password plus permission flags, later fed into
//! [`crate::editor::EditableDocument::save_encrypted_to_bytes`] (see
//! `app.rs`'s `password_dialog_window`). This is a *terminal export*
//! operation, like signing: this crate's own parser cannot reopen the
//! encrypted output it produces (see [`crate::encryption`]'s
//! `editor::encrypt` module docs), so the dialog writes to a new file and
//! does not attempt to reopen it afterward -- it just confirms success.

use std::path::PathBuf;
use std::sync::mpsc;

use crate::encryption::EncryptionAlgorithm;

pub struct PasswordDialogState {
    pub open: bool,
    pub user_password: String,
    pub owner_password: String,
    pub algorithm: EncryptionAlgorithm,
    pub allow_printing: bool,
    pub allow_modifying: bool,
    pub allow_copying: bool,
    pub allow_annotating: bool,
    pub allow_filling_forms: bool,
    pub allow_extraction: bool,
    pub allow_assembly: bool,
    pub exporting: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    pub error: Option<String>,
    pub success: Option<PathBuf>,
}

impl Default for PasswordDialogState {
    fn default() -> Self {
        Self {
            open: false,
            user_password: String::new(),
            owner_password: String::new(),
            algorithm: EncryptionAlgorithm::Aes256,
            allow_printing: true,
            allow_modifying: true,
            allow_copying: true,
            allow_annotating: true,
            allow_filling_forms: true,
            allow_extraction: true,
            allow_assembly: true,
            exporting: None,
            error: None,
            success: None,
        }
    }
}

impl PasswordDialogState {
    pub fn is_exporting(&self) -> bool {
        self.exporting.is_some()
    }
}
