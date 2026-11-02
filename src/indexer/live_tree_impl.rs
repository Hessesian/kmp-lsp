use std::sync::Arc;
use tower_lsp::lsp_types::Url;

use super::live_tree::{lang_for_path, parse_live, LiveDoc};
use super::Indexer;
use crate::types::FileData;

/// How many parsed unopened files `live_doc_or_parse` remembers.
const UNOPENED_PARSE_CAPACITY: usize = 32;

/// A tree parsed from one version of an indexed file.
pub(crate) struct UnopenedParse {
    uri: String,
    parsed_from: Arc<FileData>,
    doc: Arc<LiveDoc>,
}

impl Indexer {
    /// The tree of `uri`'s indexed `file_data`, parsed at most once per
    /// version for the most recent files.
    fn parse_unopened(&self, uri: &Url, file_data: Arc<FileData>) -> Option<Arc<LiveDoc>> {
        let remembered = self
            .unopened_parses
            .lock()
            .ok()?
            .iter()
            .find(|parse| parse.uri == uri.as_str() && Arc::ptr_eq(&parse.parsed_from, &file_data))
            .map(|parse| Arc::clone(&parse.doc));
        if remembered.is_some() {
            return remembered;
        }
        let lang = lang_for_path(uri.path())?;
        let doc = Arc::new(parse_live(&file_data.lines.join("\n"), lang)?);
        let mut parses = self.unopened_parses.lock().ok()?;
        parses.retain(|parse| parse.uri != uri.as_str());
        if parses.len() >= UNOPENED_PARSE_CAPACITY {
            parses.remove(0);
        }
        parses.push(UnopenedParse {
            uri: uri.to_string(),
            parsed_from: file_data,
            doc: Arc::clone(&doc),
        });
        Some(doc)
    }

    /// Parse `content` and store the resulting `LiveDoc` for `uri`.
    ///
    /// If the file extension is unsupported or parsing fails, any previously
    /// stored tree for `uri` is removed so consumers never read a stale doc.
    pub(crate) fn store_live_tree(&self, uri: &Url, content: &str) {
        let path = uri.path();
        match lang_for_path(path).and_then(|lang| parse_live(content, lang)) {
            Some(doc) => {
                self.live_trees.insert(uri.to_string(), Arc::new(doc));
            }
            None => {
                self.live_trees.remove(uri.as_str());
            }
        }
        // The new content invalidates any memoized brace-repair candidates —
        // they're derived from the old bytes and would silently answer every
        // repair attempt against stale text otherwise.
        self.repair_candidates.remove(uri.as_str());
    }

    /// Return the `LiveDoc` for `uri`, or `None` if the file is not open.
    pub(crate) fn live_doc(&self, uri: &Url) -> Option<Arc<LiveDoc>> {
        self.live_trees.get(uri.as_str()).map(|r| Arc::clone(&*r))
    }

    /// Return the `LiveDoc` for `uri`, parsing on-demand if not already cached.
    ///
    /// Used by CST-based queries (e.g. `enclosing_class_at`) that are called
    /// before `textDocument/didOpen` has been fully processed — the actor
    /// receives `did_open` asynchronously so the live tree may not exist yet
    /// when a navigation request arrives.  Content is taken from `live_lines`
    /// (if present), the indexed file, or disk (cold-start fallback); the
    /// result is stored into `live_trees` **only when the file is currently open**
    /// (i.e. `live_lines` is present) so that `live_trees` does not accumulate
    /// parse trees for files that were never opened by the editor.
    pub(crate) fn live_doc_or_parse(&self, uri: &Url) -> Option<Arc<LiveDoc>> {
        if let Some(doc) = self.live_doc(uri) {
            return Some(doc);
        }

        let is_open = self.live_lines.contains_key(uri.as_str());
        if !is_open {
            let indexed = self.files.get(uri.as_str()).map(|file| Arc::clone(&*file));
            if let Some(file_data) = indexed {
                return self.parse_unopened(uri, file_data);
            }
        }

        let content: String = if let Some(ll) = self.live_lines.get(uri.as_str()) {
            ll.join("\n")
        } else if let Ok(path) = uri.to_file_path() {
            std::fs::read_to_string(path).ok()?
        } else {
            return None;
        };

        let lang = lang_for_path(uri.path())?;
        let doc = parse_live(&content, lang)?;
        let doc = Arc::new(doc);
        if is_open {
            self.live_trees.insert(uri.to_string(), Arc::clone(&doc));
        }
        Some(doc)
    }

    /// Remove the live parse tree for `uri` (called on `textDocument/didClose`).
    pub(crate) fn remove_live_tree(&self, uri: &Url) {
        self.live_trees.remove(uri.as_str());
        self.repair_candidates.remove(uri.as_str());
    }
}
