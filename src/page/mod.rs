//! PDF Page handling.

use crate::content::ContentBuilder;
use crate::font::{Font, Standard14Font};
#[cfg(feature = "images")]
use crate::image::Image;
use crate::object::PdfStream;
use crate::types::Rectangle;

/// A PDF page.
#[derive(Debug)]
pub struct Page {
    /// The page dimensions (MediaBox).
    pub media_box: Rectangle,
    /// Font resources: (resource name, font).
    pub fonts: Vec<(String, Font)>,
    /// Image resources: (resource name, image).
    #[cfg(feature = "images")]
    pub images: Vec<(String, Image)>,
    /// The content stream operators.
    pub content: ContentBuilder,
}

impl Page {
    /// Creates a new page with the given dimensions.
    pub fn new(media_box: Rectangle) -> Self {
        Self {
            media_box,
            fonts: Vec::new(),
            #[cfg(feature = "images")]
            images: Vec::new(),
            content: ContentBuilder::new(),
        }
    }

    /// Creates an A4 page.
    pub fn a4() -> Self {
        Self::new(Rectangle::a4())
    }

    /// Creates a US Letter page.
    pub fn letter() -> Self {
        Self::new(Rectangle::letter())
    }

    /// Adds a font to the page resources.
    pub fn add_font(&mut self, name: impl Into<String>, font: Font) {
        self.fonts.push((name.into(), font));
    }

    /// Adds an image to the page resources.
    ///
    /// The image can be referenced in content streams using the given name
    /// with the paint_xobject operator.
    #[cfg(feature = "images")]
    pub fn add_image(&mut self, name: impl Into<String>, image: Image) {
        self.images.push((name.into(), image));
    }

    /// Sets the content builder for this page.
    pub fn set_content(&mut self, content: ContentBuilder) {
        self.content = content;
    }

    /// Returns the width of the page.
    pub fn width(&self) -> f64 {
        self.media_box.width()
    }

    /// Returns the height of the page.
    pub fn height(&self) -> f64 {
        self.media_box.height()
    }

    /// Builds the content stream for this page.
    pub fn build_content_stream(&self) -> PdfStream {
        PdfStream::from_text(self.content.build_string())
    }
}

impl Default for Page {
    fn default() -> Self {
        Self::a4()
    }
}

/// Builder for creating PDF pages.
#[derive(Debug, Default)]
pub struct PageBuilder {
    media_box: Rectangle,
    fonts: Vec<(String, Font)>,
    #[cfg(feature = "images")]
    images: Vec<(String, Image)>,
    content: Option<ContentBuilder>,
}

impl PageBuilder {
    /// Creates a new page builder with default settings (A4).
    pub fn new() -> Self {
        Self {
            media_box: Rectangle::a4(),
            fonts: Vec::new(),
            #[cfg(feature = "images")]
            images: Vec::new(),
            content: None,
        }
    }

    /// Creates a page builder for an A4 page.
    pub fn a4() -> Self {
        Self::new().media_box(Rectangle::a4())
    }

    /// Creates a page builder for a US Letter page.
    pub fn letter() -> Self {
        Self::new().media_box(Rectangle::letter())
    }

    /// Creates a page builder for an A3 page.
    pub fn a3() -> Self {
        Self::new().media_box(Rectangle::a3())
    }

    /// Creates a page builder for an A5 page.
    pub fn a5() -> Self {
        Self::new().media_box(Rectangle::a5())
    }

    /// Creates a page builder for a US Legal page.
    pub fn legal() -> Self {
        Self::new().media_box(Rectangle::legal())
    }

    /// Creates a page builder with custom dimensions (in points).
    pub fn custom(width: f64, height: f64) -> Self {
        Self::new().media_box(Rectangle::from_dimensions(width, height))
    }

    /// Sets the media box (page dimensions).
    pub fn media_box(mut self, rect: Rectangle) -> Self {
        self.media_box = rect;
        self
    }

    /// Sets the width of the page.
    pub fn width(mut self, width: f64) -> Self {
        self.media_box.urx = self.media_box.llx + width;
        self
    }

    /// Sets the height of the page.
    pub fn height(mut self, height: f64) -> Self {
        self.media_box.ury = self.media_box.lly + height;
        self
    }

    /// Adds a font with a resource name.
    pub fn font(mut self, name: impl Into<String>, font: impl Into<Font>) -> Self {
        self.fonts.push((name.into(), font.into()));
        self
    }

    /// Adds a standard 14 font.
    pub fn standard_font(self, name: impl Into<String>, font: Standard14Font) -> Self {
        self.font(name, Font::from(font))
    }

    /// Adds Helvetica as font F1.
    pub fn helvetica(self) -> Self {
        self.standard_font("F1", Standard14Font::Helvetica)
    }

    /// Adds Times Roman as font F1.
    pub fn times(self) -> Self {
        self.standard_font("F1", Standard14Font::TimesRoman)
    }

    /// Adds Courier as font F1.
    pub fn courier(self) -> Self {
        self.standard_font("F1", Standard14Font::Courier)
    }

    /// Adds an image with a resource name.
    ///
    /// The image can be drawn in content streams using `paint_xobject(name)`.
    #[cfg(feature = "images")]
    pub fn image(mut self, name: impl Into<String>, image: Image) -> Self {
        self.images.push((name.into(), image));
        self
    }

    /// Sets the content for the page.
    pub fn content(mut self, content: ContentBuilder) -> Self {
        self.content = Some(content);
        self
    }

    /// Builds the page.
    pub fn build(self) -> Page {
        Page {
            media_box: self.media_box,
            fonts: self.fonts,
            #[cfg(feature = "images")]
            images: self.images,
            content: self.content.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    #[test]
    fn test_page_a4() {
        let page = PageBuilder::a4().build();
        assert_eq!(page.width(), 595.0);
        assert_eq!(page.height(), 842.0);
    }

    #[test]
    fn test_page_letter() {
        let page = PageBuilder::letter().build();
        assert_eq!(page.width(), 612.0);
        assert_eq!(page.height(), 792.0);
    }

    #[test]
    fn test_page_with_font() {
        let page = PageBuilder::a4()
            .font("F1", Font::helvetica())
            .build();

        assert_eq!(page.fonts.len(), 1);
        assert_eq!(page.fonts[0].0, "F1");
    }

    #[test]
    fn test_page_with_content() {
        let content = ContentBuilder::new()
            .fill_color(Color::RED)
            .rect(100.0, 100.0, 200.0, 150.0)
            .fill();

        let page = PageBuilder::a4()
            .content(content)
            .build();

        let stream = page.build_content_stream();
        assert!(!stream.is_empty());
    }

    #[test]
    fn test_page_custom_size() {
        let page = PageBuilder::custom(400.0, 600.0).build();
        assert_eq!(page.width(), 400.0);
        assert_eq!(page.height(), 600.0);
    }

    #[test]
    fn test_page_builder_shortcuts() {
        let page = PageBuilder::a4().helvetica().build();
        assert_eq!(page.fonts.len(), 1);
        assert_eq!(page.fonts[0].1.postscript_name(), "Helvetica");
    }
}
