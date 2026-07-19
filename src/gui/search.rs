//! Full-document text search: extracts each page's text on demand
//! (`EditableDocument::extract_page_text`, already Tauri-free) and does a
//! plain case-insensitive substring match, off the UI thread.

use std::sync::mpsc;
use std::sync::Arc;

use crate::render::PdfRenderer;

use super::actions;

const SNIPPET_RADIUS: usize = 40;
/// Caps result count so a pathologically repetitive query (e.g. a single
/// common letter) can't produce an unbounded results list.
const MAX_RESULTS: usize = 200;

#[derive(Clone)]
pub struct SearchMatch {
    pub page_index: usize,
    pub snippet: String,
}

#[derive(Default)]
pub struct SearchState {
    pub query: String,
    results: Vec<SearchMatch>,
    pending: Option<mpsc::Receiver<Vec<SearchMatch>>>,
}

impl SearchState {
    /// Starts a background search over every page for the current `query`.
    /// An empty query just clears any existing results.
    pub fn run(&mut self, renderer: &Arc<PdfRenderer>, page_count: usize) {
        let query = self.query.trim().to_string();
        if query.is_empty() {
            self.results.clear();
            self.pending = None;
            return;
        }
        let renderer = Arc::clone(renderer);
        let rx = actions::spawn(move || search_all_pages(&renderer, page_count, &query));
        self.pending = Some(rx);
    }

    /// Polls for a finished search. Returns `true` if one is still running
    /// (caller should request a repaint).
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.pending else {
            return false;
        };
        match rx.try_recv() {
            Ok(results) => {
                self.results = results;
                self.pending = None;
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                false
            }
        }
    }

    pub fn results(&self) -> &[SearchMatch] {
        &self.results
    }

    pub fn is_searching(&self) -> bool {
        self.pending.is_some()
    }

    /// Clears query/results/in-flight search -- call when switching
    /// documents.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

fn search_all_pages(renderer: &PdfRenderer, page_count: usize, query: &str) -> Vec<SearchMatch> {
    let needle = query.to_lowercase();
    let document = renderer.document();
    let mut matches = Vec::new();

    for page_index in 0..page_count {
        let Ok(page_id) = document.page_id_at(page_index) else {
            continue;
        };
        let Ok(text) = document.extract_page_text(page_id) else {
            continue;
        };
        let lower = text.to_lowercase();

        let mut search_from = 0;
        while let Some(offset) = lower[search_from..].find(&needle) {
            let match_start = search_from + offset;
            matches.push(SearchMatch {
                page_index,
                snippet: snippet_around(&text, match_start, needle.len()),
            });
            if matches.len() >= MAX_RESULTS {
                return matches;
            }
            search_from = match_start + needle.len().max(1);
        }
    }

    matches
}

fn snippet_around(text: &str, byte_pos: usize, match_len: usize) -> String {
    let start = floor_char_boundary(text, byte_pos.saturating_sub(SNIPPET_RADIUS));
    let end = ceil_char_boundary(
        text,
        (byte_pos + match_len + SNIPPET_RADIUS).min(text.len()),
    );

    let mut snippet = text[start..end].replace('\n', " ");
    if start > 0 {
        snippet.insert(0, '…');
    }
    if end < text.len() {
        snippet.push('…');
    }
    snippet
}

fn floor_char_boundary(text: &str, mut i: usize) -> usize {
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(text: &str, mut i: usize) -> usize {
    while i < text.len() && !text.is_char_boundary(i) {
        i += 1;
    }
    i
}
