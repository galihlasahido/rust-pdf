//! A small bounded LRU cache for rendered page thumbnails.
//!
//! Re-rendering a page thumbnail on every redraw of a page-list/grid UI (as
//! a Tauri desktop viewer would do while scrolling) is wasteful: Pdfium's
//! rendering cost dominates redraw latency for the typical "many small
//! thumbnails" access pattern. This cache keeps the most recently used
//! `max_entries` thumbnails (keyed by `(page_index, max_dimension)`, since a
//! caller may request different thumbnail sizes for the same page — e.g. a
//! sidebar strip vs. a grid view) in memory, evicting the least-recently
//! used entry once full.
//!
//! This is intentionally a minimal, dependency-light LRU built on
//! [`indexmap::IndexMap`] (already a dependency of this crate for
//! order-preserving PDF dictionaries) rather than pulling in a dedicated
//! LRU crate: `IndexMap` preserves insertion order, so "least recently
//! used" is simply "the front of the map", and promoting an entry to
//! "most recently used" is a remove-then-reinsert at the back.

use crate::render::RgbaImage;
use indexmap::IndexMap;

/// Cache key: the page index together with the requested thumbnail's
/// maximum dimension, since the same page may be rendered at multiple
/// thumbnail sizes.
pub(crate) type ThumbnailKey = (usize, u32);

/// Bounded, `!Sync` LRU cache mapping [`ThumbnailKey`] -> rendered
/// thumbnail.
///
/// Callers needing concurrent access (e.g. from multiple Tauri command
/// invocations running on different threads) should wrap this in a
/// `Mutex`; [`crate::render::PdfRenderer`] does so internally.
#[derive(Debug)]
pub(crate) struct ThumbnailCache {
    entries: IndexMap<ThumbnailKey, RgbaImage>,
    max_entries: usize,
}

impl ThumbnailCache {
    /// Creates a new cache holding at most `max_entries` thumbnails.
    ///
    /// `max_entries` is clamped to at least `1` so the cache is always
    /// able to hold the most recently requested thumbnail, even if a
    /// caller mistakenly passes `0`.
    pub(crate) fn new(max_entries: usize) -> Self {
        Self {
            entries: IndexMap::with_capacity(max_entries.clamp(1, 256)),
            max_entries: max_entries.max(1),
        }
    }

    /// Returns a clone of the cached thumbnail for `key`, if present, and
    /// marks it as most-recently-used. Returns `None` on a cache miss.
    pub(crate) fn get(&mut self, key: ThumbnailKey) -> Option<RgbaImage> {
        let image = self.entries.shift_remove(&key)?;
        self.entries.insert(key, image.clone());
        Some(image)
    }

    /// Inserts a freshly rendered thumbnail, evicting the least-recently
    /// used entry first if the cache is already at capacity.
    pub(crate) fn insert(&mut self, key: ThumbnailKey, image: RgbaImage) {
        // If `key` is already present, remove-then-reinsert below both
        // updates its value and promotes it to most-recently-used, so no
        // separate eviction is needed for that case.
        if self.entries.len() >= self.max_entries && !self.entries.contains_key(&key) {
            // `IndexMap` preserves insertion/access order (see `get`
            // above, which re-inserts at the back on access), so index 0
            // is always the least-recently-used entry.
            self.entries.shift_remove_index(0);
        } else {
            self.entries.shift_remove(&key);
        }
        self.entries.insert(key, image);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_image(fill: u8) -> RgbaImage {
        RgbaImage::from_pixel(2, 2, image::Rgba([fill, fill, fill, 255]))
    }

    #[test]
    fn miss_on_empty_cache() {
        let mut cache = ThumbnailCache::new(4);
        assert!(cache.get((0, 128)).is_none());
    }

    #[test]
    fn insert_then_get_round_trips() {
        let mut cache = ThumbnailCache::new(4);
        cache.insert((0, 128), dummy_image(10));
        let got = cache.get((0, 128)).expect("cache hit");
        assert_eq!(got.get_pixel(0, 0).0, [10, 10, 10, 255]);
    }

    #[test]
    fn distinguishes_same_page_different_size() {
        let mut cache = ThumbnailCache::new(4);
        cache.insert((0, 128), dummy_image(1));
        cache.insert((0, 256), dummy_image(2));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get((0, 128)).unwrap().get_pixel(0, 0).0[0], 1);
        assert_eq!(cache.get((0, 256)).unwrap().get_pixel(0, 0).0[0], 2);
    }

    #[test]
    fn evicts_least_recently_used_when_full() {
        let mut cache = ThumbnailCache::new(2);
        cache.insert((0, 128), dummy_image(1));
        cache.insert((1, 128), dummy_image(2));
        // Touch page 0 so page 1 becomes the least-recently-used entry.
        assert!(cache.get((0, 128)).is_some());
        cache.insert((2, 128), dummy_image(3));

        assert_eq!(cache.len(), 2);
        assert!(cache.get((1, 128)).is_none(), "page 1 should be evicted");
        assert!(cache.get((0, 128)).is_some(), "page 0 should survive");
        assert!(cache.get((2, 128)).is_some(), "page 2 should survive");
    }

    #[test]
    fn zero_capacity_is_clamped_to_one() {
        let mut cache = ThumbnailCache::new(0);
        cache.insert((0, 128), dummy_image(1));
        assert_eq!(cache.len(), 1);
        cache.insert((1, 128), dummy_image(2));
        assert_eq!(cache.len(), 1, "capacity must be clamped to at least 1");
    }

    #[test]
    fn reinserting_existing_key_does_not_grow_or_duplicate() {
        let mut cache = ThumbnailCache::new(3);
        cache.insert((0, 128), dummy_image(1));
        cache.insert((0, 128), dummy_image(9));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get((0, 128)).unwrap().get_pixel(0, 0).0[0], 9);
    }
}
