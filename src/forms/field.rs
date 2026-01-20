//! Form field types.

use crate::color::Color;
use crate::types::Rectangle;
use super::{FormFieldTrait, BorderStyle};
use bitflags::bitflags;

/// Form field type enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormFieldType {
    /// Text input field.
    Text,
    /// Checkbox field.
    CheckBox,
    /// Radio button (part of a group).
    RadioButton,
    /// Combo box (dropdown list).
    ComboBox,
    /// List box (scrollable list).
    ListBox,
    /// Push button.
    PushButton,
}

impl FormFieldType {
    /// Returns the PDF field type name.
    pub fn pdf_type(&self) -> &'static str {
        match self {
            FormFieldType::Text => "Tx",
            FormFieldType::CheckBox => "Btn",
            FormFieldType::RadioButton => "Btn",
            FormFieldType::ComboBox => "Ch",
            FormFieldType::ListBox => "Ch",
            FormFieldType::PushButton => "Btn",
        }
    }
}

bitflags! {
    /// Field flags as defined in PDF specification.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FieldFlags: u32 {
        /// Field is read-only.
        const READ_ONLY = 1 << 0;
        /// Field is required.
        const REQUIRED = 1 << 1;
        /// Field should not be exported.
        const NO_EXPORT = 1 << 2;

        // Text field specific flags
        /// Multi-line text field.
        const MULTILINE = 1 << 12;
        /// Password field (characters displayed as asterisks).
        const PASSWORD = 1 << 13;
        /// File select field.
        const FILE_SELECT = 1 << 20;
        /// Do not spell check.
        const DO_NOT_SPELL_CHECK = 1 << 22;
        /// Do not scroll.
        const DO_NOT_SCROLL = 1 << 23;
        /// Comb field (equal spacing).
        const COMB = 1 << 24;
        /// Rich text field.
        const RICH_TEXT = 1 << 25;

        // Button specific flags
        /// No toggle to off (radio buttons).
        const NO_TOGGLE_TO_OFF = 1 << 14;
        /// Radio button (vs checkbox).
        const RADIO = 1 << 15;
        /// Push button.
        const PUSH_BUTTON = 1 << 16;
        /// Radio buttons in unison.
        const RADIOS_IN_UNISON = 1 << 25;

        // Choice field specific flags
        /// Combo box (vs list box).
        const COMBO = 1 << 17;
        /// Editable combo box.
        const EDIT = 1 << 18;
        /// Sort options alphabetically.
        const SORT = 1 << 19;
        /// Multi-select list box.
        const MULTI_SELECT = 1 << 21;
        /// Commit on selection change.
        const COMMIT_ON_SEL_CHANGE = 1 << 26;
    }
}

impl Default for FieldFlags {
    fn default() -> Self {
        FieldFlags::empty()
    }
}

/// Generic form field that can hold any field type.
#[derive(Debug, Clone)]
pub struct FormField {
    /// Field name (unique identifier).
    pub name: String,
    /// Field type.
    pub field_type: FormFieldType,
    /// Field position and size.
    pub rect: Rectangle,
    /// Field flags.
    pub flags: FieldFlags,
    /// Default value.
    pub default_value: Option<String>,
    /// Current value.
    pub value: Option<String>,
    /// Maximum length (for text fields).
    pub max_length: Option<u32>,
    /// Options (for choice fields).
    pub options: Vec<String>,
    /// Selected index/indices (for choice fields).
    pub selected_indices: Vec<usize>,
    /// Is checked (for checkboxes).
    pub checked: bool,
    /// Export value (for checkboxes/radio buttons).
    pub export_value: Option<String>,
    /// Font name for text rendering.
    pub font_name: Option<String>,
    /// Font size.
    pub font_size: f64,
    /// Text color.
    pub text_color: Color,
    /// Background color.
    pub background_color: Option<Color>,
    /// Border color.
    pub border_color: Option<Color>,
    /// Border style.
    pub border_style: BorderStyle,
    /// Border width.
    pub border_width: f64,
    /// Tooltip/alternate field name.
    pub tooltip: Option<String>,
    /// Button caption (for push buttons).
    pub caption: Option<String>,
    /// Radio group buttons (for radio groups).
    pub radio_buttons: Vec<RadioButton>,
}

impl Default for FormField {
    fn default() -> Self {
        Self {
            name: String::new(),
            field_type: FormFieldType::Text,
            rect: Rectangle::new(0.0, 0.0, 100.0, 20.0),
            flags: FieldFlags::empty(),
            default_value: None,
            value: None,
            max_length: None,
            options: Vec::new(),
            selected_indices: Vec::new(),
            checked: false,
            export_value: None,
            font_name: None,
            font_size: 12.0,
            text_color: Color::BLACK,
            background_color: Some(Color::WHITE),
            border_color: Some(Color::BLACK),
            border_style: BorderStyle::Solid,
            border_width: 1.0,
            tooltip: None,
            caption: None,
            radio_buttons: Vec::new(),
        }
    }
}

/// Text input field.
#[derive(Debug, Clone)]
pub struct TextField {
    name: String,
    rect: Rectangle,
    flags: FieldFlags,
    default_value: Option<String>,
    value: Option<String>,
    max_length: Option<u32>,
    font_name: Option<String>,
    font_size: f64,
    text_color: Color,
    background_color: Option<Color>,
    border_color: Option<Color>,
    border_style: BorderStyle,
    border_width: f64,
    tooltip: Option<String>,
}

impl TextField {
    /// Creates a new text field with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rect: Rectangle::new(0.0, 0.0, 200.0, 20.0),
            flags: FieldFlags::empty(),
            default_value: None,
            value: None,
            max_length: None,
            font_name: None,
            font_size: 12.0,
            text_color: Color::BLACK,
            background_color: Some(Color::WHITE),
            border_color: Some(Color::BLACK),
            border_style: BorderStyle::Solid,
            border_width: 1.0,
            tooltip: None,
        }
    }

    /// Sets the field position and size.
    pub fn rect(mut self, x: f64, y: f64, width: f64, height: f64) -> Self {
        self.rect = Rectangle::new(x, y, x + width, y + height);
        self
    }

    /// Sets the field rectangle.
    pub fn rectangle(mut self, rect: Rectangle) -> Self {
        self.rect = rect;
        self
    }

    /// Sets the default value.
    pub fn default_value(mut self, value: impl Into<String>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    /// Sets the current value.
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Sets the maximum length.
    pub fn max_length(mut self, length: u32) -> Self {
        self.max_length = Some(length);
        self
    }

    /// Makes the field multi-line.
    pub fn multiline(mut self) -> Self {
        self.flags |= FieldFlags::MULTILINE;
        self
    }

    /// Makes the field a password field.
    pub fn password(mut self) -> Self {
        self.flags |= FieldFlags::PASSWORD;
        self
    }

    /// Makes the field read-only.
    pub fn read_only(mut self) -> Self {
        self.flags |= FieldFlags::READ_ONLY;
        self
    }

    /// Makes the field required.
    pub fn required(mut self) -> Self {
        self.flags |= FieldFlags::REQUIRED;
        self
    }

    /// Sets the font name.
    pub fn font(mut self, name: impl Into<String>) -> Self {
        self.font_name = Some(name.into());
        self
    }

    /// Sets the font size.
    pub fn font_size(mut self, size: f64) -> Self {
        self.font_size = size;
        self
    }

    /// Sets the text color.
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    /// Sets the background color.
    pub fn background_color(mut self, color: Color) -> Self {
        self.background_color = Some(color);
        self
    }

    /// Sets the border color.
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = Some(color);
        self
    }

    /// Sets the border style.
    pub fn border_style(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        self
    }

    /// Sets the border width.
    pub fn border_width(mut self, width: f64) -> Self {
        self.border_width = width;
        self
    }

    /// Sets the tooltip.
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Makes this a comb field with equal character spacing.
    pub fn comb(mut self) -> Self {
        self.flags |= FieldFlags::COMB;
        self
    }
}

impl FormFieldTrait for TextField {
    fn name(&self) -> &str {
        &self.name
    }

    fn field_type(&self) -> FormFieldType {
        FormFieldType::Text
    }

    fn rect(&self) -> Rectangle {
        self.rect
    }

    fn flags(&self) -> FieldFlags {
        self.flags
    }

    fn to_form_field(&self) -> FormField {
        FormField {
            name: self.name.clone(),
            field_type: FormFieldType::Text,
            rect: self.rect,
            flags: self.flags,
            default_value: self.default_value.clone(),
            value: self.value.clone(),
            max_length: self.max_length,
            font_name: self.font_name.clone(),
            font_size: self.font_size,
            text_color: self.text_color,
            background_color: self.background_color,
            border_color: self.border_color,
            border_style: self.border_style,
            border_width: self.border_width,
            tooltip: self.tooltip.clone(),
            ..Default::default()
        }
    }
}

/// Checkbox field.
#[derive(Debug, Clone)]
pub struct CheckBox {
    name: String,
    rect: Rectangle,
    flags: FieldFlags,
    checked: bool,
    export_value: String,
    background_color: Option<Color>,
    border_color: Option<Color>,
    border_style: BorderStyle,
    border_width: f64,
    check_color: Color,
    tooltip: Option<String>,
}

impl CheckBox {
    /// Creates a new checkbox with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rect: Rectangle::new(0.0, 0.0, 20.0, 20.0),
            flags: FieldFlags::empty(),
            checked: false,
            export_value: "Yes".to_string(),
            background_color: Some(Color::WHITE),
            border_color: Some(Color::BLACK),
            border_style: BorderStyle::Solid,
            border_width: 1.0,
            check_color: Color::BLACK,
            tooltip: None,
        }
    }

    /// Sets the field position and size.
    pub fn rect(mut self, x: f64, y: f64, width: f64, height: f64) -> Self {
        self.rect = Rectangle::new(x, y, x + width, y + height);
        self
    }

    /// Sets the field rectangle.
    pub fn rectangle(mut self, rect: Rectangle) -> Self {
        self.rect = rect;
        self
    }

    /// Sets whether the checkbox is checked.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Sets the export value (value when checked).
    pub fn export_value(mut self, value: impl Into<String>) -> Self {
        self.export_value = value.into();
        self
    }

    /// Makes the field read-only.
    pub fn read_only(mut self) -> Self {
        self.flags |= FieldFlags::READ_ONLY;
        self
    }

    /// Makes the field required.
    pub fn required(mut self) -> Self {
        self.flags |= FieldFlags::REQUIRED;
        self
    }

    /// Sets the background color.
    pub fn background_color(mut self, color: Color) -> Self {
        self.background_color = Some(color);
        self
    }

    /// Sets the border color.
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = Some(color);
        self
    }

    /// Sets the check mark color.
    pub fn check_color(mut self, color: Color) -> Self {
        self.check_color = color;
        self
    }

    /// Sets the border style.
    pub fn border_style(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        self
    }

    /// Sets the border width.
    pub fn border_width(mut self, width: f64) -> Self {
        self.border_width = width;
        self
    }

    /// Sets the tooltip.
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Returns whether the checkbox is checked.
    pub fn is_checked(&self) -> bool {
        self.checked
    }

    /// Returns the export value.
    pub fn get_export_value(&self) -> &str {
        &self.export_value
    }

    /// Returns the check color.
    pub fn get_check_color(&self) -> Color {
        self.check_color
    }
}

impl FormFieldTrait for CheckBox {
    fn name(&self) -> &str {
        &self.name
    }

    fn field_type(&self) -> FormFieldType {
        FormFieldType::CheckBox
    }

    fn rect(&self) -> Rectangle {
        self.rect
    }

    fn flags(&self) -> FieldFlags {
        self.flags
    }

    fn to_form_field(&self) -> FormField {
        FormField {
            name: self.name.clone(),
            field_type: FormFieldType::CheckBox,
            rect: self.rect,
            flags: self.flags,
            checked: self.checked,
            export_value: Some(self.export_value.clone()),
            background_color: self.background_color,
            border_color: self.border_color,
            border_style: self.border_style,
            border_width: self.border_width,
            tooltip: self.tooltip.clone(),
            text_color: self.check_color,
            ..Default::default()
        }
    }
}

/// Radio button (individual button in a group).
#[derive(Debug, Clone)]
pub struct RadioButton {
    name: String,
    rect: Rectangle,
    export_value: String,
    tooltip: Option<String>,
}

impl RadioButton {
    /// Creates a new radio button with the given name/value.
    pub fn new(name: impl Into<String>) -> Self {
        let name_str = name.into();
        Self {
            name: name_str.clone(),
            rect: Rectangle::new(0.0, 0.0, 20.0, 20.0),
            export_value: name_str,
            tooltip: None,
        }
    }

    /// Sets the button position and size.
    pub fn rect(mut self, x: f64, y: f64, width: f64, height: f64) -> Self {
        self.rect = Rectangle::new(x, y, x + width, y + height);
        self
    }

    /// Sets the button rectangle.
    pub fn rectangle(mut self, rect: Rectangle) -> Self {
        self.rect = rect;
        self
    }

    /// Sets the export value.
    pub fn export_value(mut self, value: impl Into<String>) -> Self {
        self.export_value = value.into();
        self
    }

    /// Sets the tooltip.
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Returns the button name.
    pub fn get_name(&self) -> &str {
        &self.name
    }

    /// Returns the export value.
    pub fn get_export_value(&self) -> &str {
        &self.export_value
    }

    /// Returns the rectangle.
    pub fn get_rect(&self) -> Rectangle {
        self.rect
    }

    /// Returns the tooltip.
    pub fn get_tooltip(&self) -> Option<&str> {
        self.tooltip.as_deref()
    }
}

/// Radio button group.
#[derive(Debug, Clone)]
pub struct RadioGroup {
    name: String,
    buttons: Vec<RadioButton>,
    selected_index: Option<usize>,
    flags: FieldFlags,
    background_color: Option<Color>,
    border_color: Option<Color>,
    border_style: BorderStyle,
    border_width: f64,
    selected_color: Color,
    tooltip: Option<String>,
}

impl RadioGroup {
    /// Creates a new radio group with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            buttons: Vec::new(),
            selected_index: None,
            flags: FieldFlags::RADIO | FieldFlags::NO_TOGGLE_TO_OFF,
            background_color: Some(Color::WHITE),
            border_color: Some(Color::BLACK),
            border_style: BorderStyle::Solid,
            border_width: 1.0,
            selected_color: Color::BLACK,
            tooltip: None,
        }
    }

    /// Adds a radio button to the group.
    pub fn add_button(mut self, button: RadioButton) -> Self {
        self.buttons.push(button);
        self
    }

    /// Adds multiple radio buttons.
    pub fn buttons(mut self, buttons: Vec<RadioButton>) -> Self {
        self.buttons = buttons;
        self
    }

    /// Sets the selected button index.
    pub fn selected(mut self, index: usize) -> Self {
        if index < self.buttons.len() {
            self.selected_index = Some(index);
        }
        self
    }

    /// Makes the field read-only.
    pub fn read_only(mut self) -> Self {
        self.flags |= FieldFlags::READ_ONLY;
        self
    }

    /// Makes the field required.
    pub fn required(mut self) -> Self {
        self.flags |= FieldFlags::REQUIRED;
        self
    }

    /// Sets the background color for all buttons.
    pub fn background_color(mut self, color: Color) -> Self {
        self.background_color = Some(color);
        self
    }

    /// Sets the border color for all buttons.
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = Some(color);
        self
    }

    /// Sets the selected indicator color.
    pub fn selected_color(mut self, color: Color) -> Self {
        self.selected_color = color;
        self
    }

    /// Sets the border style for all buttons.
    pub fn border_style(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        self
    }

    /// Sets the border width for all buttons.
    pub fn border_width(mut self, width: f64) -> Self {
        self.border_width = width;
        self
    }

    /// Sets the tooltip.
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Returns the buttons in the group.
    pub fn get_buttons(&self) -> &[RadioButton] {
        &self.buttons
    }

    /// Returns the selected index.
    pub fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    /// Returns the background color.
    pub fn get_background_color(&self) -> Option<Color> {
        self.background_color
    }

    /// Returns the border color.
    pub fn get_border_color(&self) -> Option<Color> {
        self.border_color
    }

    /// Returns the selected color.
    pub fn get_selected_color(&self) -> Color {
        self.selected_color
    }

    /// Returns the border style.
    pub fn get_border_style(&self) -> BorderStyle {
        self.border_style
    }

    /// Returns the border width.
    pub fn get_border_width(&self) -> f64 {
        self.border_width
    }
}

impl FormFieldTrait for RadioGroup {
    fn name(&self) -> &str {
        &self.name
    }

    fn field_type(&self) -> FormFieldType {
        FormFieldType::RadioButton
    }

    fn rect(&self) -> Rectangle {
        // Return bounding box of all buttons
        if self.buttons.is_empty() {
            return Rectangle::new(0.0, 0.0, 0.0, 0.0);
        }

        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;

        for button in &self.buttons {
            min_x = min_x.min(button.rect.llx);
            min_y = min_y.min(button.rect.lly);
            max_x = max_x.max(button.rect.urx);
            max_y = max_y.max(button.rect.ury);
        }

        Rectangle::new(min_x, min_y, max_x, max_y)
    }

    fn flags(&self) -> FieldFlags {
        self.flags
    }

    fn to_form_field(&self) -> FormField {
        FormField {
            name: self.name.clone(),
            field_type: FormFieldType::RadioButton,
            rect: self.rect(),
            flags: self.flags,
            selected_indices: self.selected_index.map(|i| vec![i]).unwrap_or_default(),
            background_color: self.background_color,
            border_color: self.border_color,
            border_style: self.border_style,
            border_width: self.border_width,
            tooltip: self.tooltip.clone(),
            text_color: self.selected_color,
            radio_buttons: self.buttons.clone(),
            ..Default::default()
        }
    }
}

/// Combo box (dropdown list) field.
#[derive(Debug, Clone)]
pub struct ComboBox {
    name: String,
    rect: Rectangle,
    flags: FieldFlags,
    options: Vec<String>,
    selected_index: Option<usize>,
    editable: bool,
    font_name: Option<String>,
    font_size: f64,
    text_color: Color,
    background_color: Option<Color>,
    border_color: Option<Color>,
    border_style: BorderStyle,
    border_width: f64,
    tooltip: Option<String>,
}

impl ComboBox {
    /// Creates a new combo box with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rect: Rectangle::new(0.0, 0.0, 150.0, 20.0),
            flags: FieldFlags::COMBO,
            options: Vec::new(),
            selected_index: None,
            editable: false,
            font_name: None,
            font_size: 12.0,
            text_color: Color::BLACK,
            background_color: Some(Color::WHITE),
            border_color: Some(Color::BLACK),
            border_style: BorderStyle::Solid,
            border_width: 1.0,
            tooltip: None,
        }
    }

    /// Sets the field position and size.
    pub fn rect(mut self, x: f64, y: f64, width: f64, height: f64) -> Self {
        self.rect = Rectangle::new(x, y, x + width, y + height);
        self
    }

    /// Sets the field rectangle.
    pub fn rectangle(mut self, rect: Rectangle) -> Self {
        self.rect = rect;
        self
    }

    /// Sets the options.
    pub fn options<S: Into<String>>(mut self, options: Vec<S>) -> Self {
        self.options = options.into_iter().map(|s| s.into()).collect();
        self
    }

    /// Adds an option.
    pub fn add_option(mut self, option: impl Into<String>) -> Self {
        self.options.push(option.into());
        self
    }

    /// Sets the selected option by index.
    pub fn selected_index(mut self, index: usize) -> Self {
        if index < self.options.len() {
            self.selected_index = Some(index);
        }
        self
    }

    /// Makes the combo box editable.
    pub fn editable(mut self) -> Self {
        self.editable = true;
        self.flags |= FieldFlags::EDIT;
        self
    }

    /// Makes the field read-only.
    pub fn read_only(mut self) -> Self {
        self.flags |= FieldFlags::READ_ONLY;
        self
    }

    /// Makes the field required.
    pub fn required(mut self) -> Self {
        self.flags |= FieldFlags::REQUIRED;
        self
    }

    /// Sorts options alphabetically.
    pub fn sorted(mut self) -> Self {
        self.flags |= FieldFlags::SORT;
        self.options.sort();
        self
    }

    /// Sets the font name.
    pub fn font(mut self, name: impl Into<String>) -> Self {
        self.font_name = Some(name.into());
        self
    }

    /// Sets the font size.
    pub fn font_size(mut self, size: f64) -> Self {
        self.font_size = size;
        self
    }

    /// Sets the text color.
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    /// Sets the background color.
    pub fn background_color(mut self, color: Color) -> Self {
        self.background_color = Some(color);
        self
    }

    /// Sets the border color.
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = Some(color);
        self
    }

    /// Sets the border style.
    pub fn border_style(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        self
    }

    /// Sets the border width.
    pub fn border_width(mut self, width: f64) -> Self {
        self.border_width = width;
        self
    }

    /// Sets the tooltip.
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Returns the options.
    pub fn get_options(&self) -> &[String] {
        &self.options
    }

    /// Returns the selected index.
    pub fn get_selected_index(&self) -> Option<usize> {
        self.selected_index
    }
}

impl FormFieldTrait for ComboBox {
    fn name(&self) -> &str {
        &self.name
    }

    fn field_type(&self) -> FormFieldType {
        FormFieldType::ComboBox
    }

    fn rect(&self) -> Rectangle {
        self.rect
    }

    fn flags(&self) -> FieldFlags {
        self.flags
    }

    fn to_form_field(&self) -> FormField {
        FormField {
            name: self.name.clone(),
            field_type: FormFieldType::ComboBox,
            rect: self.rect,
            flags: self.flags,
            options: self.options.clone(),
            selected_indices: self.selected_index.map(|i| vec![i]).unwrap_or_default(),
            font_name: self.font_name.clone(),
            font_size: self.font_size,
            text_color: self.text_color,
            background_color: self.background_color,
            border_color: self.border_color,
            border_style: self.border_style,
            border_width: self.border_width,
            tooltip: self.tooltip.clone(),
            ..Default::default()
        }
    }
}

/// List box (scrollable list) field.
#[derive(Debug, Clone)]
pub struct ListBox {
    name: String,
    rect: Rectangle,
    flags: FieldFlags,
    options: Vec<String>,
    selected_indices: Vec<usize>,
    multi_select: bool,
    font_name: Option<String>,
    font_size: f64,
    text_color: Color,
    background_color: Option<Color>,
    border_color: Option<Color>,
    border_style: BorderStyle,
    border_width: f64,
    tooltip: Option<String>,
}

impl ListBox {
    /// Creates a new list box with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rect: Rectangle::new(0.0, 0.0, 150.0, 100.0),
            flags: FieldFlags::empty(),
            options: Vec::new(),
            selected_indices: Vec::new(),
            multi_select: false,
            font_name: None,
            font_size: 12.0,
            text_color: Color::BLACK,
            background_color: Some(Color::WHITE),
            border_color: Some(Color::BLACK),
            border_style: BorderStyle::Solid,
            border_width: 1.0,
            tooltip: None,
        }
    }

    /// Sets the field position and size.
    pub fn rect(mut self, x: f64, y: f64, width: f64, height: f64) -> Self {
        self.rect = Rectangle::new(x, y, x + width, y + height);
        self
    }

    /// Sets the field rectangle.
    pub fn rectangle(mut self, rect: Rectangle) -> Self {
        self.rect = rect;
        self
    }

    /// Sets the options.
    pub fn options<S: Into<String>>(mut self, options: Vec<S>) -> Self {
        self.options = options.into_iter().map(|s| s.into()).collect();
        self
    }

    /// Adds an option.
    pub fn add_option(mut self, option: impl Into<String>) -> Self {
        self.options.push(option.into());
        self
    }

    /// Sets the selected option by index.
    pub fn selected_index(mut self, index: usize) -> Self {
        if index < self.options.len() {
            self.selected_indices = vec![index];
        }
        self
    }

    /// Sets multiple selected indices.
    pub fn selected_indices(mut self, indices: Vec<usize>) -> Self {
        self.selected_indices = indices.into_iter().filter(|&i| i < self.options.len()).collect();
        self
    }

    /// Enables multi-selection.
    pub fn multi_select(mut self) -> Self {
        self.multi_select = true;
        self.flags |= FieldFlags::MULTI_SELECT;
        self
    }

    /// Makes the field read-only.
    pub fn read_only(mut self) -> Self {
        self.flags |= FieldFlags::READ_ONLY;
        self
    }

    /// Makes the field required.
    pub fn required(mut self) -> Self {
        self.flags |= FieldFlags::REQUIRED;
        self
    }

    /// Sorts options alphabetically.
    pub fn sorted(mut self) -> Self {
        self.flags |= FieldFlags::SORT;
        self.options.sort();
        self
    }

    /// Sets the font name.
    pub fn font(mut self, name: impl Into<String>) -> Self {
        self.font_name = Some(name.into());
        self
    }

    /// Sets the font size.
    pub fn font_size(mut self, size: f64) -> Self {
        self.font_size = size;
        self
    }

    /// Sets the text color.
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    /// Sets the background color.
    pub fn background_color(mut self, color: Color) -> Self {
        self.background_color = Some(color);
        self
    }

    /// Sets the border color.
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = Some(color);
        self
    }

    /// Sets the border style.
    pub fn border_style(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        self
    }

    /// Sets the border width.
    pub fn border_width(mut self, width: f64) -> Self {
        self.border_width = width;
        self
    }

    /// Sets the tooltip.
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Returns the options.
    pub fn get_options(&self) -> &[String] {
        &self.options
    }

    /// Returns the selected indices.
    pub fn get_selected_indices(&self) -> &[usize] {
        &self.selected_indices
    }
}

impl FormFieldTrait for ListBox {
    fn name(&self) -> &str {
        &self.name
    }

    fn field_type(&self) -> FormFieldType {
        FormFieldType::ListBox
    }

    fn rect(&self) -> Rectangle {
        self.rect
    }

    fn flags(&self) -> FieldFlags {
        self.flags
    }

    fn to_form_field(&self) -> FormField {
        FormField {
            name: self.name.clone(),
            field_type: FormFieldType::ListBox,
            rect: self.rect,
            flags: self.flags,
            options: self.options.clone(),
            selected_indices: self.selected_indices.clone(),
            font_name: self.font_name.clone(),
            font_size: self.font_size,
            text_color: self.text_color,
            background_color: self.background_color,
            border_color: self.border_color,
            border_style: self.border_style,
            border_width: self.border_width,
            tooltip: self.tooltip.clone(),
            ..Default::default()
        }
    }
}

/// Push button field.
#[derive(Debug, Clone)]
pub struct PushButton {
    name: String,
    rect: Rectangle,
    flags: FieldFlags,
    caption: String,
    font_name: Option<String>,
    font_size: f64,
    text_color: Color,
    background_color: Option<Color>,
    border_color: Option<Color>,
    border_style: BorderStyle,
    border_width: f64,
    tooltip: Option<String>,
}

impl PushButton {
    /// Creates a new push button with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rect: Rectangle::new(0.0, 0.0, 100.0, 30.0),
            flags: FieldFlags::PUSH_BUTTON,
            caption: "Button".to_string(),
            font_name: None,
            font_size: 12.0,
            text_color: Color::BLACK,
            background_color: Some(Color::gray(0.9)),
            border_color: Some(Color::BLACK),
            border_style: BorderStyle::Beveled,
            border_width: 1.0,
            tooltip: None,
        }
    }

    /// Sets the field position and size.
    pub fn rect(mut self, x: f64, y: f64, width: f64, height: f64) -> Self {
        self.rect = Rectangle::new(x, y, x + width, y + height);
        self
    }

    /// Sets the field rectangle.
    pub fn rectangle(mut self, rect: Rectangle) -> Self {
        self.rect = rect;
        self
    }

    /// Sets the button caption.
    pub fn caption(mut self, text: impl Into<String>) -> Self {
        self.caption = text.into();
        self
    }

    /// Makes the field read-only.
    pub fn read_only(mut self) -> Self {
        self.flags |= FieldFlags::READ_ONLY;
        self
    }

    /// Sets the font name.
    pub fn font(mut self, name: impl Into<String>) -> Self {
        self.font_name = Some(name.into());
        self
    }

    /// Sets the font size.
    pub fn font_size(mut self, size: f64) -> Self {
        self.font_size = size;
        self
    }

    /// Sets the text color.
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    /// Sets the background color.
    pub fn background_color(mut self, color: Color) -> Self {
        self.background_color = Some(color);
        self
    }

    /// Sets the border color.
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = Some(color);
        self
    }

    /// Sets the border style.
    pub fn border_style(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        self
    }

    /// Sets the border width.
    pub fn border_width(mut self, width: f64) -> Self {
        self.border_width = width;
        self
    }

    /// Sets the tooltip.
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Returns the caption.
    pub fn get_caption(&self) -> &str {
        &self.caption
    }
}

impl FormFieldTrait for PushButton {
    fn name(&self) -> &str {
        &self.name
    }

    fn field_type(&self) -> FormFieldType {
        FormFieldType::PushButton
    }

    fn rect(&self) -> Rectangle {
        self.rect
    }

    fn flags(&self) -> FieldFlags {
        self.flags
    }

    fn to_form_field(&self) -> FormField {
        FormField {
            name: self.name.clone(),
            field_type: FormFieldType::PushButton,
            rect: self.rect,
            flags: self.flags,
            caption: Some(self.caption.clone()),
            font_name: self.font_name.clone(),
            font_size: self.font_size,
            text_color: self.text_color,
            background_color: self.background_color,
            border_color: self.border_color,
            border_style: self.border_style,
            border_width: self.border_width,
            tooltip: self.tooltip.clone(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_field_type_pdf_type() {
        assert_eq!(FormFieldType::Text.pdf_type(), "Tx");
        assert_eq!(FormFieldType::CheckBox.pdf_type(), "Btn");
        assert_eq!(FormFieldType::RadioButton.pdf_type(), "Btn");
        assert_eq!(FormFieldType::ComboBox.pdf_type(), "Ch");
        assert_eq!(FormFieldType::ListBox.pdf_type(), "Ch");
        assert_eq!(FormFieldType::PushButton.pdf_type(), "Btn");
    }

    #[test]
    fn test_field_flags() {
        let mut flags = FieldFlags::empty();
        assert!(!flags.contains(FieldFlags::READ_ONLY));

        flags |= FieldFlags::READ_ONLY;
        assert!(flags.contains(FieldFlags::READ_ONLY));

        flags |= FieldFlags::REQUIRED;
        assert!(flags.contains(FieldFlags::READ_ONLY));
        assert!(flags.contains(FieldFlags::REQUIRED));
    }

    #[test]
    fn test_text_field_builder() {
        let field = TextField::new("email")
            .rect(100.0, 500.0, 200.0, 25.0)
            .default_value("user@example.com")
            .max_length(100)
            .required();

        assert_eq!(field.name(), "email");
        assert!(field.flags().contains(FieldFlags::REQUIRED));
    }

    #[test]
    fn test_checkbox_builder() {
        let field = CheckBox::new("terms")
            .rect(100.0, 400.0, 18.0, 18.0)
            .export_value("accepted")
            .checked(true);

        assert_eq!(field.name(), "terms");
        assert!(field.is_checked());
        assert_eq!(field.get_export_value(), "accepted");
    }

    #[test]
    fn test_combobox_builder() {
        let field = ComboBox::new("country")
            .rect(100.0, 300.0, 150.0, 25.0)
            .options(vec!["USA", "Canada", "UK"])
            .selected_index(0);

        assert_eq!(field.name(), "country");
        assert_eq!(field.get_options().len(), 3);
        assert_eq!(field.get_selected_index(), Some(0));
    }

    #[test]
    fn test_listbox_builder() {
        let field = ListBox::new("items")
            .rect(100.0, 200.0, 150.0, 80.0)
            .options(vec!["Item 1", "Item 2", "Item 3"])
            .multi_select()
            .selected_indices(vec![0, 2]);

        assert_eq!(field.name(), "items");
        assert_eq!(field.get_options().len(), 3);
        assert_eq!(field.get_selected_indices(), &[0, 2]);
    }

    #[test]
    fn test_push_button_builder() {
        let field = PushButton::new("submit")
            .rect(100.0, 100.0, 100.0, 30.0)
            .caption("Submit Form");

        assert_eq!(field.name(), "submit");
        assert_eq!(field.get_caption(), "Submit Form");
    }
}
