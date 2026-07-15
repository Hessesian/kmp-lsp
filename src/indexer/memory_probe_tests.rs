//! Memory-attribution probe (TEST-ONLY, `#[ignore]`d by default).
//!
//! Loads a real on-disk workspace index (`~/.cache/kmp-lsp/<hash>/index.bin`)
//! through the same warm-apply path the server uses (`cache_entry_to_file_result`
//! → `apply_workspace_result`), then walks the reconstructed [`Indexer`] and
//! attributes retained bytes to each in-RAM structure.
//!
//! Run:
//! ```text
//! cargo test --bin kmp-lsp memory_retainer_profile -- --ignored --nocapture
//! ```
//! Corpus selection: env `KMP_LSP_PROFILE_CACHE=<dir>` (dir containing
//! `index.bin`), else the largest `index.bin` under `~/.cache/kmp-lsp/*/`.
//!
//! Zero production code depends on this file; it only reads `pub(crate)` /
//! `pub(super)` items already reachable from inside the crate.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tower_lsp::lsp_types::{Location, Url};

use super::cache::{cache_entry_to_file_result, IndexCache};
use super::jar::{ensure_jar_materialized, populate_tier1_from_manifest};
use super::jar_cache::{load_jar_cache, JarCacheEntry};
use super::jar_manifest_cache::JarManifestName;
use super::sources_jar_cache::{load_sources_jar_cache, SourcesFileEntry, SourcesJarEntry};
use super::Indexer;
use crate::sidecar::SidecarSymbol;
use crate::types::{
    FileData, FileIndexResult, IndexStats, SymbolColdFields, SymbolEntry, SymbolLoc,
    WorkspaceIndexResult,
};

// ─── Sizing constants ──────────────────────────────────────────────────────
// 64-bit targets. `String`/`Vec` headers are ptr+len+cap = 24 bytes and live
// *inline* wherever the value is embedded (struct field or another Vec's
// buffer), so they are charged via `size_of::<T>()` of the container element,
// never double-counted here. We add the separately heap-allocated payload,
// charged as `len()` for Strings — a LOWER BOUND on `capacity()`; bincode
// deserializes with exact-size allocations, so for this corpus len == cap.
// Vec buffers are charged at `capacity()`. Explicit headers are added for
// values that own their own heap block (`Arc<Vec<..>>`, standalone `Vec`).
const STRING_HDR: usize = std::mem::size_of::<String>(); // 24
const VEC_HDR: usize = std::mem::size_of::<Vec<u8>>(); // 24
const ARC_CTL: usize = 16; // strong + weak atomic counts
/// Estimated per-`Location` heap cost *beyond* the inline struct + the URI
/// string bytes: `url::Url` keeps only the serialized string on the heap (the
/// component indices are inline `u32`s in the struct). We therefore charge the
/// URI string length and add this small slop for String cap rounding.
const URL_STR_SLOP: usize = 8;

fn to_mb(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// VmRSS in bytes from /proc/self/status (Linux). 0 if unavailable.
fn vm_rss_bytes() -> usize {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            // "VmRSS:   123456 kB"
            let kb: usize = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            return kb * 1024;
        }
    }
    0
}

/// Best-effort release of freed heap back to the OS so post-drop RSS is
/// meaningful (glibc otherwise retains it). No-op / harmless elsewhere.
#[cfg(target_os = "linux")]
fn trim_heap() {
    extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }
    unsafe {
        let _ = malloc_trim(0);
    }
}

/// `malloc_trim` is glibc-only; the symbol does not exist on macOS/Windows,
/// and even an `#[ignore]`d test must LINK on every platform.
#[cfg(not(target_os = "linux"))]
fn trim_heap() {}

/// Pick the corpus cache dir: env override, else the largest `index.bin`.
fn pick_corpus_dir() -> Option<(PathBuf, u64)> {
    if let Ok(dir) = std::env::var("KMP_LSP_PROFILE_CACHE") {
        let p = PathBuf::from(dir).join("index.bin");
        let size = std::fs::metadata(&p).ok()?.len();
        return Some((p, size));
    }
    let base = super::cache::xdg_cache_base().join("kmp-lsp");
    let mut best: Option<(PathBuf, u64)> = None;
    for entry in std::fs::read_dir(&base).ok()? {
        let Ok(entry) = entry else { continue };
        let idx = entry.path().join("index.bin");
        if let Ok(meta) = std::fs::metadata(&idx) {
            let size = meta.len();
            if best.as_ref().is_none_or(|(_, b)| size > *b) {
                best = Some((idx, size));
            }
        }
    }
    best
}

// ─── Accounting accumulator ─────────────────────────────────────────────────

#[derive(Default, Clone, Copy)]
struct Split {
    ws: usize,
    lib: usize,
}
impl Split {
    fn add(&mut self, is_lib: bool, bytes: usize) {
        if is_lib {
            self.lib += bytes;
        } else {
            self.ws += bytes;
        }
    }
    fn total(&self) -> usize {
        self.ws + self.lib
    }
}

#[derive(Default)]
struct FileTally {
    keys: Split,        // URI map-key String bytes
    file_struct: Split, // Arc control + FileData struct + Vec header slop
    lines: Split,       // Arc<Vec<String>> line text + headers
    sym_struct: Split,  // SymbolEntry vec buffers
    sym_name: Split,
    sym_detail: Split,
    sym_params: Split,
    sym_other_str: Split, // container payload + boxed SymbolColdFields (when present)
    imports: Split,
    identifiers: Split, // declared_names
    cst_vecs: Split,    // supers/rhs/method_call/field_access/type_annotations
    n_ws_files: usize,
    n_lib_files: usize,
    n_symbols: usize,
    sparsity: SymbolFieldSparsity,
}

/// Per-field population counts across every [`SymbolEntry`] in the corpus, plus
/// the joint-emptiness count for the candidate "cold" group. This is the
/// measurement that decides whether an `Option<Box<Cold>>` layout diet would
/// actually shrink the struct: a field being individually rare only helps if it
/// is simultaneously empty with the rest of the boxed group.
///
/// Convention: each counter is how many symbols have the field NON-default
/// (populated). `name`/`kind`/`visibility`/`range`/`selection_range`/
/// `param_counts`/`trailing_lambda`/`deprecated` are always-present or scalar
/// fields that stay inline regardless, so they are not measured here.
#[derive(Default)]
struct SymbolFieldSparsity {
    total: usize,
    detail_populated: usize,
    params_populated: usize,
    type_params_populated: usize,
    extension_receiver_populated: usize,
    extension_receiver_type_populated: usize,
    container_populated: usize,
    doc_populated: usize,
    /// Symbols where EVERY field in the container-INCLUSIVE cold group is
    /// empty/default simultaneously: `type_params` empty AND
    /// `extension_receiver` empty AND `extension_receiver_type` empty AND
    /// `container` is `None` AND `doc` empty.
    cold_group_with_container_all_empty: usize,
    /// Symbols where the container-EXCLUSIVE sparse group is all empty:
    /// `type_params` + `extension_receiver` + `extension_receiver_type` + `doc`.
    /// This is the group that would actually be worth boxing — `container` is
    /// populated on the vast majority of symbols, so folding it in destroys the
    /// joint emptiness. Only these symbols shed the boxed group's inline bytes.
    sparse_group_all_empty: usize,
    /// Same as `sparse_group_all_empty` but also requiring `detail` and
    /// `params` empty — the fraction that would shrink if the WHOLE
    /// variable-payload set were boxed.
    all_variable_fields_empty: usize,
}

impl SymbolFieldSparsity {
    fn observe(&mut self, symbol: &SymbolEntry) {
        self.total += 1;
        let detail_empty = symbol.detail.is_empty();
        let params_empty = symbol.params.is_empty();
        let type_params_empty = symbol.type_params().is_empty();
        let extension_receiver_empty = symbol.extension_receiver().is_empty();
        let extension_receiver_type_empty = symbol.extension_receiver_type().is_empty();
        let container_empty = symbol.container.is_none();
        let doc_empty = symbol.doc().is_empty();

        if !detail_empty {
            self.detail_populated += 1;
        }
        if !params_empty {
            self.params_populated += 1;
        }
        if !type_params_empty {
            self.type_params_populated += 1;
        }
        if !extension_receiver_empty {
            self.extension_receiver_populated += 1;
        }
        if !extension_receiver_type_empty {
            self.extension_receiver_type_populated += 1;
        }
        if !container_empty {
            self.container_populated += 1;
        }
        if !doc_empty {
            self.doc_populated += 1;
        }

        let sparse_group_all_empty = type_params_empty
            && extension_receiver_empty
            && extension_receiver_type_empty
            && doc_empty;
        if sparse_group_all_empty {
            self.sparse_group_all_empty += 1;
        }
        if sparse_group_all_empty && container_empty {
            self.cold_group_with_container_all_empty += 1;
        }
        if sparse_group_all_empty && detail_empty && params_empty {
            self.all_variable_fields_empty += 1;
        }
    }

    fn merge(&mut self, other: &SymbolFieldSparsity) {
        self.total += other.total;
        self.detail_populated += other.detail_populated;
        self.params_populated += other.params_populated;
        self.type_params_populated += other.type_params_populated;
        self.extension_receiver_populated += other.extension_receiver_populated;
        self.extension_receiver_type_populated += other.extension_receiver_type_populated;
        self.container_populated += other.container_populated;
        self.doc_populated += other.doc_populated;
        self.cold_group_with_container_all_empty += other.cold_group_with_container_all_empty;
        self.sparse_group_all_empty += other.sparse_group_all_empty;
        self.all_variable_fields_empty += other.all_variable_fields_empty;
    }
}

fn str_bytes(s: &str) -> usize {
    // heap payload only; the 24-byte header is charged by the container.
    s.len()
}

/// Charge a `Vec<String>` (standalone, owns its buffer): header + slots + payloads.
fn vec_string_bytes(v: &[String]) -> usize {
    VEC_HDR + v.len() * STRING_HDR + v.iter().map(|s| str_bytes(s)).sum::<usize>()
}

fn account_file(t: &mut FileTally, uri_key: &str, is_lib: bool, data: &FileData) {
    if is_lib {
        t.n_lib_files += 1;
    } else {
        t.n_ws_files += 1;
    }

    // Map key (URI string) lives in the DashMap; header charged by shard, we
    // charge payload + one header as an approximation of the owned key String.
    t.keys.add(is_lib, STRING_HDR + str_bytes(uri_key));

    // Arc<FileData>: control block + FileData struct itself.
    t.file_struct
        .add(is_lib, ARC_CTL + std::mem::size_of::<FileData>());

    // lines: Arc<Vec<String>>
    let lines = &data.lines;
    let lines_bytes = ARC_CTL
        + VEC_HDR
        + lines.len() * STRING_HDR
        + lines.iter().map(|l| str_bytes(l)).sum::<usize>();
    t.lines.add(is_lib, lines_bytes);

    // symbols: Vec<SymbolEntry>
    t.sym_struct.add(
        is_lib,
        VEC_HDR + data.symbols.capacity() * std::mem::size_of::<SymbolEntry>(),
    );
    for s in &data.symbols {
        t.n_symbols += 1;
        t.sparsity.observe(s);
        t.sym_name.add(is_lib, str_bytes(&s.name));
        t.sym_detail.add(is_lib, str_bytes(&s.detail));
        t.sym_params.add(is_lib, str_bytes(&s.params));
        // `container` stays inline in `SymbolEntry` (its String header is charged
        // by `size_of::<SymbolEntry>()` in the struct-vec line above); charge only
        // its heap payload here.
        let mut other = 0usize;
        if let Some(container_name) = &s.container {
            other += str_bytes(container_name);
        }
        // Boxed cold group (`type_params`/`extension_receiver`/
        // `extension_receiver_type`/`doc`): when present, a heap-allocated
        // `SymbolColdFields` holds the four inline headers (its `size_of`), plus
        // the field payloads and the `type_params` Vec buffer. When absent
        // (~99% of symbols) nothing is charged here — only the 8-byte
        // `Option<Box<..>>` pointer, already counted in `size_of::<SymbolEntry>()`.
        if s.cold.is_some() {
            other += std::mem::size_of::<SymbolColdFields>()
                + str_bytes(s.extension_receiver())
                + str_bytes(s.extension_receiver_type())
                + str_bytes(s.doc())
                + s.type_params().len() * STRING_HDR
                + s.type_params()
                    .iter()
                    .map(|type_param| str_bytes(type_param))
                    .sum::<usize>();
        }
        t.sym_other_str.add(is_lib, other);
    }

    // imports: Vec<ImportEntry> (two Strings each)
    let mut imports_bytes =
        VEC_HDR + data.imports.capacity() * std::mem::size_of::<crate::types::ImportEntry>();
    for import_entry in &data.imports {
        imports_bytes += str_bytes(&import_entry.full_path) + str_bytes(&import_entry.local_name);
    }
    t.imports.add(is_lib, imports_bytes);
    if let Some(package_name) = &data.package {
        t.imports.add(is_lib, STRING_HDR + str_bytes(package_name));
    }

    // declared_names: Vec<String>
    t.identifiers
        .add(is_lib, vec_string_bytes(&data.declared_names));

    // CST inference side-tables (Vecs of tuples with Strings)
    let mut side_table_bytes = 0usize;
    // supers: Vec<(u32, String, Vec<String>)>
    side_table_bytes +=
        VEC_HDR + data.supers.capacity() * std::mem::size_of::<(u32, String, Vec<String>)>();
    for (_, super_name, super_type_params) in &data.supers {
        side_table_bytes += str_bytes(super_name) + vec_string_bytes(super_type_params);
    }
    // rhs_types / type_annotations: Vec<(u32, String, String)>
    for table in [&data.rhs_types, &data.type_annotations] {
        side_table_bytes +=
            VEC_HDR + table.capacity() * std::mem::size_of::<(u32, String, String)>();
        for (_, first_string, second_string) in table {
            side_table_bytes += str_bytes(first_string) + str_bytes(second_string);
        }
    }
    // method_call_rhs / field_access_rhs: Vec<(u32, String, String, String)>
    for table in [&data.method_call_rhs, &data.field_access_rhs] {
        side_table_bytes +=
            VEC_HDR + table.capacity() * std::mem::size_of::<(u32, String, String, String)>();
        for (_, first_string, second_string, third_string) in table {
            side_table_bytes +=
                str_bytes(first_string) + str_bytes(second_string) + str_bytes(third_string);
        }
    }
    t.cst_vecs.add(is_lib, side_table_bytes);
}

fn location_bytes(loc: &Location) -> usize {
    // Struct bytes are charged by the containing Vec buffer; here add the heap
    // URI string payload + small Url slop.
    loc.uri.as_str().len() + URL_STR_SLOP
}

// ─── Row / table ────────────────────────────────────────────────────────────

struct Row {
    name: &'static str,
    entries: usize,
    ws: usize,
    lib: usize,
}
impl Row {
    fn bytes(&self) -> usize {
        self.ws + self.lib
    }
}

#[test]
#[ignore = "manual memory profiling against a real on-disk cache"]
fn memory_retainer_profile() {
    let rss_before = vm_rss_bytes();

    let (corpus, on_disk) = pick_corpus_dir().expect(
        "no cache corpus found; set KMP_LSP_PROFILE_CACHE or populate ~/.cache/kmp-lsp/*/index.bin",
    );
    let corpus_dir = corpus.parent().unwrap().to_path_buf();
    eprintln!(
        "corpus: {}  (index.bin on-disk = {:.1} MB)",
        corpus_dir.display(),
        to_mb(on_disk as usize)
    );

    // ── Load through the real warm-apply path ──────────────────────────────
    let bytes = std::fs::read(&corpus).expect("read index.bin");
    let cache: IndexCache = bincode::deserialize(&bytes)
        .expect("deserialize IndexCache — struct layout must match CACHE_VERSION of this build");
    drop(bytes);
    assert_eq!(
        cache.version,
        super::cache::CACHE_VERSION,
        "on-disk cache version differs from this build"
    );

    let complete_scan = cache.complete_scan;
    // Consume the cache HashMap so each on-disk `Arc<FileData>` is freed as soon
    // as its FileIndexResult is built. `into_iter` frees each entry immediately
    // after use, so this building phase never holds two full copies of the file
    // data — the apply phase below is where the old 2x peak came from.
    let mut skipped_paths = 0usize;
    let results: Vec<FileIndexResult> = cache
        .entries
        .into_iter()
        .filter_map(
            |(path_str, entry)| match Url::from_file_path(Path::new(&path_str)) {
                Ok(uri) => Some(cache_entry_to_file_result(&uri, &entry)),
                Err(()) => {
                    skipped_paths += 1;
                    eprintln!("probe: skipping cache entry with non-file path: {path_str}");
                    None
                }
            },
        )
        .collect();
    assert_eq!(
        skipped_paths, 0,
        "cache entries were skipped — attribution below would be incomplete"
    );
    let n_entries = results.len();

    let indexer = Indexer::new();
    let result = WorkspaceIndexResult {
        files: results,
        stats: IndexStats::default(),
        workspace_root: corpus_dir.clone(),
        aborted: false,
        complete_scan,
    };
    // `apply_workspace_result` now takes `result` by value and *moves* each
    // file's FileData into its Arc (via `file_contributions_owned`), draining the
    // `result.files` vec as the maps are populated. The two full copies never
    // coexist, so this read reflects the ~1x apply peak, not the old ~2x.
    indexer.apply_workspace_result(result);
    let rss_peak = vm_rss_bytes();

    // The transient loader state was consumed by the apply above; the Indexer is
    // now the sole retainer. Trim so post-apply RSS is meaningful.
    trim_heap();

    let rss_after_load = vm_rss_bytes();

    // ── Walk the Indexer and attribute bytes ───────────────────────────────
    let library: std::collections::HashSet<String> =
        indexer.library_uris.iter().map(|u| u.clone()).collect();
    let is_lib = |uri: &str| library.contains(uri);

    let mut ft = FileTally::default();
    for e in indexer.files.iter() {
        account_file(&mut ft, e.key(), is_lib(e.key()), e.value());
    }

    // definitions: DashMap<String, Vec<SymbolLoc>>
    // Values are interned `SymbolLoc`s stored inline in the Vec buffer (20 B each,
    // no per-loc heap — each file's URI lives once in `file_table`, accounted below).
    // "SymbolLocs" charges the live inline entries (with ws/lib split via
    // `file_table`); "Vec buffers" charges only the Vec header + spare-capacity slack,
    // so the two rows sum to the full buffer allocation with no double-count.
    let mut def_keys = Split::default();
    let mut def_vec = Split::default();
    let mut def_locs = Split::default();
    let mut def_loc_count = 0usize;
    for e in indexer.definitions.iter() {
        def_keys.ws += STRING_HDR + str_bytes(e.key());
        let vec = e.value();
        def_vec.ws +=
            VEC_HDR + vec.capacity().saturating_sub(vec.len()) * std::mem::size_of::<SymbolLoc>();
        for loc in vec.iter() {
            def_loc_count += 1;
            let is_library = indexer
                .file_table
                .url(loc.file)
                .is_some_and(|url| is_lib(url.as_str()));
            def_locs.add(is_library, std::mem::size_of::<SymbolLoc>());
        }
    }

    // qualified: DashMap<String, SymbolLoc>
    // The value is now an interned `SymbolLoc` (FileId + Range), stored inline in
    // the DashMap node with NO per-entry heap — the file URI lives once in
    // `file_table` (accounted separately below), not once per symbol. We charge
    // the inline struct size here so the row reflects the new per-entry cost.
    let mut qual_keys = Split::default();
    let mut qual_locs = Split::default();
    for e in indexer.qualified.iter() {
        qual_keys.ws += STRING_HDR + str_bytes(e.key());
        let is_library = indexer
            .file_table
            .url(e.value().file)
            .is_some_and(|url| is_lib(url.as_str()));
        qual_locs.add(is_library, std::mem::size_of::<SymbolLoc>());
    }

    // file_table: one Arc<Url> per interned file (by_id Vec) + the reverse
    // DashMap<String, FileId> (by_uri). This is where the per-FILE URI heap now
    // lives — previously duplicated per-SYMBOL across `qualified` (and, in later
    // migration steps, `definitions`/`subtypes`).
    let mut file_table_split = Split::default();
    let interned_urls = indexer.file_table.urls_snapshot();
    let interned_file_count = interned_urls.len();
    // by_id Vec buffer: one Arc pointer slot per file.
    file_table_split.ws += VEC_HDR + interned_file_count * std::mem::size_of::<Arc<Url>>();
    for url in &interned_urls {
        let uri = url.as_str();
        let library = is_lib(uri);
        // The Arc<Url> allocation: control block + the Url struct + its heap string.
        file_table_split.add(
            library,
            ARC_CTL + std::mem::size_of::<Url>() + uri.len() + URL_STR_SLOP,
        );
        // by_uri entry: owned key String (dup of the URI) + the FileId value.
        file_table_split.add(
            library,
            STRING_HDR + uri.len() + std::mem::size_of::<crate::types::FileId>(),
        );
    }

    // subtypes: DashMap<String, Vec<SymbolLoc>>
    // Same interned shape as `definitions` (see that block): "SymbolLocs" charges
    // the live inline entries, "Vec buffers" the header + spare-capacity slack.
    let mut sub_keys = Split::default();
    let mut sub_vec = Split::default();
    let mut sub_locs = Split::default();
    let mut sub_loc_count = 0usize;
    for e in indexer.subtypes.iter() {
        sub_keys.ws += STRING_HDR + str_bytes(e.key());
        let vec = e.value();
        sub_vec.ws +=
            VEC_HDR + vec.capacity().saturating_sub(vec.len()) * std::mem::size_of::<SymbolLoc>();
        for loc in vec.iter() {
            sub_loc_count += 1;
            let is_library = indexer
                .file_table
                .url(loc.file)
                .is_some_and(|url| is_lib(url.as_str()));
            sub_locs.add(is_library, std::mem::size_of::<SymbolLoc>());
        }
    }

    // packages: DashMap<String, Vec<FileId>> — values are 4-byte interned handles
    // (the file URIs live once in `file_table`), not duplicated URI strings.
    let mut pkg_bytes = 0usize;
    for e in indexer.packages.iter() {
        pkg_bytes += STRING_HDR
            + str_bytes(e.key())
            + VEC_HDR
            + e.value().capacity() * std::mem::size_of::<crate::types::FileId>();
    }

    // content_hashes: DashMap<String, u64>
    let mut ch_bytes = 0usize;
    for e in indexer.content_hashes.iter() {
        ch_bytes += STRING_HDR + str_bytes(e.key()) + 8;
    }

    // importable_fqns: RwLock<HashMap<String, Vec<String>>>
    let mut imp_fqn_bytes = 0usize;
    {
        let map = indexer.importable_fqns.read().unwrap();
        for (k, v) in map.iter() {
            imp_fqn_bytes += STRING_HDR + str_bytes(k) + vec_string_bytes(v);
        }
    }

    // bare_name_cache: RwLock<Vec<String>>
    let bare_bytes = { vec_string_bytes(&indexer.bare_name_cache.read().unwrap()) };

    // JAR maps (empty on a workspace-only warm load, but account anyway).
    let mut jar_bytes = 0usize;
    let mut jar_file_count = 0usize;
    for e in indexer.jar_files.iter() {
        jar_file_count += 1;
        let mut junk = FileTally::default();
        account_file(&mut junk, e.key(), true, e.value());
        ft.sparsity.merge(&junk.sparsity);
        jar_bytes += junk.keys.total()
            + junk.file_struct.total()
            + junk.lines.total()
            + junk.sym_struct.total()
            + junk.sym_name.total()
            + junk.sym_detail.total()
            + junk.sym_params.total()
            + junk.sym_other_str.total()
            + junk.imports.total()
            + junk.identifiers.total()
            + junk.cst_vecs.total();
    }
    for e in indexer.jar_definitions.iter() {
        jar_bytes += STRING_HDR
            + str_bytes(e.key())
            + VEC_HDR
            + e.value().capacity() * std::mem::size_of::<Location>();
        for loc in e.value() {
            jar_bytes += location_bytes(loc);
        }
    }
    for e in indexer.jar_symbol_packages.iter() {
        jar_bytes += STRING_HDR + str_bytes(e.key()) + vec_string_bytes(e.value());
    }
    for e in indexer.jar_uri_to_defs.iter() {
        jar_bytes += STRING_HDR + str_bytes(e.key()) + vec_string_bytes(e.value());
    }

    let rss_after_walk = vm_rss_bytes();

    // ── Build the table ────────────────────────────────────────────────────
    let rows = vec![
        Row {
            name: "files: line text (lines)",
            entries: n_entries,
            ws: ft.lines.ws,
            lib: ft.lines.lib,
        },
        Row {
            name: "files: symbol structs",
            entries: ft.n_symbols,
            ws: ft.sym_struct.ws,
            lib: ft.sym_struct.lib,
        },
        Row {
            name: "files: symbol .name",
            entries: ft.n_symbols,
            ws: ft.sym_name.ws,
            lib: ft.sym_name.lib,
        },
        Row {
            name: "files: symbol .detail",
            entries: ft.n_symbols,
            ws: ft.sym_detail.ws,
            lib: ft.sym_detail.lib,
        },
        Row {
            name: "files: symbol .params",
            entries: ft.n_symbols,
            ws: ft.sym_params.ws,
            lib: ft.sym_params.lib,
        },
        Row {
            name: "files: symbol other str",
            entries: ft.n_symbols,
            ws: ft.sym_other_str.ws,
            lib: ft.sym_other_str.lib,
        },
        Row {
            name: "files: imports+package",
            entries: n_entries,
            ws: ft.imports.ws,
            lib: ft.imports.lib,
        },
        Row {
            name: "files: identifiers (declared)",
            entries: n_entries,
            ws: ft.identifiers.ws,
            lib: ft.identifiers.lib,
        },
        Row {
            name: "files: CST side-tables",
            entries: n_entries,
            ws: ft.cst_vecs.ws,
            lib: ft.cst_vecs.lib,
        },
        Row {
            name: "files: FileData+Arc struct",
            entries: n_entries,
            ws: ft.file_struct.ws,
            lib: ft.file_struct.lib,
        },
        Row {
            name: "files: map keys (URI)",
            entries: n_entries,
            ws: ft.keys.ws,
            lib: ft.keys.lib,
        },
        Row {
            name: "definitions: keys",
            entries: indexer.definitions.len(),
            ws: def_keys.ws,
            lib: def_keys.lib,
        },
        Row {
            name: "definitions: Vec buffers",
            entries: indexer.definitions.len(),
            ws: def_vec.ws,
            lib: def_vec.lib,
        },
        Row {
            name: "definitions: SymbolLocs",
            entries: def_loc_count,
            ws: def_locs.ws,
            lib: def_locs.lib,
        },
        Row {
            name: "qualified: keys",
            entries: indexer.qualified.len(),
            ws: qual_keys.ws,
            lib: qual_keys.lib,
        },
        Row {
            name: "qualified: SymbolLocs",
            entries: indexer.qualified.len(),
            ws: qual_locs.ws,
            lib: qual_locs.lib,
        },
        Row {
            name: "file_table: interned URIs",
            entries: interned_file_count,
            ws: file_table_split.ws,
            lib: file_table_split.lib,
        },
        Row {
            name: "subtypes: keys",
            entries: indexer.subtypes.len(),
            ws: sub_keys.ws,
            lib: sub_keys.lib,
        },
        Row {
            name: "subtypes: Vec buffers",
            entries: indexer.subtypes.len(),
            ws: sub_vec.ws,
            lib: sub_vec.lib,
        },
        Row {
            name: "subtypes: SymbolLocs",
            entries: sub_loc_count,
            ws: sub_locs.ws,
            lib: sub_locs.lib,
        },
        Row {
            name: "packages",
            entries: indexer.packages.len(),
            ws: pkg_bytes,
            lib: 0,
        },
        Row {
            name: "content_hashes",
            entries: indexer.content_hashes.len(),
            ws: ch_bytes,
            lib: 0,
        },
        Row {
            name: "importable_fqns",
            entries: indexer.importable_fqns.read().unwrap().len(),
            ws: imp_fqn_bytes,
            lib: 0,
        },
        Row {
            name: "bare_name_cache",
            entries: indexer.bare_name_cache.read().unwrap().len(),
            ws: bare_bytes,
            lib: 0,
        },
        Row {
            name: "jar_* (files/defs/pkgs)",
            entries: jar_file_count,
            ws: 0,
            lib: jar_bytes,
        },
    ];

    let accounted: usize = rows.iter().map(Row::bytes).sum();
    let ws_total: usize = rows.iter().map(|r| r.ws).sum();
    let lib_total: usize = rows.iter().map(|r| r.lib).sum();

    eprintln!();
    eprintln!(
        "struct sizes: FileData={}B SymbolEntry={}B Location={}B ImportEntry={}B",
        std::mem::size_of::<FileData>(),
        std::mem::size_of::<SymbolEntry>(),
        std::mem::size_of::<Location>(),
        std::mem::size_of::<crate::types::ImportEntry>()
    );
    eprintln!(
        "files: {} entries ({} workspace, {} library), {} symbols total",
        indexer.files.len(),
        ft.n_ws_files,
        ft.n_lib_files,
        ft.n_symbols
    );
    eprintln!();
    eprintln!(
        "{:<32} {:>10} {:>11} {:>11} {:>11} {:>7}",
        "retainer", "entries", "ws MB", "lib MB", "MB", "% acct"
    );
    eprintln!("{}", "-".repeat(86));
    let mut sortable: Vec<&Row> = rows.iter().collect();
    sortable.sort_by_key(|r| std::cmp::Reverse(r.bytes()));
    for r in &sortable {
        eprintln!(
            "{:<32} {:>10} {:>11.2} {:>11.2} {:>11.2} {:>6.1}%",
            r.name,
            r.entries,
            to_mb(r.ws),
            to_mb(r.lib),
            to_mb(r.bytes()),
            100.0 * r.bytes() as f64 / accounted as f64
        );
    }
    eprintln!("{}", "-".repeat(86));
    eprintln!(
        "{:<32} {:>10} {:>11.2} {:>11.2} {:>11.2} {:>6.1}%",
        "TOTAL (accounted)",
        n_entries,
        to_mb(ws_total),
        to_mb(lib_total),
        to_mb(accounted),
        100.0
    );

    // ── Process truth cross-check ──────────────────────────────────────────
    let rss_delta = rss_after_load.saturating_sub(rss_before);
    eprintln!();
    eprintln!("RSS before load:      {:>8.1} MB", to_mb(rss_before));
    eprintln!(
        "RSS apply peak:       {:>8.1} MB  (result vec moved into Indexer, no 2nd copy)",
        to_mb(rss_peak)
    );
    eprintln!(
        "RSS after load+drop:  {:>8.1} MB  (Δ vs before = {:.1} MB)",
        to_mb(rss_after_load),
        to_mb(rss_delta)
    );
    eprintln!("RSS after probe walk: {:>8.1} MB", to_mb(rss_after_walk));
    eprintln!("NOTE: bin uses jemalloc — freed transient clones are retained by the");
    eprintln!("      allocator, so RSS≈2x accounted reflects warm-load PEAK, not steady state.");
    eprintln!(
        "accounted total:      {:>8.1} MB  ({:.1}% workspace / {:.1}% library)",
        to_mb(accounted),
        100.0 * ws_total as f64 / accounted as f64,
        100.0 * lib_total as f64 / accounted as f64
    );
    let gap = rss_delta.saturating_sub(accounted);
    eprintln!("unaccounted (RSS Δ − accounted): {:.1} MB  (allocator slack + DashMap shard/hashtable overhead)",
        to_mb(gap));

    // top-3 mechanisms
    eprintln!();
    eprintln!("top-3 retainers:");
    for (i, r) in sortable.iter().take(3).enumerate() {
        eprintln!(
            "  {}. {} — {:.1} MB ({:.1}% of accounted)",
            i + 1,
            r.name,
            to_mb(r.bytes()),
            100.0 * r.bytes() as f64 / accounted as f64
        );
    }

    // ── SymbolEntry field sparsity ─────────────────────────────────────────
    // Evidence for the "SymbolEntry diet" decision: how often is each variable
    // field actually populated, and how often is the whole candidate cold group
    // simultaneously empty (the only symbols that would shrink under a
    // `Option<Box<Cold>>` split)?
    let sparsity = &ft.sparsity;
    let total = sparsity.total.max(1);
    let percent_populated = |count: usize| 100.0 * count as f64 / total as f64;
    eprintln!();
    eprintln!(
        "SymbolEntry field sparsity ({} symbols, size_of = {} B):",
        sparsity.total,
        std::mem::size_of::<SymbolEntry>()
    );
    eprintln!("{:<30} {:>12} {:>9}", "field (populated)", "count", "% pop");
    eprintln!("{}", "-".repeat(53));
    for (label, count) in [
        ("detail (String)", sparsity.detail_populated),
        ("params (String)", sparsity.params_populated),
        ("type_params (Vec)", sparsity.type_params_populated),
        (
            "extension_receiver (String)",
            sparsity.extension_receiver_populated,
        ),
        (
            "extension_receiver_type (String)",
            sparsity.extension_receiver_type_populated,
        ),
        ("container (Option)", sparsity.container_populated),
        ("doc (String)", sparsity.doc_populated),
    ] {
        eprintln!("{label:<30} {count:>12} {:>8.1}%", percent_populated(count));
    }
    eprintln!("{}", "-".repeat(53));
    eprintln!("joint-emptiness (fraction that would shed a boxed group's inline bytes):");
    eprintln!("  sparse group (type_params + extension_receiver + extension_receiver_type + doc):");
    eprintln!(
        "    {} / {}  ({:.1}%)  ← container EXCLUDED (the group worth boxing)",
        sparsity.sparse_group_all_empty,
        sparsity.total,
        percent_populated(sparsity.sparse_group_all_empty)
    );
    eprintln!("  + container (adds `container is None` to the sparse group):");
    eprintln!(
        "    {} / {}  ({:.1}%)  ← container INCLUDED (populated on most symbols → collapses)",
        sparsity.cold_group_with_container_all_empty,
        sparsity.total,
        percent_populated(sparsity.cold_group_with_container_all_empty)
    );
    eprintln!("  + detail + params (whole variable-payload set):");
    eprintln!(
        "    {} / {}  ({:.1}%)",
        sparsity.all_variable_fields_empty,
        sparsity.total,
        percent_populated(sparsity.all_variable_fields_empty)
    );

    // Keep the Indexer alive across the RSS reads.
    assert!(!indexer.files.is_empty(), "loaded a non-empty index");
    drop(indexer);
}

// ─── Library-JAR cache footprint probe ──────────────────────────────────────
//
// The companion probe above loads ONLY `~/.cache/kmp-lsp/<hash>/index.bin` — the
// per-workspace source index — which by design excludes all library data. This
// probe measures the two GLOBAL library caches the eager sources-JAR pipeline
// writes, which every prior measurement in the memory effort skipped:
//   • `jar-symbols-v{JAR_CACHE_VERSION}.bin`  — compiled-JAR sidecar symbols
//     (`HashMap<String, JarCacheEntry>`, `Vec<SidecarSymbol>` per JAR).
//   • `sources-jar-v2-c{CACHE_VERSION}.bin`   — per-source-file tree-sitter
//     parse output (`HashMap<String, SourcesJarEntry>`, each holding
//     `Vec<SourcesFileEntry>` of `Arc<FileData>` — the SAME `FileData`/
//     `SymbolEntry` shapes the workspace probe already attributes).
//
// Run:
// ```text
// cargo test --bin kmp-lsp library_jar_cache_footprint -- --ignored --nocapture
// ```

/// `~/.cache/kmp-lsp` (or `$XDG_CACHE_HOME/kmp-lsp`) — the directory both global
/// library caches live in.
fn kmp_lsp_cache_base() -> PathBuf {
    super::cache::xdg_cache_base().join("kmp-lsp")
}

/// Total on-disk bytes of every cache file whose name matches `prefix`/`suffix`
/// (the version-embedding names are not known at compile time). `0` if none.
fn on_disk_bytes_matching(prefix: &str, suffix: &str) -> u64 {
    let Ok(dir) = std::fs::read_dir(kmp_lsp_cache_base()) else {
        return 0;
    };
    dir.flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str().map(str::to_owned)?;
            if name.starts_with(prefix) && name.ends_with(suffix) {
                entry.metadata().ok().map(|m| m.len())
            } else {
                None
            }
        })
        .sum()
}

/// Every `sources-jar-*.bin` present on disk, with its byte size — used to
/// diagnose a filename-version skew (the sources cache filename embeds
/// `CACHE_VERSION`, which the `SymbolEntry` layout is coupled to).
fn sources_jar_cache_files_on_disk() -> Vec<(String, u64)> {
    let mut found = Vec::new();
    let Ok(dir) = std::fs::read_dir(kmp_lsp_cache_base()) else {
        return found;
    };
    for entry in dir.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with("sources-jar-") && name.ends_with(".bin") {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            found.push((name, size));
        }
    }
    found.sort();
    found
}

/// Charge an inline `Vec<String>` field whose 24-byte header is already counted
/// by the enclosing struct's `size_of` (so, unlike [`vec_string_bytes`], it adds
/// NO `VEC_HDR`): only the per-element `String` headers plus heap payloads.
fn inline_vec_string_bytes(v: &[String]) -> usize {
    v.len() * STRING_HDR + v.iter().map(|s| str_bytes(s)).sum::<usize>()
}

/// Separately heap-allocated payload of one [`SidecarSymbol`], EXCLUDING the
/// `size_of::<SidecarSymbol>()` slot it occupies in the enclosing `Vec` buffer
/// (that slot — including every inline `String`/`Vec` header — is charged once at
/// the Vec-buffer level, matching `account_file`'s convention).
fn sidecar_symbol_heap_bytes(symbol: &SidecarSymbol) -> usize {
    str_bytes(&symbol.name)
        + str_bytes(&symbol.kind)
        + str_bytes(&symbol.container)
        + str_bytes(&symbol.detail)
        + str_bytes(&symbol.doc)
        + str_bytes(&symbol.extension_receiver_type)
        + str_bytes(&symbol.pkg)
        + inline_vec_string_bytes(&symbol.type_params)
        + inline_vec_string_bytes(&symbol.supers)
}

/// Sum the FileData-derived bytes a [`FileTally`] accumulated (mirrors the
/// `jar_bytes` roll-up in `memory_retainer_profile`).
fn file_tally_total(tally: &FileTally) -> usize {
    tally.keys.total()
        + tally.file_struct.total()
        + tally.lines.total()
        + tally.sym_struct.total()
        + tally.sym_name.total()
        + tally.sym_detail.total()
        + tally.sym_params.total()
        + tally.sym_other_str.total()
        + tally.imports.total()
        + tally.identifiers.total()
        + tally.cst_vecs.total()
}

#[test]
#[ignore = "manual library-JAR cache memory profiling against real on-disk caches"]
fn library_jar_cache_footprint() {
    let rss_start = vm_rss_bytes();
    eprintln!(
        "struct sizes: JarCacheEntry={}B SidecarSymbol={}B SourcesJarEntry={}B \
         SourcesFileEntry={}B FileData={}B SymbolEntry={}B",
        std::mem::size_of::<JarCacheEntry>(),
        std::mem::size_of::<SidecarSymbol>(),
        std::mem::size_of::<SourcesJarEntry>(),
        std::mem::size_of::<SourcesFileEntry>(),
        std::mem::size_of::<FileData>(),
        std::mem::size_of::<SymbolEntry>(),
    );
    eprintln!("RSS at start:                {:>8.1} MB", to_mb(rss_start));
    eprintln!();

    // ══ Cache 1: jar-symbols (compiled-JAR sidecar symbols) ══════════════════
    // Loaded via the real production path. `load_jar_cache` reads the WHOLE file
    // into a `Vec<u8>` (`std::fs::read`) and only then deserializes, so the raw
    // bytes and the fully-built map briefly coexist — see the double-copy note in
    // the report. The on-disk name is independent of `CACHE_VERSION`.
    let jar_disk = on_disk_bytes_matching("jar-symbols-v", ".bin");
    let jar_cache: std::collections::HashMap<String, JarCacheEntry> = load_jar_cache();
    let rss_after_jar_load = vm_rss_bytes();

    let mut jar_key_bytes = 0usize;
    let mut jar_entry_struct_bytes = 0usize;
    let mut jar_symbol_vec_bytes = 0usize;
    let mut jar_symbol_heap = 0usize;
    let mut jar_symbol_count = 0usize;
    for (jar_path, entry) in &jar_cache {
        jar_key_bytes += STRING_HDR + str_bytes(jar_path);
        jar_entry_struct_bytes += std::mem::size_of::<JarCacheEntry>();
        jar_symbol_vec_bytes +=
            VEC_HDR + entry.symbols.capacity() * std::mem::size_of::<SidecarSymbol>();
        for symbol in &entry.symbols {
            jar_symbol_count += 1;
            jar_symbol_heap += sidecar_symbol_heap_bytes(symbol);
        }
    }
    let jar_accounted =
        jar_key_bytes + jar_entry_struct_bytes + jar_symbol_vec_bytes + jar_symbol_heap;
    let rss_after_jar_walk = vm_rss_bytes();

    eprintln!("── jar-symbols cache ────────────────────────────────────────────");
    eprintln!(
        "on-disk:                     {:>8.1} MB",
        to_mb(jar_disk as usize)
    );
    eprintln!(
        "entries (JARs):              {:>8}   symbols: {}",
        jar_cache.len(),
        jar_symbol_count
    );
    eprintln!(
        "  map keys (JAR paths):      {:>8.1} MB",
        to_mb(jar_key_bytes)
    );
    eprintln!(
        "  JarCacheEntry structs:     {:>8.1} MB",
        to_mb(jar_entry_struct_bytes)
    );
    eprintln!(
        "  symbol Vec buffers:        {:>8.1} MB",
        to_mb(jar_symbol_vec_bytes)
    );
    eprintln!(
        "  symbol String payloads:    {:>8.1} MB",
        to_mb(jar_symbol_heap)
    );
    eprintln!(
        "  ACCOUNTED:                 {:>8.1} MB   (disk→RAM ratio {:.2}x)",
        to_mb(jar_accounted),
        jar_accounted as f64 / jar_disk.max(1) as f64
    );
    eprintln!(
        "RSS after jar load:          {:>8.1} MB   (Δ from start = {:.1} MB)",
        to_mb(rss_after_jar_load),
        to_mb(rss_after_jar_load.saturating_sub(rss_start))
    );
    eprintln!(
        "RSS after jar walk:          {:>8.1} MB",
        to_mb(rss_after_jar_walk)
    );
    eprintln!();

    // ══ Cache 2: sources-jar (per-source-file FileData) ══════════════════════
    // Loaded via `load_sources_jar_cache(None)`. NOTE: the sources-cache filename
    // embeds `CACHE_VERSION`, and `SymbolEntry`'s serialized layout is coupled to
    // it. If the on-disk file was written by a DIFFERENT `CACHE_VERSION` than this
    // build, `load_sources_jar_cache(None)` cannot find it (path mismatch) and
    // returns empty — the version skew is reported rather than silently measured.
    let sources_disk_files = sources_jar_cache_files_on_disk();
    let sources_cache = load_sources_jar_cache(None);
    let rss_after_sources_load = vm_rss_bytes();

    eprintln!("── sources-jar cache ────────────────────────────────────────────");
    eprintln!("this build CACHE_VERSION = {}", super::cache::CACHE_VERSION);
    eprintln!(
        "sources-jar-*.bin on disk:   {} file(s)",
        sources_disk_files.len()
    );
    for (name, size) in &sources_disk_files {
        eprintln!("    {name}  ({:.1} MB)", to_mb(*size as usize));
    }

    if sources_cache.is_empty() {
        eprintln!(
            "load_sources_jar_cache(None) returned an EMPTY map — the loader looks for \
             `sources-jar-v2-c{}.bin`, which is not present. The on-disk file(s) above were \
             written by a different CACHE_VERSION, whose FileData/SymbolEntry byte layout is \
             INCOMPATIBLE with this build (bincode 1.x is positional). The real sources-jar \
             footprint CANNOT be measured on this branch without either regenerating the cache \
             with this build or checking out the build that wrote it.",
            super::cache::CACHE_VERSION
        );
        eprintln!();
        eprintln!("══ combined (measurable on this build) ══════════════════════════");
        eprintln!(
            "jar-symbols accounted:       {:>8.1} MB   (sources-jar: version-skew BLOCKED)",
            to_mb(jar_accounted)
        );
        // Cross-check the loader really returned nothing before asserting the jar side.
        assert!(
            sources_cache.is_empty(),
            "sources cache unexpectedly populated"
        );
        assert!(
            !jar_cache.is_empty(),
            "jar-symbols cache is empty — nothing was measured; \
             is ~/.cache/kmp-lsp/jar-symbols-v*.bin present?"
        );
        drop(jar_cache);
        return;
    }

    // ── Sources cache IS loadable: attribute it with the same rigor as the
    //    workspace probe (`account_file` for the FileData shapes, plus this
    //    cache's own container overhead). ────────────────────────────────────
    let mut sources_tally = FileTally::default();
    let mut sources_jar_key_bytes = 0usize;
    let mut sources_entry_struct_bytes = 0usize;
    let mut sources_file_vec_bytes = 0usize;
    let mut sources_file_count = 0usize;
    for (jar_path, entry) in &sources_cache {
        sources_jar_key_bytes += STRING_HDR + str_bytes(jar_path);
        sources_entry_struct_bytes += std::mem::size_of::<SourcesJarEntry>();
        sources_file_vec_bytes +=
            VEC_HDR + entry.files.capacity() * std::mem::size_of::<SourcesFileEntry>();
        for file_entry in &entry.files {
            sources_file_count += 1;
            // `account_file` charges the URI (as key), the Arc<FileData> control
            // block + FileData struct, and every line/symbol/import/CST payload.
            account_file(
                &mut sources_tally,
                &file_entry.uri,
                true,
                &file_entry.file_data,
            );
        }
    }
    let sources_file_bytes = file_tally_total(&sources_tally);
    let sources_container_bytes =
        sources_jar_key_bytes + sources_entry_struct_bytes + sources_file_vec_bytes;
    let sources_accounted = sources_file_bytes + sources_container_bytes;
    let rss_after_sources_walk = vm_rss_bytes();
    let sources_disk: u64 = sources_disk_files.iter().map(|(_, size)| *size).sum();

    eprintln!(
        "entries (JARs):              {:>8}   source files: {}   symbols: {}",
        sources_cache.len(),
        sources_file_count,
        sources_tally.n_symbols
    );
    eprintln!(
        "  container (keys+structs+Vec):{:>6.1} MB",
        to_mb(sources_container_bytes)
    );
    eprintln!(
        "  FileData (lines/symbols/…): {:>8.1} MB",
        to_mb(sources_file_bytes)
    );
    eprintln!(
        "  ACCOUNTED:                 {:>8.1} MB   (disk→RAM ratio {:.2}x)",
        to_mb(sources_accounted),
        sources_accounted as f64 / sources_disk.max(1) as f64
    );
    eprintln!(
        "RSS after sources load:      {:>8.1} MB   (this is the COMBINED peak — jar still live)",
        to_mb(rss_after_sources_load)
    );
    eprintln!(
        "RSS after sources walk:      {:>8.1} MB",
        to_mb(rss_after_sources_walk)
    );
    eprintln!();

    let combined_accounted = jar_accounted + sources_accounted;
    let combined_disk = jar_disk + sources_disk;
    eprintln!("══ combined ═════════════════════════════════════════════════════");
    eprintln!(
        "combined on-disk:            {:>8.1} MB",
        to_mb(combined_disk as usize)
    );
    eprintln!(
        "combined ACCOUNTED:          {:>8.1} MB   (disk→RAM ratio {:.2}x)",
        to_mb(combined_accounted),
        combined_accounted as f64 / combined_disk.max(1) as f64
    );
    eprintln!(
        "combined RSS peak:           {:>8.1} MB   (Δ from start = {:.1} MB)",
        to_mb(rss_after_sources_load.max(rss_after_jar_load)),
        to_mb(
            rss_after_sources_load
                .max(rss_after_jar_load)
                .saturating_sub(rss_start)
        )
    );

    trim_heap();
    assert!(!jar_cache.is_empty(), "jar-symbols cache is empty");
    drop(jar_cache);
    drop(sources_cache);
}

// ─── Lazy-JAR-loading Tier-1/Tier-2 split profile ───────────────────────────
//
// Tasks 1-12 flipped the crawl from eagerly materializing every compiled
// JAR's full symbol data (`jar_definitions`/`jar_files`, Tier 2) to only
// building a cheap name+kind+container(+package) manifest (`jar_bare_names`/
// `jar_qualified`, Tier 1) — see design §Tier 1. This is the number the
// whole design exists to produce: how much of the real corpus never gets
// touched by a realistic session (and so stays Tier-1-only) vs. how much
// gets promoted to Tier 2, plus the accounted-MB cost of each tier.
//
// Two constraints on how this is measured, both inherited from the probes
// above:
//   1. No live sidecar exists in this test binary (`Indexer::new()` hardcodes
//      `jar_sidecar` to `None` under `#[cfg(test)]` — see
//      `scan_handler_tests.rs::indexer_new_jar_phase_is_unavailable_in_tests`),
//      so `build_jar_manifest`'s cache-miss branch (which needs a live
//      sidecar) can never run here, and the real on-disk
//      `jar-manifest-v1.bin` is too fresh post-flip to cover the whole
//      corpus (a few hundred bytes at the time this probe was written).
//      Rather than invent a synthetic manifest, this probe derives EVERY
//      JAR's real Tier-1 manifest names directly from the real,
//      already-cached full symbol data (`jar-symbols-v{VERSION}.bin`, loaded
//      via the same `load_jar_cache()` `library_jar_cache_footprint` already
//      uses above), through the exact same per-symbol mapping
//      `build_jar_manifest`'s own sidecar-response branch uses
//      (name/kind/container/`pkg`→`package`), then feeds it through the
//      real, unmodified `populate_tier1_from_manifest`. This substitutes
//      "already-cached full data" for "a live sidecar response" as the
//      manifest's input — the same substitution `build_jar_manifest`'s own
//      cache-HIT branch makes from a warm manifest cache — and runs it
//      through unmodified production code, not a new measurement mechanism.
//   2. "A handful of hover/completion calls" is simulated by calling the
//      exact two-line pattern every real hover/completion/resolution
//      consumer calls (see `indexer/lookup.rs`/`indexer/resolution.rs`):
//      `jar_qualified_or_bare_has_candidate` then `ensure_jar_materialized`.
//      Driving this through the full LSP request/response machinery would
//      exercise the identical `jar_files`/`jar_definitions` write path for
//      no additional measurement fidelity — `ensure_jar_materialized` IS the
//      chokepoint every one of those consumers calls.
//   3. Which JARs make up that "handful" is deterministic, not an artifact
//      of the `HashMap`'s (randomized, per-process) iteration order: the
//      candidate list is sorted by representative name before the first
//      `TOUCHED_JAR_SAMPLE` are taken. So the headline Tier-1-only
//      percentage below is reproducible run-to-run against the same
//      on-disk `jar-symbols` cache — the number this probe exists to cite
//      in a PR description doesn't move under you between runs.
//
// Run:
// ```text
// cargo test --bin kmp-lsp lazy_jar_loading_tier_split_profile -- --ignored --nocapture
// ```

/// How many distinct compiled JARs to "touch" via a simulated hover/completion
/// call — deliberately small, matching the brief's "a handful." A touch may
/// promote more than one JAR if its representative bare name collides with
/// another JAR's symbol (a real, observable effect of bare-name lookup, not
/// a probe artifact) — the actual promoted count is measured, not assumed.
///
/// The sample is the first `TOUCHED_JAR_SAMPLE` candidates after sorting by
/// representative name, not an arbitrary iteration-order prefix — this keeps
/// the sample (and the resulting headline percentage) deterministic and
/// reproducible across runs against the same on-disk cache.
const TOUCHED_JAR_SAMPLE: usize = 8;

#[test]
#[ignore = "manual memory profiling — measures the lazy-loading win against a real corpus"]
fn lazy_jar_loading_tier_split_profile() {
    let rss_start = vm_rss_bytes();

    // ── Phase 1: load the real corpus and build a real Tier-1 manifest for
    // EVERY jar — this is what the crawl now does eagerly and cheaply for
    // the whole corpus after the flip. ─────────────────────────────────────
    let jar_cache: std::collections::HashMap<String, JarCacheEntry> = load_jar_cache();
    assert!(
        !jar_cache.is_empty(),
        "jar-symbols cache is empty — nothing to measure; is \
         ~/.cache/kmp-lsp/jar-symbols-v*.bin present?"
    );
    let corpus_size = jar_cache.len();
    eprintln!("corpus: {corpus_size} compiled JARs (from the real jar-symbols cache)");

    let indexer = Indexer::new();
    let mut touch_candidates: Vec<(crate::types::JarId, String)> = Vec::with_capacity(corpus_size);
    let mut total_manifest_names = 0usize;
    for (path_str, entry) in &jar_cache {
        let jar_id = indexer.jar_table.intern(path_str);
        let names: Vec<JarManifestName> = entry
            .symbols
            .iter()
            .map(|s| JarManifestName {
                name: s.name.clone(),
                kind: s.kind.clone(),
                container: (!s.container.is_empty()).then(|| s.container.clone()),
                package: (!s.pkg.is_empty()).then(|| s.pkg.clone()),
                extension_receiver: (!s.extension_receiver_type.is_empty()).then(|| {
                    s.extension_receiver_type
                        .split('<')
                        .next()
                        .unwrap_or("")
                        .to_owned()
                }),
            })
            .collect();
        total_manifest_names += populate_tier1_from_manifest(&indexer, jar_id, &names);
        // Representative bare name for the "touch" simulation below: the
        // first symbol's short name, mirroring what a user completing/
        // hovering on a type from this library would type.
        if let Some(representative) = entry.symbols.first().map(|s| s.name.clone()) {
            touch_candidates.push((jar_id, representative));
        }
    }
    // The full-data cache (hundreds of MB) has done its one job — building
    // the cheap manifest — and is dropped here, exactly like
    // `build_jar_manifest` never retains the sidecar's full response past
    // its per-JAR mapping.
    drop(jar_cache);
    trim_heap();
    let rss_after_tier1 = vm_rss_bytes();

    eprintln!(
        "Tier 1 manifest built for all {corpus_size} JARs: {total_manifest_names} names total \
         (jar_bare_names + jar_qualified)"
    );

    // ── Phase 2: simulate a handful of hover/completion calls ─────────────
    // `touch_candidates` was accumulated by iterating `jar_cache`, a
    // `std::collections::HashMap` — its iteration order is randomized
    // per-process (SipHash-seeded) and is NOT stable across runs, even
    // against the exact same on-disk cache. Sort by the representative name
    // first so the same corpus always yields the same first-N "touched"
    // sample (and thus the same headline Tier-1-only percentage) regardless
    // of hash-seed noise.
    touch_candidates.sort_unstable_by(|a, b| a.1.cmp(&b.1));
    let touched: Vec<(crate::types::JarId, String)> = touch_candidates
        .into_iter()
        .take(TOUCHED_JAR_SAMPLE)
        .collect();
    eprintln!(
        "simulating {} hover/completion call(s), touching bare names: {:?}",
        touched.len(),
        touched.iter().map(|(_, n)| n.clone()).collect::<Vec<_>>()
    );
    for (_, name) in &touched {
        // The exact real consumer pattern (`indexer/lookup.rs`,
        // `indexer/resolution.rs`): check for a Tier-1 candidate, then
        // attempt on-demand Tier-2 promotion. No live sidecar is needed —
        // the promotion path (`ensure_jar_materialized` →
        // `materialize_jar_on_demand` → `index_jars`) hits the real, fresh
        // on-disk `jar-symbols` cache entry for this JAR (cache-hit path).
        if indexer.jar_qualified_or_bare_has_candidate(name) {
            ensure_jar_materialized(&indexer, name);
        }
    }
    let rss_after_materialize = vm_rss_bytes();
    trim_heap();
    let rss_after_trim = vm_rss_bytes();

    // ── Tally: Tier-1-only vs Tier-2-materialized ──────────────────────────
    let tier2_count = indexer.materialized.len();
    let failed_count = indexer.materialization_failed.len();
    let tier1_only_count = corpus_size.saturating_sub(tier2_count + failed_count);

    // Tier 1 accounting: jar_bare_names (DashMap<String, Vec<JarId>>) +
    // jar_qualified (DashMap<String, JarId>) — the two maps that ARE Tier 1
    // by design (§Tier 1): always-eager, cheap-by-construction.
    let mut tier1_bare_bytes = 0usize;
    let mut tier1_bare_names = 0usize;
    for e in indexer.jar_bare_names.iter() {
        tier1_bare_names += 1;
        tier1_bare_bytes += STRING_HDR
            + str_bytes(e.key())
            + VEC_HDR
            + e.value().capacity() * std::mem::size_of::<crate::types::JarId>();
    }
    let mut tier1_qualified_bytes = 0usize;
    let mut tier1_qualified_names = 0usize;
    for e in indexer.jar_qualified.iter() {
        tier1_qualified_names += 1;
        tier1_qualified_bytes +=
            STRING_HDR + str_bytes(e.key()) + std::mem::size_of::<crate::types::JarId>();
    }
    let tier1_bytes = tier1_bare_bytes + tier1_qualified_bytes;

    // Tier 2 accounting: mirrors the `jar_bytes` roll-up in
    // `memory_retainer_profile` exactly (jar_files/jar_definitions/
    // jar_symbol_packages/jar_uri_to_defs) — only the `tier2_count` jars this
    // probe actually promoted have any data in these maps.
    let mut tier2_bytes = 0usize;
    let mut tier2_file_count = 0usize;
    let mut tier2_symbol_count = 0usize;
    for e in indexer.jar_files.iter() {
        tier2_file_count += 1;
        let mut junk = FileTally::default();
        account_file(&mut junk, e.key(), true, e.value());
        tier2_symbol_count += junk.n_symbols;
        tier2_bytes += file_tally_total(&junk);
    }
    for e in indexer.jar_definitions.iter() {
        tier2_bytes += STRING_HDR
            + str_bytes(e.key())
            + VEC_HDR
            + e.value().capacity() * std::mem::size_of::<Location>();
        for loc in e.value() {
            tier2_bytes += location_bytes(loc);
        }
    }
    for e in indexer.jar_symbol_packages.iter() {
        tier2_bytes += STRING_HDR + str_bytes(e.key()) + vec_string_bytes(e.value());
    }
    for e in indexer.jar_uri_to_defs.iter() {
        tier2_bytes += STRING_HDR + str_bytes(e.key()) + vec_string_bytes(e.value());
    }
    let tier1_mb = to_mb(tier1_bytes);
    let tier2_mb = to_mb(tier2_bytes);

    eprintln!();
    eprintln!("{:<32} {:>10} {:>14}", "tier", "jars", "accounted MB");
    eprintln!("{}", "-".repeat(58));
    eprintln!(
        "{:<32} {:>10} {:>14.2}",
        "Tier 1 only (manifest)", tier1_only_count, tier1_mb
    );
    eprintln!(
        "{:<32} {:>10} {:>14.2}",
        "Tier 2 materialized (full)", tier2_count, tier2_mb
    );
    if failed_count > 0 {
        eprintln!(
            "{:<32} {:>10} {:>14}",
            "materialization failed", failed_count, "n/a"
        );
    }
    eprintln!("{}", "-".repeat(58));

    eprintln!();
    eprintln!(
        "jars: {tier1_only_count} Tier-1-only, {tier2_count} materialized ({failed_count} \
         failed) out of {corpus_size} total"
    );
    eprintln!(
        "  -> {:.1}% of the {corpus_size}-JAR corpus stayed Tier-1-only",
        100.0 * tier1_only_count as f64 / corpus_size.max(1) as f64
    );
    eprintln!(
        "Tier 1 accounted: {tier1_mb:.1} MB  ({tier1_bare_names} bare-name entries, \
         {tier1_qualified_names} FQN entries, {total_manifest_names} manifest names total)"
    );
    eprintln!(
        "Tier 2 accounted: {tier2_mb:.1} MB  ({tier2_file_count} files, \
         {tier2_symbol_count} symbols)"
    );
    eprintln!();
    eprintln!(
        "RSS before:                    {:>8.1} MB",
        to_mb(rss_start)
    );
    eprintln!(
        "RSS after Tier-1 build+trim:   {:>8.1} MB  (Δ = {:.1} MB)",
        to_mb(rss_after_tier1),
        to_mb(rss_after_tier1.saturating_sub(rss_start))
    );
    eprintln!(
        "RSS after materialize (peak):  {:>8.1} MB",
        to_mb(rss_after_materialize)
    );
    eprintln!(
        "RSS after materialize+trim:    {:>8.1} MB  (Δ vs before = {:.1} MB)",
        to_mb(rss_after_trim),
        to_mb(rss_after_trim.saturating_sub(rss_start))
    );
    let rss_delta_mb = to_mb(rss_after_trim.saturating_sub(rss_start));
    eprintln!(
        "RSS delta: {rss_delta_mb:.1} MB (compare against the 560.4 MB baseline from PR #213's \
         fully-eager measurement)"
    );

    assert_eq!(
        tier1_only_count + tier2_count + failed_count,
        corpus_size,
        "every jar must be accounted for exactly once across the three buckets"
    );
    assert!(
        tier2_count > 0,
        "the simulated touches should have materialized at least one JAR"
    );
    assert!(
        tier1_only_count > 0,
        "the whole thesis is that most of the corpus stays Tier-1-only — if this \
         is 0, either the corpus is tiny or something over-promoted"
    );

    drop(indexer);
}
