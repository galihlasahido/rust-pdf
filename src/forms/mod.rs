//! PDF Interactive Forms (AcroForms) support.
//!
//! This module provides functionality for creating interactive PDF forms
//! with various field types including text fields, checkboxes, radio buttons,
//! combo boxes, and push buttons.
//!
//! # Features
//!
//! - Text fields (single-line and multi-line)
//! - Checkboxes
//! - Radio button groups
//! - Combo boxes (dropdown lists)
//! - List boxes
//! - Push buttons
//!
//! # Example
//!
//! ```ignore
//! use rust_pdf::prelude::*;
//! use rust_pdf::forms::{TextField, CheckBox, ComboBox, FormField};
//!
//! // Create form fields
//! let name_field = TextField::new("name")
//!     .rect(100.0, 700.0, 200.0, 20.0)
//!     .default_value("Enter your name")
//!     .max_length(50);
//!
//! let subscribe = CheckBox::new("subscribe")
//!     .rect(100.0, 650.0, 20.0, 20.0)
//!     .checked(true);
//!
//! let country = ComboBox::new("country")
//!     .rect(100.0, 600.0, 150.0, 20.0)
//!     .options(vec!["USA", "Canada", "UK", "Germany", "France"])
//!     .selected_index(0);
//!
//! // Add to page
//! let page = PageBuilder::a4()
//!     .font("F1", Standard14Font::Helvetica)
//!     .form_field(name_field)
//!     .form_field(subscribe)
//!     .form_field(country)
//!     .content(content)
//!     .build();
//! ```

mod field;
mod widget;

pub use field::{
    CheckBox, ComboBox, FieldFlags, FormField, FormFieldType, ListBox, PushButton,
    RadioButton, RadioGroup, TextField,
};
pub use widget::{AppearanceBuilder, BorderStyle};

use crate::types::Rectangle;

/// Result type for form operations.
pub type FormResult<T> = Result<T, crate::error::FormError>;

/// Common trait for all form fields.
pub trait FormFieldTrait {
    /// Returns the field name.
    fn name(&self) -> &str;

    /// Returns the field type.
    fn field_type(&self) -> FormFieldType;

    /// Returns the field rectangle (position and size).
    fn rect(&self) -> Rectangle;

    /// Returns the field flags.
    fn flags(&self) -> FieldFlags;

    /// Converts to a generic FormField.
    fn to_form_field(&self) -> FormField;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_field_creation() {
        let field = TextField::new("username")
            .rect(100.0, 700.0, 200.0, 20.0)
            .default_value("Enter username");

        assert_eq!(field.name(), "username");
        assert_eq!(field.field_type(), FormFieldType::Text);
    }

    #[test]
    fn test_checkbox_creation() {
        let field = CheckBox::new("agree")
            .rect(100.0, 650.0, 20.0, 20.0)
            .checked(true);

        assert_eq!(field.name(), "agree");
        assert_eq!(field.field_type(), FormFieldType::CheckBox);
        assert!(field.is_checked());
    }

    #[test]
    fn test_combobox_creation() {
        let field = ComboBox::new("country")
            .rect(100.0, 600.0, 150.0, 20.0)
            .options(vec!["USA", "Canada", "UK"]);

        assert_eq!(field.name(), "country");
        assert_eq!(field.field_type(), FormFieldType::ComboBox);
        assert_eq!(field.get_options().len(), 3);
    }

    #[test]
    fn test_radio_group_creation() {
        let group = RadioGroup::new("gender")
            .add_button(RadioButton::new("male").rect(100.0, 550.0, 20.0, 20.0))
            .add_button(RadioButton::new("female").rect(100.0, 520.0, 20.0, 20.0))
            .selected(0);

        assert_eq!(group.name(), "gender");
        assert_eq!(group.get_buttons().len(), 2);
    }
}
