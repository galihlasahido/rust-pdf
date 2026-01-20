//! Image handling for PDF documents.
//!
//! This module provides support for embedding JPEG and PNG images in PDF documents
//! as XObject resources.

mod xobject;

pub use xobject::ImageXObject;

use crate::error::ImageError;
use std::path::Path;

/// Supported image color spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    /// Grayscale (1 component)
    DeviceGray,
    /// RGB (3 components)
    DeviceRGB,
    /// CMYK (4 components)
    DeviceCMYK,
}

impl ColorSpace {
    /// Returns the PDF name for this color space.
    pub fn as_pdf_name(&self) -> &'static str {
        match self {
            ColorSpace::DeviceGray => "DeviceGray",
            ColorSpace::DeviceRGB => "DeviceRGB",
            ColorSpace::DeviceCMYK => "DeviceCMYK",
        }
    }

    /// Returns the number of color components.
    pub fn components(&self) -> u8 {
        match self {
            ColorSpace::DeviceGray => 1,
            ColorSpace::DeviceRGB => 3,
            ColorSpace::DeviceCMYK => 4,
        }
    }
}

/// The compression filter used for an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFilter {
    /// DCT (JPEG) compression
    DCTDecode,
    /// Flate (zlib) compression
    FlateDecode,
}

impl ImageFilter {
    /// Returns the PDF name for this filter.
    pub fn as_pdf_name(&self) -> &'static str {
        match self {
            ImageFilter::DCTDecode => "DCTDecode",
            ImageFilter::FlateDecode => "FlateDecode",
        }
    }
}

/// A raster image that can be embedded in a PDF.
#[derive(Debug, Clone)]
pub struct Image {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Color space.
    pub color_space: ColorSpace,
    /// Bits per component (typically 8).
    pub bits_per_component: u8,
    /// The compression filter.
    pub filter: ImageFilter,
    /// The raw image data (compressed).
    pub data: Vec<u8>,
    /// Optional soft mask (alpha channel) for PNG with transparency.
    pub soft_mask: Option<Box<Image>>,
}

impl Image {
    /// Creates a new image from raw components.
    pub fn new(
        width: u32,
        height: u32,
        color_space: ColorSpace,
        bits_per_component: u8,
        filter: ImageFilter,
        data: Vec<u8>,
    ) -> Self {
        Self {
            width,
            height,
            color_space,
            bits_per_component,
            filter,
            data,
            soft_mask: None,
        }
    }

    /// Loads an image from a file.
    ///
    /// Supports JPEG and PNG formats.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ImageError> {
        let path = path.as_ref();
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase());

        match extension.as_deref() {
            Some("jpg") | Some("jpeg") => Self::from_jpeg_file(path),
            Some("png") => Self::from_png_file(path),
            Some(ext) => Err(ImageError::UnsupportedFormat(ext.to_string())),
            None => Err(ImageError::UnsupportedFormat("unknown".to_string())),
        }
    }

    /// Loads an image from bytes with auto-detection.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ImageError> {
        // Try to detect format from magic bytes
        if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
            Self::from_jpeg_bytes(bytes)
        } else if bytes.len() >= 8
            && bytes[0..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        {
            Self::from_png_bytes(bytes)
        } else {
            Err(ImageError::UnsupportedFormat(
                "unable to detect format".to_string(),
            ))
        }
    }

    /// Loads a JPEG image from a file.
    pub fn from_jpeg_file(path: impl AsRef<Path>) -> Result<Self, ImageError> {
        let bytes =
            std::fs::read(path).map_err(|e| ImageError::LoadFailed(e.to_string()))?;
        Self::from_jpeg_bytes(&bytes)
    }

    /// Loads a JPEG image from bytes.
    ///
    /// JPEG images can be embedded directly in PDF using DCTDecode filter,
    /// so we just need to parse the header to get dimensions.
    pub fn from_jpeg_bytes(bytes: &[u8]) -> Result<Self, ImageError> {
        use image::ImageReader;
        use std::io::Cursor;

        let reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| ImageError::LoadFailed(e.to_string()))?;

        let img = reader
            .decode()
            .map_err(|e| ImageError::DecodeFailed(e.to_string()))?;

        let width = img.width();
        let height = img.height();

        let color_space = match img.color() {
            image::ColorType::L8 | image::ColorType::L16 | image::ColorType::La8 | image::ColorType::La16 => {
                ColorSpace::DeviceGray
            }
            _ => ColorSpace::DeviceRGB,
        };

        // For JPEG, we embed the original bytes directly
        Ok(Self {
            width,
            height,
            color_space,
            bits_per_component: 8,
            filter: ImageFilter::DCTDecode,
            data: bytes.to_vec(),
            soft_mask: None,
        })
    }

    /// Loads a PNG image from a file.
    pub fn from_png_file(path: impl AsRef<Path>) -> Result<Self, ImageError> {
        let bytes =
            std::fs::read(path).map_err(|e| ImageError::LoadFailed(e.to_string()))?;
        Self::from_png_bytes(&bytes)
    }

    /// Loads a PNG image from bytes.
    ///
    /// PNG images are decoded and re-encoded using FlateDecode filter.
    /// Alpha channels are separated into soft masks.
    pub fn from_png_bytes(bytes: &[u8]) -> Result<Self, ImageError> {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use image::ImageReader;
        use std::io::{Cursor, Write};

        let reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| ImageError::LoadFailed(e.to_string()))?;

        let img = reader
            .decode()
            .map_err(|e| ImageError::DecodeFailed(e.to_string()))?;

        let width = img.width();
        let height = img.height();

        // Determine color space and handle alpha
        let (color_space, raw_data, alpha_data) = match img.color() {
            image::ColorType::L8 | image::ColorType::L16 => {
                let gray = img.to_luma8();
                (ColorSpace::DeviceGray, gray.into_raw(), None)
            }
            image::ColorType::La8 | image::ColorType::La16 => {
                let gray_alpha = img.to_luma_alpha8();
                let pixels = gray_alpha.into_raw();
                let mut gray = Vec::with_capacity(pixels.len() / 2);
                let mut alpha = Vec::with_capacity(pixels.len() / 2);
                for chunk in pixels.chunks(2) {
                    gray.push(chunk[0]);
                    alpha.push(chunk[1]);
                }
                (ColorSpace::DeviceGray, gray, Some(alpha))
            }
            image::ColorType::Rgb8 | image::ColorType::Rgb16 | image::ColorType::Rgb32F => {
                let rgb = img.to_rgb8();
                (ColorSpace::DeviceRGB, rgb.into_raw(), None)
            }
            image::ColorType::Rgba8 | image::ColorType::Rgba16 | image::ColorType::Rgba32F => {
                let rgba = img.to_rgba8();
                let pixels = rgba.into_raw();
                let mut rgb = Vec::with_capacity((pixels.len() / 4) * 3);
                let mut alpha = Vec::with_capacity(pixels.len() / 4);
                for chunk in pixels.chunks(4) {
                    rgb.push(chunk[0]);
                    rgb.push(chunk[1]);
                    rgb.push(chunk[2]);
                    alpha.push(chunk[3]);
                }
                (ColorSpace::DeviceRGB, rgb, Some(alpha))
            }
            other => {
                return Err(ImageError::UnsupportedFormat(format!("{:?}", other)));
            }
        };

        // Compress the raw image data
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&raw_data)
            .map_err(|e| ImageError::EncodeFailed(e.to_string()))?;
        let compressed_data = encoder
            .finish()
            .map_err(|e| ImageError::EncodeFailed(e.to_string()))?;

        // Create soft mask if we have alpha
        let soft_mask = if let Some(alpha) = alpha_data {
            let mut alpha_encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            alpha_encoder
                .write_all(&alpha)
                .map_err(|e| ImageError::EncodeFailed(e.to_string()))?;
            let compressed_alpha = alpha_encoder
                .finish()
                .map_err(|e| ImageError::EncodeFailed(e.to_string()))?;

            Some(Box::new(Image {
                width,
                height,
                color_space: ColorSpace::DeviceGray,
                bits_per_component: 8,
                filter: ImageFilter::FlateDecode,
                data: compressed_alpha,
                soft_mask: None,
            }))
        } else {
            None
        };

        Ok(Self {
            width,
            height,
            color_space,
            bits_per_component: 8,
            filter: ImageFilter::FlateDecode,
            data: compressed_data,
            soft_mask,
        })
    }

    /// Returns true if this image has an alpha channel (soft mask).
    pub fn has_alpha(&self) -> bool {
        self.soft_mask.is_some()
    }

    /// Returns the aspect ratio (width / height).
    pub fn aspect_ratio(&self) -> f64 {
        self.width as f64 / self.height as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_space() {
        assert_eq!(ColorSpace::DeviceGray.components(), 1);
        assert_eq!(ColorSpace::DeviceRGB.components(), 3);
        assert_eq!(ColorSpace::DeviceCMYK.components(), 4);
    }

    #[test]
    fn test_filter_names() {
        assert_eq!(ImageFilter::DCTDecode.as_pdf_name(), "DCTDecode");
        assert_eq!(ImageFilter::FlateDecode.as_pdf_name(), "FlateDecode");
    }
}
