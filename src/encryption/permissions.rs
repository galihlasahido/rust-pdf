//! PDF document permissions.
//!
//! Permissions control what operations are allowed on an encrypted PDF.

use zeroize::Zeroize;

/// PDF document permissions.
///
/// These flags control what operations are allowed on an encrypted document.
/// By default, all permissions are denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Zeroize)]
pub struct Permissions {
    flags: i32,
}

// Permission bit flags (PDF 2.0 Reference, Table 22)
const PRINT: i32 = 1 << 2;           // Bit 3: Print
const MODIFY: i32 = 1 << 3;          // Bit 4: Modify contents
const COPY: i32 = 1 << 4;            // Bit 5: Copy text/graphics
const ANNOTATE: i32 = 1 << 5;        // Bit 6: Add/modify annotations, fill forms
const FILL_FORMS: i32 = 1 << 8;      // Bit 9: Fill form fields
const EXTRACT: i32 = 1 << 9;         // Bit 10: Extract (for accessibility)
const ASSEMBLE: i32 = 1 << 10;       // Bit 11: Assemble (insert, rotate, delete pages)
const PRINT_HIGH_QUALITY: i32 = 1 << 11; // Bit 12: Print high quality

// Reserved bits that should be set to 1
const RESERVED_MASK: i32 = 0xFFFFF0C0u32 as i32;

impl Permissions {
    /// Creates permissions with all operations denied.
    pub fn new() -> Self {
        Self {
            flags: RESERVED_MASK,
        }
    }

    /// Creates permissions with all operations allowed.
    pub fn all_allowed() -> Self {
        Self {
            flags: RESERVED_MASK | PRINT | MODIFY | COPY | ANNOTATE |
                   FILL_FORMS | EXTRACT | ASSEMBLE | PRINT_HIGH_QUALITY,
        }
    }

    /// Allow or deny printing.
    pub fn allow_printing(mut self, allow: bool) -> Self {
        if allow {
            self.flags |= PRINT | PRINT_HIGH_QUALITY;
        } else {
            self.flags &= !(PRINT | PRINT_HIGH_QUALITY);
        }
        self
    }

    /// Allow or deny low-quality printing only.
    pub fn allow_low_quality_printing(mut self, allow: bool) -> Self {
        if allow {
            self.flags |= PRINT;
            self.flags &= !PRINT_HIGH_QUALITY;
        } else {
            self.flags &= !PRINT;
        }
        self
    }

    /// Allow or deny modifying document contents.
    pub fn allow_modifying(mut self, allow: bool) -> Self {
        if allow {
            self.flags |= MODIFY;
        } else {
            self.flags &= !MODIFY;
        }
        self
    }

    /// Allow or deny copying text and graphics.
    pub fn allow_copying(mut self, allow: bool) -> Self {
        if allow {
            self.flags |= COPY;
        } else {
            self.flags &= !COPY;
        }
        self
    }

    /// Allow or deny adding/modifying annotations.
    pub fn allow_annotating(mut self, allow: bool) -> Self {
        if allow {
            self.flags |= ANNOTATE;
        } else {
            self.flags &= !ANNOTATE;
        }
        self
    }

    /// Allow or deny filling form fields.
    pub fn allow_filling_forms(mut self, allow: bool) -> Self {
        if allow {
            self.flags |= FILL_FORMS;
        } else {
            self.flags &= !FILL_FORMS;
        }
        self
    }

    /// Allow or deny text extraction for accessibility.
    pub fn allow_extraction(mut self, allow: bool) -> Self {
        if allow {
            self.flags |= EXTRACT;
        } else {
            self.flags &= !EXTRACT;
        }
        self
    }

    /// Allow or deny document assembly (insert, rotate, delete pages).
    pub fn allow_assembly(mut self, allow: bool) -> Self {
        if allow {
            self.flags |= ASSEMBLE;
        } else {
            self.flags &= !ASSEMBLE;
        }
        self
    }

    /// Returns true if printing is allowed.
    pub fn can_print(&self) -> bool {
        self.flags & PRINT != 0
    }

    /// Returns true if high-quality printing is allowed.
    pub fn can_print_high_quality(&self) -> bool {
        self.flags & PRINT_HIGH_QUALITY != 0
    }

    /// Returns true if modifying is allowed.
    pub fn can_modify(&self) -> bool {
        self.flags & MODIFY != 0
    }

    /// Returns true if copying is allowed.
    pub fn can_copy(&self) -> bool {
        self.flags & COPY != 0
    }

    /// Returns true if annotating is allowed.
    pub fn can_annotate(&self) -> bool {
        self.flags & ANNOTATE != 0
    }

    /// Returns true if filling forms is allowed.
    pub fn can_fill_forms(&self) -> bool {
        self.flags & FILL_FORMS != 0
    }

    /// Returns true if extraction is allowed.
    pub fn can_extract(&self) -> bool {
        self.flags & EXTRACT != 0
    }

    /// Returns true if assembly is allowed.
    pub fn can_assemble(&self) -> bool {
        self.flags & ASSEMBLE != 0
    }

    /// Returns the raw permission flags for the /P entry.
    pub fn as_i32(&self) -> i32 {
        self.flags
    }
}

impl Default for Permissions {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_permissions() {
        let perms = Permissions::new();
        assert!(!perms.can_print());
        assert!(!perms.can_modify());
        assert!(!perms.can_copy());
        assert!(!perms.can_annotate());
    }

    #[test]
    fn test_all_allowed() {
        let perms = Permissions::all_allowed();
        assert!(perms.can_print());
        assert!(perms.can_print_high_quality());
        assert!(perms.can_modify());
        assert!(perms.can_copy());
        assert!(perms.can_annotate());
        assert!(perms.can_fill_forms());
        assert!(perms.can_extract());
        assert!(perms.can_assemble());
    }

    #[test]
    fn test_allow_printing() {
        let perms = Permissions::new().allow_printing(true);
        assert!(perms.can_print());
        assert!(perms.can_print_high_quality());
    }

    #[test]
    fn test_low_quality_printing() {
        let perms = Permissions::new().allow_low_quality_printing(true);
        assert!(perms.can_print());
        assert!(!perms.can_print_high_quality());
    }

    #[test]
    fn test_builder_pattern() {
        let perms = Permissions::new()
            .allow_printing(true)
            .allow_copying(false)
            .allow_annotating(true);

        assert!(perms.can_print());
        assert!(!perms.can_copy());
        assert!(perms.can_annotate());
    }
}
