//! Widget appearance generation.

use crate::color::Color;
use crate::types::Rectangle;

/// Border style for form fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderStyle {
    /// Solid border.
    #[default]
    Solid,
    /// Dashed border.
    Dashed,
    /// Beveled border (3D effect).
    Beveled,
    /// Inset border (3D effect).
    Inset,
    /// Underline only.
    Underline,
}

impl BorderStyle {
    /// Returns the PDF border style code.
    pub fn pdf_code(&self) -> &'static str {
        match self {
            BorderStyle::Solid => "S",
            BorderStyle::Dashed => "D",
            BorderStyle::Beveled => "B",
            BorderStyle::Inset => "I",
            BorderStyle::Underline => "U",
        }
    }
}

/// Builder for widget appearances.
#[derive(Debug, Clone)]
pub struct AppearanceBuilder {
    rect: Rectangle,
    background_color: Option<Color>,
    border_color: Option<Color>,
    border_style: BorderStyle,
    border_width: f64,
}

impl AppearanceBuilder {
    /// Creates a new appearance builder.
    pub fn new(rect: Rectangle) -> Self {
        Self {
            rect,
            background_color: None,
            border_color: None,
            border_style: BorderStyle::Solid,
            border_width: 1.0,
        }
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

    /// Builds appearance stream for a text field.
    pub fn build_text_appearance(&self, text: Option<&str>, font_name: &str, font_size: f64, text_color: Color) -> String {
        let width = self.rect.width();
        let height = self.rect.height();
        let mut stream = String::new();

        // Save state
        stream.push_str("q\n");

        // Background
        if let Some(bg) = &self.background_color {
            stream.push_str(&format_color_fill(bg));
            stream.push_str(&format!("0 0 {} {} re f\n", width, height));
        }

        // Border
        if let Some(bc) = &self.border_color {
            stream.push_str(&format_color_stroke(bc));
            stream.push_str(&format!("{} w\n", self.border_width));

            match self.border_style {
                BorderStyle::Dashed => {
                    stream.push_str("[3] 0 d\n");
                }
                _ => {}
            }

            let offset = self.border_width / 2.0;
            stream.push_str(&format!(
                "{} {} {} {} re S\n",
                offset, offset,
                width - self.border_width,
                height - self.border_width
            ));
        }

        // Text
        if let Some(txt) = text {
            if !txt.is_empty() {
                stream.push_str("BT\n");
                stream.push_str(&format_color_fill(&text_color));
                stream.push_str(&format!("/{} {} Tf\n", font_name, font_size));

                // Position text with padding
                let padding = 4.0;
                let text_y = (height - font_size) / 2.0 + 2.0;
                stream.push_str(&format!("{} {} Td\n", padding, text_y));
                stream.push_str(&format!("({}) Tj\n", escape_pdf_string(txt)));
                stream.push_str("ET\n");
            }
        }

        // Restore state
        stream.push_str("Q\n");

        stream
    }

    /// Builds appearance stream for a checkbox (checked state).
    pub fn build_checkbox_checked(&self, check_color: Color) -> String {
        let width = self.rect.width();
        let height = self.rect.height();
        let mut stream = String::new();

        // Save state
        stream.push_str("q\n");

        // Background
        if let Some(bg) = &self.background_color {
            stream.push_str(&format_color_fill(bg));
            stream.push_str(&format!("0 0 {} {} re f\n", width, height));
        }

        // Border
        if let Some(bc) = &self.border_color {
            stream.push_str(&format_color_stroke(bc));
            stream.push_str(&format!("{} w\n", self.border_width));
            let offset = self.border_width / 2.0;
            stream.push_str(&format!(
                "{} {} {} {} re S\n",
                offset, offset,
                width - self.border_width,
                height - self.border_width
            ));
        }

        // Checkmark
        stream.push_str(&format_color_stroke(&check_color));
        stream.push_str("2 w\n");

        // Draw checkmark (two lines)
        let margin = width * 0.2;
        let x1 = margin;
        let y1 = height * 0.5;
        let x2 = width * 0.4;
        let y2 = margin;
        let x3 = width - margin;
        let y3 = height - margin;

        stream.push_str(&format!("{} {} m\n", x1, y1));
        stream.push_str(&format!("{} {} l\n", x2, y2));
        stream.push_str(&format!("{} {} l\n", x3, y3));
        stream.push_str("S\n");

        // Restore state
        stream.push_str("Q\n");

        stream
    }

    /// Builds appearance stream for a checkbox (unchecked state).
    pub fn build_checkbox_unchecked(&self) -> String {
        let width = self.rect.width();
        let height = self.rect.height();
        let mut stream = String::new();

        // Save state
        stream.push_str("q\n");

        // Background
        if let Some(bg) = &self.background_color {
            stream.push_str(&format_color_fill(bg));
            stream.push_str(&format!("0 0 {} {} re f\n", width, height));
        }

        // Border
        if let Some(bc) = &self.border_color {
            stream.push_str(&format_color_stroke(bc));
            stream.push_str(&format!("{} w\n", self.border_width));
            let offset = self.border_width / 2.0;
            stream.push_str(&format!(
                "{} {} {} {} re S\n",
                offset, offset,
                width - self.border_width,
                height - self.border_width
            ));
        }

        // Restore state
        stream.push_str("Q\n");

        stream
    }

    /// Builds appearance stream for a radio button (selected state).
    pub fn build_radio_selected(&self, selected_color: Color) -> String {
        let width = self.rect.width();
        let height = self.rect.height();
        let mut stream = String::new();

        let cx = width / 2.0;
        let cy = height / 2.0;
        let radius = width.min(height) / 2.0 - self.border_width;

        // Save state
        stream.push_str("q\n");

        // Background circle
        if let Some(bg) = &self.background_color {
            stream.push_str(&format_color_fill(bg));
            stream.push_str(&draw_circle(cx, cy, radius));
            stream.push_str("f\n");
        }

        // Border circle
        if let Some(bc) = &self.border_color {
            stream.push_str(&format_color_stroke(bc));
            stream.push_str(&format!("{} w\n", self.border_width));
            stream.push_str(&draw_circle(cx, cy, radius));
            stream.push_str("S\n");
        }

        // Inner filled circle (selected indicator)
        stream.push_str(&format_color_fill(&selected_color));
        let inner_radius = radius * 0.5;
        stream.push_str(&draw_circle(cx, cy, inner_radius));
        stream.push_str("f\n");

        // Restore state
        stream.push_str("Q\n");

        stream
    }

    /// Builds appearance stream for a radio button (unselected state).
    pub fn build_radio_unselected(&self) -> String {
        let width = self.rect.width();
        let height = self.rect.height();
        let mut stream = String::new();

        let cx = width / 2.0;
        let cy = height / 2.0;
        let radius = width.min(height) / 2.0 - self.border_width;

        // Save state
        stream.push_str("q\n");

        // Background circle
        if let Some(bg) = &self.background_color {
            stream.push_str(&format_color_fill(bg));
            stream.push_str(&draw_circle(cx, cy, radius));
            stream.push_str("f\n");
        }

        // Border circle
        if let Some(bc) = &self.border_color {
            stream.push_str(&format_color_stroke(bc));
            stream.push_str(&format!("{} w\n", self.border_width));
            stream.push_str(&draw_circle(cx, cy, radius));
            stream.push_str("S\n");
        }

        // Restore state
        stream.push_str("Q\n");

        stream
    }

    /// Builds appearance stream for a push button.
    pub fn build_button_appearance(&self, caption: &str, font_name: &str, font_size: f64, text_color: Color) -> String {
        let width = self.rect.width();
        let height = self.rect.height();
        let mut stream = String::new();

        // Save state
        stream.push_str("q\n");

        // Background
        if let Some(bg) = &self.background_color {
            stream.push_str(&format_color_fill(bg));
            stream.push_str(&format!("0 0 {} {} re f\n", width, height));
        }

        // 3D effect for beveled style
        if self.border_style == BorderStyle::Beveled {
            // Light edge (top, left)
            stream.push_str("1 1 1 rg\n");
            stream.push_str(&format!("0 {} m\n", height));
            stream.push_str(&format!("{} {} l\n", width, height));
            stream.push_str(&format!("{} {} l\n", width - 2.0, height - 2.0));
            stream.push_str(&format!("2 {} l\n", height - 2.0));
            stream.push_str("2 2 l\n");
            stream.push_str("0 0 l\n");
            stream.push_str("f\n");

            // Dark edge (bottom, right)
            stream.push_str("0.5 0.5 0.5 rg\n");
            stream.push_str(&format!("{} {} m\n", width, height));
            stream.push_str(&format!("{} 0 l\n", width));
            stream.push_str("0 0 l\n");
            stream.push_str("2 2 l\n");
            stream.push_str(&format!("{} 2 l\n", width - 2.0));
            stream.push_str(&format!("{} {} l\n", width - 2.0, height - 2.0));
            stream.push_str("f\n");
        } else if let Some(bc) = &self.border_color {
            // Simple border
            stream.push_str(&format_color_stroke(bc));
            stream.push_str(&format!("{} w\n", self.border_width));
            let offset = self.border_width / 2.0;
            stream.push_str(&format!(
                "{} {} {} {} re S\n",
                offset, offset,
                width - self.border_width,
                height - self.border_width
            ));
        }

        // Caption text (centered)
        if !caption.is_empty() {
            stream.push_str("BT\n");
            stream.push_str(&format_color_fill(&text_color));
            stream.push_str(&format!("/{} {} Tf\n", font_name, font_size));

            // Approximate text width (rough calculation)
            let text_width = caption.len() as f64 * font_size * 0.5;
            let text_x = (width - text_width) / 2.0;
            let text_y = (height - font_size) / 2.0 + 2.0;

            stream.push_str(&format!("{} {} Td\n", text_x.max(4.0), text_y));
            stream.push_str(&format!("({}) Tj\n", escape_pdf_string(caption)));
            stream.push_str("ET\n");
        }

        // Restore state
        stream.push_str("Q\n");

        stream
    }

    /// Builds appearance stream for a combo box.
    pub fn build_combobox_appearance(&self, selected_text: Option<&str>, font_name: &str, font_size: f64, text_color: Color) -> String {
        let width = self.rect.width();
        let height = self.rect.height();
        let mut stream = String::new();

        // Save state
        stream.push_str("q\n");

        // Background
        if let Some(bg) = &self.background_color {
            stream.push_str(&format_color_fill(bg));
            stream.push_str(&format!("0 0 {} {} re f\n", width, height));
        }

        // Border
        if let Some(bc) = &self.border_color {
            stream.push_str(&format_color_stroke(bc));
            stream.push_str(&format!("{} w\n", self.border_width));
            let offset = self.border_width / 2.0;
            stream.push_str(&format!(
                "{} {} {} {} re S\n",
                offset, offset,
                width - self.border_width,
                height - self.border_width
            ));
        }

        // Dropdown arrow area
        let arrow_width = height; // Square area for arrow
        let arrow_x = width - arrow_width;

        // Arrow area background
        stream.push_str("0.9 0.9 0.9 rg\n");
        stream.push_str(&format!("{} 0 {} {} re f\n", arrow_x, arrow_width, height));

        // Arrow (downward triangle)
        stream.push_str("0 0 0 rg\n");
        let arrow_cx = arrow_x + arrow_width / 2.0;
        let arrow_top = height * 0.7;
        let arrow_bottom = height * 0.3;
        let arrow_half = arrow_width * 0.25;

        stream.push_str(&format!("{} {} m\n", arrow_cx - arrow_half, arrow_top));
        stream.push_str(&format!("{} {} l\n", arrow_cx + arrow_half, arrow_top));
        stream.push_str(&format!("{} {} l\n", arrow_cx, arrow_bottom));
        stream.push_str("f\n");

        // Selected text
        if let Some(txt) = selected_text {
            if !txt.is_empty() {
                stream.push_str("BT\n");
                stream.push_str(&format_color_fill(&text_color));
                stream.push_str(&format!("/{} {} Tf\n", font_name, font_size));

                let padding = 4.0;
                let text_y = (height - font_size) / 2.0 + 2.0;
                stream.push_str(&format!("{} {} Td\n", padding, text_y));
                stream.push_str(&format!("({}) Tj\n", escape_pdf_string(txt)));
                stream.push_str("ET\n");
            }
        }

        // Restore state
        stream.push_str("Q\n");

        stream
    }

    /// Builds appearance stream for a list box.
    pub fn build_listbox_appearance(&self, options: &[String], selected_indices: &[usize], font_name: &str, font_size: f64, text_color: Color) -> String {
        let width = self.rect.width();
        let height = self.rect.height();
        let mut stream = String::new();

        // Save state
        stream.push_str("q\n");

        // Background
        if let Some(bg) = &self.background_color {
            stream.push_str(&format_color_fill(bg));
            stream.push_str(&format!("0 0 {} {} re f\n", width, height));
        }

        // Border
        if let Some(bc) = &self.border_color {
            stream.push_str(&format_color_stroke(bc));
            stream.push_str(&format!("{} w\n", self.border_width));
            let offset = self.border_width / 2.0;
            stream.push_str(&format!(
                "{} {} {} {} re S\n",
                offset, offset,
                width - self.border_width,
                height - self.border_width
            ));
        }

        // List items
        let line_height = font_size + 4.0;
        let padding = 4.0;
        let max_visible = (height / line_height).floor() as usize;

        stream.push_str("BT\n");
        stream.push_str(&format!("/{} {} Tf\n", font_name, font_size));

        for (i, option) in options.iter().take(max_visible).enumerate() {
            let y = height - padding - (i as f64 + 1.0) * line_height + 4.0;

            // Highlight selected items
            if selected_indices.contains(&i) {
                stream.push_str("ET\n");
                stream.push_str("0.2 0.4 0.8 rg\n");
                stream.push_str(&format!(
                    "{} {} {} {} re f\n",
                    self.border_width, y - 2.0,
                    width - self.border_width * 2.0, line_height
                ));
                stream.push_str("BT\n");
                stream.push_str(&format!("/{} {} Tf\n", font_name, font_size));
                stream.push_str("1 1 1 rg\n"); // White text on selection
            } else {
                stream.push_str(&format_color_fill(&text_color));
            }

            stream.push_str(&format!("{} {} Td\n", padding, y));
            stream.push_str(&format!("({}) Tj\n", escape_pdf_string(option)));
            stream.push_str(&format!("{} {} Td\n", -padding, -y)); // Reset position
        }

        stream.push_str("ET\n");

        // Restore state
        stream.push_str("Q\n");

        stream
    }
}

/// Formats a color for fill operations.
fn format_color_fill(color: &Color) -> String {
    match color {
        Color::Gray(g) => format!("{} g\n", g.level),
        Color::Rgb(rgb) => format!("{} {} {} rg\n", rgb.r, rgb.g, rgb.b),
        Color::Cmyk(cmyk) => format!("{} {} {} {} k\n", cmyk.c, cmyk.m, cmyk.y, cmyk.k),
    }
}

/// Formats a color for stroke operations.
fn format_color_stroke(color: &Color) -> String {
    match color {
        Color::Gray(g) => format!("{} G\n", g.level),
        Color::Rgb(rgb) => format!("{} {} {} RG\n", rgb.r, rgb.g, rgb.b),
        Color::Cmyk(cmyk) => format!("{} {} {} {} K\n", cmyk.c, cmyk.m, cmyk.y, cmyk.k),
    }
}

/// Draws a circle using Bezier curves.
fn draw_circle(cx: f64, cy: f64, r: f64) -> String {
    // Magic number for approximating a circle with Bezier curves
    let k = 0.5522847498;
    let kr = k * r;

    format!(
        "{} {} m\n\
         {} {} {} {} {} {} c\n\
         {} {} {} {} {} {} c\n\
         {} {} {} {} {} {} c\n\
         {} {} {} {} {} {} c\n",
        cx + r, cy,
        cx + r, cy + kr, cx + kr, cy + r, cx, cy + r,
        cx - kr, cy + r, cx - r, cy + kr, cx - r, cy,
        cx - r, cy - kr, cx - kr, cy - r, cx, cy - r,
        cx + kr, cy - r, cx + r, cy - kr, cx + r, cy
    )
}

/// Escapes a string for PDF.
fn escape_pdf_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_border_style_pdf_code() {
        assert_eq!(BorderStyle::Solid.pdf_code(), "S");
        assert_eq!(BorderStyle::Dashed.pdf_code(), "D");
        assert_eq!(BorderStyle::Beveled.pdf_code(), "B");
        assert_eq!(BorderStyle::Inset.pdf_code(), "I");
        assert_eq!(BorderStyle::Underline.pdf_code(), "U");
    }

    #[test]
    fn test_text_appearance_builder() {
        let rect = Rectangle::new(0.0, 0.0, 200.0, 20.0);
        let builder = AppearanceBuilder::new(rect)
            .background_color(Color::WHITE)
            .border_color(Color::BLACK);

        let stream = builder.build_text_appearance(Some("Hello"), "Helv", 12.0, Color::BLACK);
        assert!(stream.contains("(Hello)"));
        assert!(stream.contains("/Helv 12 Tf"));
    }

    #[test]
    fn test_checkbox_appearance() {
        let rect = Rectangle::new(0.0, 0.0, 20.0, 20.0);
        let builder = AppearanceBuilder::new(rect)
            .background_color(Color::WHITE)
            .border_color(Color::BLACK);

        let checked = builder.build_checkbox_checked(Color::BLACK);
        assert!(checked.contains("m\n")); // Contains move command
        assert!(checked.contains("l\n")); // Contains line command

        let unchecked = builder.build_checkbox_unchecked();
        assert!(unchecked.contains("re S")); // Contains rectangle stroke
    }

    #[test]
    fn test_button_appearance() {
        let rect = Rectangle::new(0.0, 0.0, 100.0, 30.0);
        let builder = AppearanceBuilder::new(rect)
            .background_color(Color::gray(0.9))
            .border_style(BorderStyle::Beveled);

        let stream = builder.build_button_appearance("Click Me", "Helv", 12.0, Color::BLACK);
        assert!(stream.contains("(Click Me)"));
    }
}
