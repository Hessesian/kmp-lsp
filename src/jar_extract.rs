//! Lazy extraction of JAR/sources-JAR entries to disk for go-to-definition.
//!
//! Auto-mounted library sources are indexed in-memory and exposed under
//! `jar:file://…!/Foo.kt` URIs. Hover/completion work from the index, but go-def
//! hands the *editor* a URI to open, and editors only open `file://`. So when a
//! go-def target is a `jar:` entry, we extract just that one zip entry to a
//! read-only cache file and return a `file://` Location (same range) the editor
//! can actually navigate into. Compiled-only JAR URIs (no `!/entry`) have no
//! source to extract and are left unchanged.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};

use tower_lsp::lsp_types::{GotoDefinitionResponse, Location, LocationLink, Url};

use crate::indexer::Indexer;

/// Parse a `jar:file://<jar>!/<entry>` URI into `(jar_path, entry)`.
/// Returns `None` for non-`jar:` URIs and for compiled-only JAR URIs that carry
/// no `!/entry` (nothing to extract).
pub(crate) fn parse_jar_entry_uri(uri: &str) -> Option<(PathBuf, String)> {
    let rest = uri.strip_prefix("jar:")?;
    let (jar_part, entry) = rest.split_once("!/")?;
    if entry.is_empty() || is_unsafe_entry(entry) {
        return None;
    }
    // Reuse `Url::to_file_path` so percent-encoding in the path is decoded.
    let jar_path = Url::parse(jar_part).ok()?.to_file_path().ok()?;
    Some((jar_path, entry.to_owned()))
}

/// Directory under which jar entries are extracted (`~/.cache/kmp-lsp/jar-sources`).
fn jar_sources_cache_dir() -> PathBuf {
    crate::indexer::xdg_cache_base()
        .join("kmp-lsp")
        .join("jar-sources")
}

/// Whether `uri` points at a file we extracted from a jar. Such files must not be
/// re-indexed as workspace documents when the editor opens them — their symbols are
/// already in the index under the original `jar:` URI, so indexing the extracted
/// `file://` copy too would duplicate every library definition.
pub(crate) fn is_extracted_jar_source(uri: &Url) -> bool {
    uri.to_file_path()
        .map(|path| path.starts_with(jar_sources_cache_dir()))
        .unwrap_or(false)
}

/// Reject entry paths that could escape the extraction cache dir when joined:
/// absolute paths, Windows drive letters, and any `..` traversal segment.
fn is_unsafe_entry(entry: &str) -> bool {
    entry.starts_with('/')
        || entry.starts_with('\\')
        || (entry.len() >= 2 && entry.as_bytes()[1] == b':') // C:\…
        || entry.split(['/', '\\']).any(|segment| segment == "..")
}

/// Extract `entry` from `jar_path` into the on-disk cache and return a `file://`
/// `Url` for the extracted (read-only) file. Idempotent: a file already present
/// for the jar's current `(mtime, size)` is reused. Returns `None` on any
/// I/O / zip error or if the entry is absent.
pub(crate) fn extract_jar_entry_to_disk(jar_path: &Path, entry: &str) -> Option<Url> {
    // Defensive: never let an entry path escape the cache dir, even if a caller
    // passes something other than a zip-validated name.
    if entry.is_empty() || is_unsafe_entry(entry) {
        return None;
    }
    let metadata = std::fs::metadata(jar_path).ok()?;
    let mtime_secs = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut hasher = DefaultHasher::new();
    jar_path.hash(&mut hasher);
    mtime_secs.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    let fingerprint = hasher.finish();

    let stem = jar_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("jar");
    let dest = jar_sources_cache_dir()
        .join(format!("{stem}-{fingerprint:016x}"))
        .join(entry);

    if dest.is_file() {
        return Url::from_file_path(&dest).ok();
    }

    let file = std::fs::File::open(jar_path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut zip_entry = archive.by_name(entry).ok()?;
    let mut bytes = Vec::new();
    zip_entry.read_to_end(&mut bytes).ok()?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    std::fs::write(&dest, &bytes).ok()?;
    set_read_only(&dest);
    Url::from_file_path(&dest).ok()
}

/// Rewrite any `jar:…!/entry` definition target to a `file://` one backed by an
/// extracted on-disk file. Non-`jar:` / unextractable targets pass through.
pub(crate) fn rewrite_jar_definitions(
    indexer: &Indexer,
    response: GotoDefinitionResponse,
) -> GotoDefinitionResponse {
    match response {
        GotoDefinitionResponse::Scalar(location) => {
            GotoDefinitionResponse::Scalar(rewrite_location(indexer, location))
        }
        GotoDefinitionResponse::Array(locations) => GotoDefinitionResponse::Array(
            locations
                .into_iter()
                .map(|location| rewrite_location(indexer, location))
                .collect(),
        ),
        GotoDefinitionResponse::Link(links) => GotoDefinitionResponse::Link(
            links
                .into_iter()
                .map(|link| rewrite_link(indexer, link))
                .collect(),
        ),
    }
}

fn rewrite_location(indexer: &Indexer, location: Location) -> Location {
    match rewrite_jar_uri(indexer, &location.uri) {
        Some(file_uri) => Location {
            uri: file_uri,
            range: location.range,
        },
        None => location,
    }
}

fn rewrite_link(indexer: &Indexer, mut link: LocationLink) -> LocationLink {
    if let Some(file_uri) = rewrite_jar_uri(indexer, &link.target_uri) {
        link.target_uri = file_uri;
    }
    link
}

/// Extract the `jar:` URI's entry and return the `file://` URL, registering it
/// as a library URI so references/indexing treat it as read-only library source.
fn rewrite_jar_uri(indexer: &Indexer, uri: &Url) -> Option<Url> {
    if uri.scheme() != "jar" {
        return None;
    }
    let (jar_path, entry) = parse_jar_entry_uri(uri.as_str())?;
    let file_uri = extract_jar_entry_to_disk(&jar_path, &entry)?;
    indexer.library_uris.insert(file_uri.as_str().to_owned());
    Some(file_uri)
}

#[cfg(unix)]
fn set_read_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444));
}

#[cfg(not(unix))]
fn set_read_only(_path: &Path) {}

#[cfg(test)]
#[path = "jar_extract_tests.rs"]
mod tests;
