//! Shared test-only fixtures used by more than one test module under
//! `resolver/` — kept here instead of duplicated so a Gradle-cache-layout
//! change only needs updating once.

use tower_lsp::lsp_types::Url;

/// A Gradle-cache-shaped `jar:` URI for `(group, artifact, version)`, mirroring
/// the real layout `crate::cli::extract_sources::parse_jar_meta` parses:
/// `.../modules-2/files-2.1/<group>/<artifact>/<version>/<hash>/<file>.jar`.
pub(crate) fn gradle_cache_jar_uri(group: &str, artifact: &str, version: &str) -> Url {
    Url::parse(&format!(
        "jar:file:///home/user/.gradle/caches/modules-2/files-2.1/{group}/{artifact}/{version}/deadbeef/{artifact}-{version}.jar"
    ))
    .unwrap()
}
