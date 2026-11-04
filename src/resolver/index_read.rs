//! `IndexRead` for the concrete `Indexer`. It lives with the resolver because
//! reading the index on a caller's behalf runs name resolution (symbol
//! lookup, on-demand JAR materialisation); the index itself stays below it.
use std::sync::Arc;

use tower_lsp::lsp_types::{Location, Url};

use super::infer::infer_variable_type;
use crate::indexer::resolution::IndexRead;
use crate::indexer::Indexer;
use crate::types::FileData;

impl IndexRead for Indexer {
    fn get_definitions(&self, name: &str) -> Option<Vec<Location>> {
        // Reconstitute each interned `SymbolLoc` into a `Location` at this boundary.
        let mut locs: Vec<Location> = self
            .definitions
            .get(name)
            .map(|locations_ref| {
                locations_ref
                    .iter()
                    .filter_map(|symbol_location| self.file_table.location(*symbol_location))
                    .collect()
            })
            .unwrap_or_default();
        let mut unbudgeted = usize::MAX;
        crate::indexer::jar::ensure_jar_definitions_for(self, name, &mut unbudgeted);
        if let Some(jar_locs) = self.jar_definitions.get(name) {
            locs.extend(jar_locs.iter().cloned());
        }
        if locs.is_empty() {
            None
        } else {
            Some(locs)
        }
    }

    fn get_file_data(&self, uri: &str) -> Option<Arc<FileData>> {
        // URI-keyed `jar_files` read: deliberately NOT gated by a Tier-2
        // promotion — there is no URI→name reverse index to promote by. A
        // URI for a not-yet-materialized JAR misses here (known
        // limitation); callers may pass any URI, though most arrive via
        // name-keyed (promoting) lookups.
        self.files
            .get(uri)
            .map(|rf| rf.clone())
            .or_else(|| self.jar_files.get(uri).map(|rf| rf.clone()))
    }

    fn resolve_locations(
        &self,
        name: &str,
        qualifier: Option<&str>,
        from_uri: &Url,
        allow_rg: bool,
    ) -> Vec<Location> {
        if allow_rg {
            let locs = self.resolve_symbol(name, qualifier, from_uri);
            // Ensure every rg-discovered file is indexed so get_file_data() succeeds.
            for loc in &locs {
                self.ensure_indexed(&loc.uri);
            }
            locs
        } else {
            if let Some(qual) = qualifier {
                // Index-only qualified resolution: resolve the qualifier without rg,
                // then search that file for `name`. Avoids silently dropping the
                // qualifier and returning results for an unrelated top-level symbol.
                let qual_locs = self.resolve_symbol_no_rg(qual, from_uri);
                if let Some(loc) = qual_locs.into_iter().next() {
                    let locs = self.find_name_in_uri(name, loc.uri.as_str());
                    if !locs.is_empty() {
                        return locs;
                    }
                }
            }
            self.resolve_symbol_no_rg(name, from_uri)
        }
    }

    fn infer_variable_type_for(&self, name: &str, uri: &Url) -> Option<String> {
        infer_variable_type(self, name, uri)
    }

    fn ensure_indexed_on_demand(&self, uri: &str) {
        // Fast path: if the file is already in the in-memory index, skip URI parsing.
        if self.files.contains_key(uri) {
            return;
        }
        // Convert string URI to Url and trigger on-demand indexing if needed.
        // Silently skips URIs that can't be parsed — they can't be indexed anyway.
        if let Ok(parsed_uri) = Url::parse(uri) {
            self.ensure_indexed(&parsed_uri);
        }
    }

    fn jar_phase(&self) -> crate::indexer::jar_phase::JarPhase {
        self.jar_phase
            .lock()
            .map(|g| g.clone())
            .unwrap_or(crate::indexer::jar_phase::JarPhase::Unavailable)
    }

    fn materialize_completion_candidate(
        &self,
        fqn: &str,
    ) -> Option<(Option<String>, serde_json::Value)> {
        let (qualifier, leaf) = fqn.rsplit_once('.')?;
        let mut unbudgeted = usize::MAX;
        crate::indexer::jar::ensure_jar_definitions_for(self, leaf, &mut unbudgeted);
        let (detail, data) = crate::resolver::complete::jar_symbol_detail(self, leaf, qualifier)?;
        let data = data?;
        let supports_label_details = self
            .client_label_details_support
            .load(std::sync::atomic::Ordering::Relaxed);
        // Mirror `add_cross_package_symbol`'s fold: without labelDetails
        // support the package hint lives in `detail`, and this resolved
        // `detail` replaces the stub's package-only one.
        let detail = match detail {
            Some(signature) if !supports_label_details => {
                Some(format!("package {qualifier}\n{signature}"))
            }
            other => other,
        };
        Some((detail, data))
    }
}
