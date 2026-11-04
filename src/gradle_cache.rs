//! Gradle module-cache vocabulary: artifact coordinates parsed from a cache
//! path, version ordering, and the cache's home directory. Shared by the
//! indexer, the resolver and the `extract-sources` command, so it sits below
//! all of them.

use std::path::{Path, PathBuf};

// ── version comparison ────────────────────────────────────────────────────────

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum VersionPart {
    Numeric(u64),
    Text(String),
}

pub(crate) fn version_key(version: &str) -> Vec<VersionPart> {
    version
        .split(['.', '-'])
        .map(|p| match p.parse::<u64>() {
            Ok(n) => VersionPart::Numeric(n),
            Err(_) => VersionPart::Text(p.to_owned()),
        })
        .collect()
}

// ── Gradle cache path parsing ─────────────────────────────────────────────────

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) struct GradleMeta {
    pub(crate) group: String,
    pub(crate) artifact: String,
    pub(crate) version: String,
}

/// Parse `(group, artifact, version)` from a Gradle module cache path.
///
/// Gradle cache layout:
/// `<home>/caches/modules-2/files-2.1/<group>/<artifact>/<version>/<hash>/<file>`
pub(crate) fn parse_jar_meta(jar: &Path) -> Option<GradleMeta> {
    let s = jar.to_string_lossy();
    let idx = s.find("files-2.1")? + "files-2.1".len();
    let rest = s[idx..].trim_start_matches(['/', '\\']);
    let parts: Vec<&str> = rest.splitn(5, ['/', '\\']).collect();
    if parts.len() < 3 {
        return None;
    }
    Some(GradleMeta {
        group: parts[0].to_owned(),
        artifact: parts[1].to_owned(),
        version: parts[2].to_owned(),
    })
}

pub(crate) fn default_gradle_home() -> PathBuf {
    std::env::var("GRADLE_USER_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            crate::util::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".gradle")
        })
}
