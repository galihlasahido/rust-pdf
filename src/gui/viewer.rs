//! Page raster -> GPU texture cache for the PDF viewer, so panning/repaints
//! don't re-rasterize a page that's already on screen at the same DPI.

use std::sync::mpsc;
use std::sync::Arc;

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

use crate::error::RenderError;
use crate::render::{PdfRenderer, RgbaImage};

use super::actions;

/// The currently displayed page's texture (if any), plus an in-flight
/// background render (if any).
#[derive(Default)]
pub struct PageViewer {
    texture: Option<TextureHandle>,
    texture_key: Option<(usize, u32)>,
    pending: Option<PendingRender>,
    /// The error from the most recent failed render, if any.
    pub error: Option<String>,
}

struct PendingRender {
    key: (usize, u32),
    rx: mpsc::Receiver<Result<RgbaImage, RenderError>>,
}

impl PageViewer {
    /// Ensures `page_index` at `dpi` is rendered (or already is), kicking
    /// off a background render if it's not already displayed or in flight.
    /// Cheap to call every frame.
    pub fn request(&mut self, renderer: &Arc<PdfRenderer>, page_index: usize, dpi: f32) {
        let key = (page_index, dpi.to_bits());
        if self.texture_key == Some(key) {
            return;
        }
        if let Some(pending) = &self.pending {
            if pending.key == key {
                return;
            }
        }

        let renderer = Arc::clone(renderer);
        let rx = actions::spawn(move || renderer.render_page(page_index, dpi, None));
        self.pending = Some(PendingRender { key, rx });
    }

    /// Polls for a completed background render and uploads it as a texture.
    /// Returns `true` if a render is still pending (the caller should
    /// request a repaint so the result gets picked up promptly).
    pub fn poll(&mut self, ctx: &Context) -> bool {
        let Some(pending) = &self.pending else {
            return false;
        };

        match pending.rx.try_recv() {
            Ok(Ok(image)) => {
                let size = [image.width() as usize, image.height() as usize];
                let color_image = ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                let texture = ctx.load_texture("pdf-page", color_image, TextureOptions::LINEAR);
                self.texture_key = Some(pending.key);
                self.texture = Some(texture);
                self.error = None;
                self.pending = None;
                false
            }
            Ok(Err(err)) => {
                self.error = Some(err.to_string());
                self.pending = None;
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.error = Some("render thread ended unexpectedly".to_string());
                self.pending = None;
                false
            }
        }
    }

    pub fn texture(&self) -> Option<&TextureHandle> {
        self.texture.as_ref()
    }

    pub fn is_loading(&self) -> bool {
        self.pending.is_some()
    }

    /// Clears any cached texture/pending job -- call when switching to a
    /// different document.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
