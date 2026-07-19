//! Per-page thumbnail texture cache for the thumbnail sidebar. Unlike
//! [`super::viewer::PageViewer`] (one texture, the current page) this holds
//! many small textures at once, requested lazily as rows scroll into view
//! (see `Panel::show_rows` in `app.rs`) rather than all up front -- a
//! large document shouldn't spawn hundreds of render threads on load.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

use crate::render::PdfRenderer;

use super::actions;

/// Longest side of a thumbnail, in pixels.
pub const MAX_DIMENSION: u32 = 120;

/// Caps concurrent background thumbnail renders. Without this, scrolling
/// the strip quickly can bring many rows into view in one frame and spawn
/// a render thread for each of them at once; on a CPU-bound renderer
/// they'd all contend for the same cores and every one of them (including
/// the ones the user is actually looking at) gets slower, not faster, by
/// running "in parallel".
const MAX_CONCURRENT: usize = 2;

#[derive(Default)]
pub struct ThumbnailStrip {
    textures: HashMap<usize, TextureHandle>,
    pending:
        HashMap<usize, mpsc::Receiver<Result<crate::render::RgbaImage, crate::error::RenderError>>>,
    in_flight: Arc<AtomicUsize>,
}

impl ThumbnailStrip {
    /// Kicks off a background render for `page_index` if it's not already
    /// cached or in flight, and a concurrency slot is free (otherwise this
    /// is a no-op -- the caller re-requests every frame for visible rows,
    /// so it's retried automatically once a slot opens up). Cheap to call
    /// every frame for visible rows.
    pub fn request(&mut self, renderer: &Arc<PdfRenderer>, page_index: usize) {
        if self.textures.contains_key(&page_index) || self.pending.contains_key(&page_index) {
            return;
        }
        if self.in_flight.load(Ordering::SeqCst) >= MAX_CONCURRENT {
            return;
        }
        self.in_flight.fetch_add(1, Ordering::SeqCst);

        let renderer = Arc::clone(renderer);
        let in_flight = Arc::clone(&self.in_flight);
        let rx = actions::spawn(move || {
            let result = renderer.render_thumbnail(page_index, MAX_DIMENSION);
            in_flight.fetch_sub(1, Ordering::SeqCst);
            result
        });
        self.pending.insert(page_index, rx);
    }

    /// Polls all in-flight renders and uploads any that finished. Returns
    /// `true` if at least one is still pending (caller should request a
    /// repaint).
    pub fn poll(&mut self, ctx: &Context) -> bool {
        let in_flight: Vec<usize> = self.pending.keys().copied().collect();
        let mut still_pending = false;

        for page_index in in_flight {
            let Some(rx) = self.pending.get(&page_index) else {
                continue;
            };
            match rx.try_recv() {
                Ok(Ok(image)) => {
                    let size = [image.width() as usize, image.height() as usize];
                    let color_image = ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                    let texture = ctx.load_texture(
                        format!("thumb-{page_index}"),
                        color_image,
                        TextureOptions::LINEAR,
                    );
                    self.textures.insert(page_index, texture);
                    self.pending.remove(&page_index);
                }
                Ok(Err(_)) => {
                    // A single bad thumbnail (e.g. a pathological page)
                    // shouldn't break the rest of the strip -- just leave
                    // that slot blank.
                    self.pending.remove(&page_index);
                }
                Err(mpsc::TryRecvError::Empty) => still_pending = true,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending.remove(&page_index);
                }
            }
        }

        still_pending
    }

    pub fn texture(&self, page_index: usize) -> Option<&TextureHandle> {
        self.textures.get(&page_index)
    }

    /// Clears everything cached/in flight -- call when switching documents.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
