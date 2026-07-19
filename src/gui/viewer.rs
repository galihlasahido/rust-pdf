//! Page raster -> GPU texture cache for the PDF viewer, with a small LRU of
//! recently-viewed pages -- so paging back and forth doesn't re-render a
//! page that's already been seen, the way a native PDF viewer's own page
//! cache would -- plus prefetching of neighboring pages (see
//! [`PageViewer::prefetch`], driven from `app.rs`) so the next/previous
//! page is often already rendered by the time the user asks for it.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Arc;

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

use crate::error::RenderError;
use crate::render::{PdfRenderer, RgbaImage};

use super::actions;

type PageKey = (usize, u32); // (page_index, dpi.to_bits())

/// How many rendered pages to keep cached at once. Bounds memory -- each
/// entry is a full-resolution RGBA texture -- while comfortably covering
/// "flip back and forth a few pages" without re-rendering.
const CACHE_CAPACITY: usize = 12;

#[derive(Default)]
pub struct PageViewer {
    /// Least-recently-used at the front, most-recently-used at the back.
    cache: Vec<(PageKey, TextureHandle)>,
    pending: HashMap<PageKey, mpsc::Receiver<Result<RgbaImage, RenderError>>>,
    current_key: Option<PageKey>,
    /// The error from the current page's most recent failed render, if any.
    pub error: Option<String>,
}

impl PageViewer {
    /// Marks `page_index`/`dpi` as the page to display, kicking off a
    /// background render if it's not already cached or in flight. Cheap to
    /// call every frame.
    pub fn show(&mut self, renderer: &Arc<PdfRenderer>, page_index: usize, dpi: f32) {
        let key = (page_index, dpi.to_bits());
        self.current_key = Some(key);
        self.request(renderer, key);
    }

    /// Kicks off a background render for `page_index`/`dpi` without making
    /// it the currently-displayed page. Used to warm the cache for the
    /// page(s) next to the one currently shown.
    pub fn prefetch(&mut self, renderer: &Arc<PdfRenderer>, page_index: usize, dpi: f32) {
        self.request(renderer, (page_index, dpi.to_bits()));
    }

    fn request(&mut self, renderer: &Arc<PdfRenderer>, key: PageKey) {
        if self.cache_position(key).is_some() || self.pending.contains_key(&key) {
            return;
        }
        let (page_index, dpi_bits) = key;
        let dpi = f32::from_bits(dpi_bits);
        let renderer = Arc::clone(renderer);
        let rx = actions::spawn(move || renderer.render_page(page_index, dpi, None));
        self.pending.insert(key, rx);
    }

    fn cache_position(&self, key: PageKey) -> Option<usize> {
        self.cache.iter().position(|(k, _)| *k == key)
    }

    /// Polls all in-flight renders and uploads any that finished. Returns
    /// `true` if at least one is still pending (caller should request a
    /// repaint so results are picked up promptly).
    pub fn poll(&mut self, ctx: &Context) -> bool {
        let in_flight: Vec<PageKey> = self.pending.keys().copied().collect();
        let mut still_pending = false;

        for key in in_flight {
            let Some(rx) = self.pending.get(&key) else {
                continue;
            };
            match rx.try_recv() {
                Ok(Ok(image)) => {
                    let size = [image.width() as usize, image.height() as usize];
                    let color_image = ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                    let texture = ctx.load_texture(
                        format!("pdf-page-{}-{}", key.0, key.1),
                        color_image,
                        TextureOptions::LINEAR,
                    );
                    self.insert_cache(key, texture);
                    self.pending.remove(&key);
                    if self.current_key == Some(key) {
                        self.error = None;
                    }
                }
                Ok(Err(err)) => {
                    if self.current_key == Some(key) {
                        self.error = Some(err.to_string());
                    }
                    self.pending.remove(&key);
                }
                Err(mpsc::TryRecvError::Empty) => still_pending = true,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.current_key == Some(key) {
                        self.error = Some("render thread ended unexpectedly".to_string());
                    }
                    self.pending.remove(&key);
                }
            }
        }

        still_pending
    }

    fn insert_cache(&mut self, key: PageKey, texture: TextureHandle) {
        if let Some(pos) = self.cache_position(key) {
            let _ = self.cache.remove(pos);
        }
        self.cache.push((key, texture));
        if self.cache.len() > CACHE_CAPACITY {
            let _ = self.cache.remove(0);
        }
    }

    /// The texture for the currently-shown page, if it's already rendered
    /// and cached. Refreshes its position in the LRU order.
    pub fn texture(&mut self) -> Option<&TextureHandle> {
        let key = self.current_key?;
        let pos = self.cache_position(key)?;
        let entry = self.cache.remove(pos);
        self.cache.push(entry);
        self.cache.last().map(|(_, texture)| texture)
    }

    /// Whether the *currently shown* page (not a background prefetch) is
    /// still rendering.
    pub fn is_loading(&self) -> bool {
        self.current_key
            .is_some_and(|key| self.pending.contains_key(&key))
    }

    /// Clears everything cached/in flight -- call when switching to a
    /// different document.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn open_fixture() -> Arc<PdfRenderer> {
        Arc::new(
            PdfRenderer::open_file("tests/output/multipage_report.pdf")
                .expect("fixture should open"),
        )
    }

    /// Polls until nothing is pending or a deadline passes, so the test
    /// doesn't hang if a background render never completes.
    fn poll_until_idle(viewer: &mut PageViewer, ctx: &Context) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while viewer.poll(ctx) {
            assert!(Instant::now() < deadline, "render did not finish in time");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn revisiting_a_cached_page_does_not_re_render() {
        let renderer = open_fixture();
        let ctx = Context::default();
        let mut viewer = PageViewer::default();

        viewer.show(&renderer, 0, 96.0);
        assert!(viewer.is_loading());
        poll_until_idle(&mut viewer, &ctx);
        assert!(viewer.texture().is_some());

        // Navigate away, then back -- should be a pure cache hit, no new
        // pending render, exactly the bug this cache exists to fix.
        viewer.show(&renderer, 1, 96.0);
        assert!(viewer.is_loading(), "page 1 should need a fresh render");
        poll_until_idle(&mut viewer, &ctx);

        viewer.show(&renderer, 0, 96.0);
        assert!(
            !viewer.is_loading(),
            "revisiting page 0 should hit the cache, not re-render"
        );
        assert!(viewer.texture().is_some());
    }

    #[test]
    fn prefetch_populates_cache_without_becoming_current_page() {
        let renderer = open_fixture();
        let ctx = Context::default();
        let mut viewer = PageViewer::default();

        viewer.show(&renderer, 0, 96.0);
        viewer.prefetch(&renderer, 1, 96.0);
        poll_until_idle(&mut viewer, &ctx);

        assert!(viewer.texture().is_some(), "current page 0 should render");
        // Now switch to the prefetched page -- must be instant (cached).
        viewer.show(&renderer, 1, 96.0);
        assert!(
            !viewer.is_loading(),
            "prefetched page 1 should already be cached"
        );
    }
}
