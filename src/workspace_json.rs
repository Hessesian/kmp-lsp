//! Auto-discovery of source roots from `workspace.json`.
//!
//! `workspace.json` is produced by JetBrains Gradle/Maven plugins and describes
//! project structure (modules, content roots, source directories). When the file
//! exists at the workspace root we extract every non-resource source root so the
//! indexer covers them without manual `sourcePaths` configuration.
//!
//! Placeholder substitution:
//! - `<WORKSPACE>` → absolute workspace root path
//! - `<MAVEN_REPO>` → skipped (library jars are not indexed)
//!
//! Source root types we index:
//! - `"java-source"` — production Kotlin/Java sources
//! - `"java-test"` — test Kotlin/Java sources

use crate::cli::extract_sources::GradleMeta;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const SOURCE_TYPES: &[&str] = &["java-source", "java-test"];
const WORKSPACE_PLACEHOLDER: &str = "<WORKSPACE>";
/// Prefix of the real, decades-old IntelliJ Gradle-synced library naming
/// convention (`library_gradle_meta_from_name`'s fallback GAV parse).
const GRADLE_LIBRARY_NAME_PREFIX: &str = "Gradle: ";

#[derive(Deserialize)]
struct WorkspaceData {
    #[serde(default)]
    modules: Vec<ModuleData>,
    /// Every dependency-resolvable library in the project, keyed by `name`
    /// from each module's `dependencies[]` entries — see `load_module_dependencies`.
    #[serde(default)]
    libraries: Vec<LibraryData>,
    /// Optional list of external library source directories.
    /// When present (even as `[]`), these override the global `~/.kmp-lsp/sources` default.
    /// Supports the `<WORKSPACE>` placeholder (substituted with the workspace root path).
    #[serde(default, rename = "sourcePaths")]
    source_paths: Option<Vec<String>>,
    /// Optional list of compiled `.jar`/`.aar` files — or directories containing them —
    /// to index for library symbols. For projects without a Gradle cache (Make/Bazel/
    /// manual builds), where the automatic Gradle-cache scan finds nothing.
    /// Supports `<WORKSPACE>`; relative paths resolve against the workspace root.
    #[serde(default, rename = "jarPaths")]
    jar_paths: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct ModuleData {
    /// Parsed for schema fidelity; module identity for dependency-scoping
    /// purposes keys off `content_roots[].path`, not this name, so it has no
    /// reader in this slice — see `load_module_dependencies`.
    #[allow(dead_code)]
    #[serde(default)]
    name: String,
    #[serde(default, rename = "contentRoots")]
    content_roots: Vec<ContentRootData>,
    #[serde(default)]
    dependencies: Vec<DependencyData>,
    /// Parsed for schema fidelity; module identity for dependency-scoping
    /// purposes keys off `content_roots[].path` (see `load_module_dependencies`),
    /// not this Gradle-project-path field, so it has no reader in this slice.
    #[allow(dead_code)]
    #[serde(default, rename = "externalProjectId")]
    external_project_id: Option<String>,
}

/// One entry of a module's `dependencies[]` list, internally tagged on
/// `"type"` — the real schema's own discriminator shape (verified against
/// `Kotlin/kotlin-lsp`'s `model.kt`). A `#[serde(other)]` catch-all covers a
/// future/unrecognized `type` value so parsing degrades to "entry ignored,"
/// never a hard error — matching this file's existing "never panic on
/// malformed input" convention.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum DependencyData {
    Library {
        name: String,
        /// Parsed for schema fidelity; dependency scope (compile/test/
        /// runtime/provided) doesn't yet filter GAV resolution in this slice.
        #[allow(dead_code)]
        #[serde(default)]
        scope: Option<String>,
    },
    /// An intra-project module-to-module dependency edge. Modeled so a real
    /// file carrying one deserializes correctly, but no logic in this slice
    /// reads it — wiring project-graph awareness from this variant is out of
    /// scope here (see design doc's Non-goals).
    Module {
        #[allow(dead_code)]
        name: String,
    },
    InheritedSdk,
    ModuleSource,
    /// A JDK/SDK dependency entry, e.g. `{"type":"sdk","name":"17","kind":"jdk"}`.
    /// Modeled so it is never misinterpreted as a `Library` entry during GAV
    /// resolution; its fields have no reader in this slice.
    #[allow(dead_code)]
    Sdk {
        name: String,
        kind: String,
    },
    #[serde(other)]
    Unrecognized,
}

#[derive(Deserialize)]
struct ContentRootData {
    #[serde(default)]
    path: String,
    #[serde(default, rename = "sourceRoots")]
    source_roots: Vec<SourceRootData>,
}

#[derive(Deserialize)]
struct SourceRootData {
    path: String,
    #[serde(rename = "type", default)]
    root_type: String,
}

/// A project-wide library entry from `libraries[]`, referenced by name from
/// each module's `Library`-typed `dependencies[]` entries.
#[derive(Debug, Deserialize)]
pub(crate) struct LibraryData {
    name: String,
    /// Reserved for a future path-based JAR match (design doc §2 point 3:
    /// an indexed JAR's path can equal a root's `path` after placeholder
    /// substitution, sidestepping GAV parsing) — not consumed by the
    /// GAV-string resolution this slice implements. The candidate-side
    /// cross-check this slice *does* implement
    /// (`resolver::resolve::candidate_gradle_meta`) goes the other direction:
    /// it derives GAV straight from a hierarchy-walk candidate's own JAR path
    /// via `parse_jar_meta`, without needing this field at all.
    #[allow(dead_code)]
    #[serde(default)]
    roots: Vec<LibraryRootData>,
    #[serde(default)]
    properties: Option<LibraryProperties>,
}

/// One `roots[]` entry of a `LibraryData`. Parsed for schema fidelity and the
/// future path-based matching noted on `LibraryData::roots`; unread by this
/// slice's GAV-string-based resolution.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct LibraryRootData {
    path: String,
    #[serde(rename = "type", default = "default_library_root_type")]
    root_type: String,
}

fn default_library_root_type() -> String {
    "CLASSES".to_owned()
}

#[derive(Debug, Deserialize)]
struct LibraryProperties {
    #[serde(default)]
    attributes: Option<LibraryAttributes>,
}

#[derive(Debug, Deserialize)]
struct LibraryAttributes {
    #[serde(default, rename = "groupId")]
    group_id: Option<String>,
    #[serde(default, rename = "artifactId")]
    artifact_id: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

/// Reads `<workspace_root>/workspace.json` and returns source root paths.
///
/// Returns an empty `Vec` (with a log warning) if the file is missing, malformed,
/// or contains no eligible source roots — never panics.
pub(crate) fn load_source_paths(workspace_root: &Path) -> Vec<PathBuf> {
    let json_path = workspace_root.join("workspace.json");
    if !json_path.exists() {
        return Vec::new();
    }

    let content = match std::fs::read_to_string(&json_path) {
        Ok(c) => c,
        Err(error) => {
            log::warn!("workspace.json: failed to read: {error}");
            return Vec::new();
        }
    };

    let data: WorkspaceData = match serde_json::from_str(&content) {
        Ok(d) => d,
        Err(error) => {
            log::warn!("workspace.json: failed to parse: {error}");
            return Vec::new();
        }
    };

    let workspace_str = workspace_root.to_string_lossy();
    let mut paths: Vec<PathBuf> = Vec::new();

    for module in &data.modules {
        for content_root in &module.content_roots {
            for source_root in &content_root.source_roots {
                if !SOURCE_TYPES.contains(&source_root.root_type.as_str()) {
                    continue;
                }
                let resolved = source_root
                    .path
                    .replace(WORKSPACE_PLACEHOLDER, &workspace_str);
                let path = PathBuf::from(resolved);
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
    }

    log::info!(
        "workspace.json: auto-discovered {} source roots",
        paths.len()
    );
    paths
}

/// Reads the `sourcePaths` key from `<workspace_root>/workspace.json`.
///
/// Returns `Some(paths)` when the key is present (even if the list is empty —
/// an empty list is an explicit "use no library sources").  Returns `None` when
/// the file is absent or the key is not present, so callers can fall back to
/// the global `~/.kmp-lsp/sources` default.
pub(crate) fn load_configured_source_paths(workspace_root: &Path) -> Option<Vec<PathBuf>> {
    let json_path = workspace_root.join("workspace.json");
    if !json_path.exists() {
        return None;
    }

    let content = match std::fs::read_to_string(&json_path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!(
                "Failed to read workspace.json at {}: {e}",
                json_path.display()
            );
            return None;
        }
    };
    let data: WorkspaceData = match serde_json::from_str(&content) {
        Ok(d) => d,
        Err(e) => {
            log::warn!(
                "Failed to parse workspace.json at {}: {e}",
                json_path.display()
            );
            return None;
        }
    };

    // `None` means key was absent → caller applies global default.
    // `Some([])` means key present but empty → explicit "no library sources".
    let source_paths = data.source_paths?;

    let workspace_str = workspace_root.to_string_lossy();
    let paths = source_paths
        .iter()
        .map(|p| PathBuf::from(p.replace(WORKSPACE_PLACEHOLDER, &workspace_str)))
        .collect();

    Some(paths)
}

pub(crate) fn load_configured_jar_paths(workspace_root: &Path) -> Vec<PathBuf> {
    let json_path = workspace_root.join("workspace.json");
    if !json_path.exists() {
        return Vec::new();
    }
    let content = match std::fs::read_to_string(&json_path) {
        Ok(text) => text,
        Err(error) => {
            log::warn!("workspace.json: failed to read for jarPaths: {error}");
            return Vec::new();
        }
    };
    let data: WorkspaceData = match serde_json::from_str(&content) {
        Ok(parsed) => parsed,
        Err(error) => {
            log::warn!("workspace.json: failed to parse for jarPaths: {error}");
            return Vec::new();
        }
    };
    match data.jar_paths {
        Some(raw) => resolve_jar_path_specs(&raw, workspace_root),
        None => Vec::new(),
    }
}

/// Reads and parses `<workspace_root>/workspace.json` into the full
/// `WorkspaceData` structure. Returns `None` (with a log warning, except for
/// the common "no file present" case) on any I/O or parse failure — never
/// panics, matching every other loader in this file.
///
/// Used by `load_module_dependencies` (workspace scan and CLI indexing) and
/// directly by tests.
fn parse_workspace_data(workspace_root: &Path) -> Option<WorkspaceData> {
    let json_path = workspace_root.join("workspace.json");
    if !json_path.exists() {
        return None;
    }
    let content = match std::fs::read_to_string(&json_path) {
        Ok(text) => text,
        Err(error) => {
            log::warn!("workspace.json: failed to read: {error}");
            return None;
        }
    };
    match serde_json::from_str(&content) {
        Ok(data) => Some(data),
        Err(error) => {
            log::warn!("workspace.json: failed to parse: {error}");
            None
        }
    }
}

/// Resolves a `LibraryData` entry to its Gradle group/artifact/version, in
/// priority order: structured `properties.attributes` first, then a
/// `"Gradle: <group>:<artifact>:<version>"`-shaped `name` fallback. Returns
/// `None` (never a wrong guess) when neither shape matches — e.g. a
/// hand-added local library.
///
/// Used by `module_gradle_dependencies` as part of `load_module_dependencies`,
/// and directly by tests.
pub(crate) fn library_gradle_meta(library: &LibraryData) -> Option<GradleMeta> {
    if let Some(gradle_meta) = library_gradle_meta_from_attributes(library) {
        return Some(gradle_meta);
    }
    library_gradle_meta_from_name(&library.name)
}

/// Structured GAV resolution from `properties.attributes`. Requires all
/// three of `groupId`/`artifactId`/`version` to be present — a partial
/// attribute set is treated the same as absent, never guessed at.
fn library_gradle_meta_from_attributes(library: &LibraryData) -> Option<GradleMeta> {
    let attributes = library.properties.as_ref()?.attributes.as_ref()?;
    Some(GradleMeta {
        group: attributes.group_id.clone()?,
        artifact: attributes.artifact_id.clone()?,
        version: attributes.version.clone()?,
    })
}

/// Fallback GAV resolution by parsing the real, decades-old IntelliJ
/// `"Gradle: <group>:<artifact>:<version>"` library-name convention. Returns
/// `None` if the prefix is absent or the remainder isn't exactly three
/// colon-separated segments — including the third-party plugin's
/// `"Gradle: "`-prefixed synthetic Android SDK entry, which is real-shaped
/// despite being synthetic and so parses like any other library name.
fn library_gradle_meta_from_name(name: &str) -> Option<GradleMeta> {
    let coordinate = name.strip_prefix(GRADLE_LIBRARY_NAME_PREFIX)?;
    let segments: Vec<&str> = coordinate.split(':').collect();
    let [group, artifact, version] = segments[..] else {
        return None;
    };
    Some(GradleMeta {
        group: group.to_owned(),
        artifact: artifact.to_owned(),
        version: version.to_owned(),
    })
}

/// Builds a per-module dependency map: every content-root directory a module
/// owns (from its own `contentRoots[].path`, after `<WORKSPACE>`
/// substitution) is associated with the `GradleMeta` set resolved from that
/// module's `library`-typed `dependencies[]` entries. Non-`Library` entries
/// (`Module`/`InheritedSdk`/`ModuleSource`/`Sdk`) are skipped by construction
/// — matching on the `Library` arm makes passing a JDK-version string or a
/// module name through GAV resolution impossible, not merely discouraged.
///
/// Returns an empty map (never panics) when `workspace.json` is missing,
/// malformed, or carries no `libraries[]`/`dependencies[]` data.
///
/// Called from the workspace scan handler and the CLI index build path to
/// populate `Indexer::module_dependencies` for the resolver's
/// JAR-collision-scoping tie-break.
pub(crate) fn load_module_dependencies(
    workspace_root: &Path,
) -> HashMap<PathBuf, HashSet<GradleMeta>> {
    let Some(data) = parse_workspace_data(workspace_root) else {
        return HashMap::new();
    };

    let libraries_by_name: HashMap<&str, &LibraryData> = data
        .libraries
        .iter()
        .map(|library| (library.name.as_str(), library))
        .collect();

    let workspace_str = workspace_root.to_string_lossy();
    let mut dependencies_by_content_root: HashMap<PathBuf, HashSet<GradleMeta>> = HashMap::new();

    for module in &data.modules {
        let module_dependencies = module_gradle_dependencies(module, &libraries_by_name);
        if module_dependencies.is_empty() {
            continue;
        }
        for content_root in &module.content_roots {
            if content_root.path.is_empty() {
                continue;
            }
            let resolved = content_root
                .path
                .replace(WORKSPACE_PLACEHOLDER, &workspace_str);
            dependencies_by_content_root
                .entry(PathBuf::from(resolved))
                .or_default()
                .extend(module_dependencies.iter().cloned());
        }
    }

    dependencies_by_content_root
}

/// Resolves a single module's `Library`-typed `dependencies[]` entries to
/// their `GradleMeta`, looking each dependency's `name` up in the workspace's
/// project-wide `libraries[]` registry.
fn module_gradle_dependencies(
    module: &ModuleData,
    libraries_by_name: &HashMap<&str, &LibraryData>,
) -> HashSet<GradleMeta> {
    let mut module_dependencies = HashSet::new();
    for dependency in &module.dependencies {
        let DependencyData::Library { name, .. } = dependency else {
            continue;
        };
        let Some(library) = libraries_by_name.get(name.as_str()) else {
            continue;
        };
        if let Some(gradle_meta) = library_gradle_meta(library) {
            module_dependencies.insert(gradle_meta);
        }
    }
    module_dependencies
}

/// Resolve a list of jar/aar path specs into concrete compiled-jar files.
///
/// Each spec may be a single `.jar`/`.aar` file or a directory (recursively
/// expanded to its `.jar`/`.aar` files). `<WORKSPACE>` is substituted with the
/// root; relative paths resolve against the root. `*-sources.jar` / `*-javadoc.jar`
/// are excluded — those are not compiled symbol jars (KDoc is read separately by
/// the sidecar from a sibling sources jar).
///
/// Shared by `workspace.json`'s `jarPaths` and the LSP `indexingOptions.jarPaths`.
pub(crate) fn resolve_jar_path_specs(specs: &[String], workspace_root: &Path) -> Vec<PathBuf> {
    let workspace_str = workspace_root.to_string_lossy();
    let mut out: Vec<PathBuf> = Vec::new();
    for spec in specs {
        let resolved = spec.replace(WORKSPACE_PLACEHOLDER, &workspace_str);
        let path = {
            let resolved_path = PathBuf::from(&resolved);
            if resolved_path.is_absolute() {
                resolved_path
            } else {
                workspace_root.join(resolved_path)
            }
        };
        if path.is_dir() {
            collect_compiled_jars(&path, &mut out);
        } else if path.is_file() && is_compiled_jar(&path) && !out.contains(&path) {
            out.push(path);
        } else if !path.exists() {
            log::warn!("jarPaths: configured path not found: {}", path.display());
        }
    }
    out
}

/// Whether `path` is a compiled jar/aar (excludes sources/javadoc jars).
/// Suffix-based so a legitimately-named jar like `my-sources-helper.jar` is kept.
fn is_compiled_jar(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    (name.ends_with(".jar") || name.ends_with(".aar"))
        && !name.ends_with("-sources.jar")
        && !name.ends_with("-javadoc.jar")
}

/// Recursively collect compiled `.jar`/`.aar` files under `dir`. Only descends
/// into *real* subdirectories (via `DirEntry::file_type`, which does not follow
/// symlinks) so a symlink cycle can't cause unbounded recursion.
fn collect_compiled_jars(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // `DirEntry::file_type` does not traverse symlinks, so a symlinked dir
        // reports as a symlink (not a dir) and is skipped — no cycle recursion.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            collect_compiled_jars(&path, out);
        } else if file_type.is_file() && is_compiled_jar(&path) && !out.contains(&path) {
            out.push(path);
        }
    }
}
///
/// Activates when a build file (`build.gradle.kts`, `build.gradle`, `pom.xml`, …) exists
/// at the workspace root. Probes well-known source directories; returns only those that
/// actually exist on disk so the indexer never spins on empty paths.
///
/// Multi-module Gradle: `settings.gradle(.kts)` is parsed for `include(":module")` calls;
/// each listed module is treated as a subproject and its standard source dirs are probed.
/// Nested module paths (e.g. `":features:play-domain"`) are supported; colons are replaced
/// with the OS path separator so the result is a valid relative directory path.
///
/// Probed layout: every immediate child of `src/` that contains a `kotlin/` or `java/`
/// subdirectory is treated as a source root. This covers plain Gradle/Maven
/// (`src/main/kotlin`, `src/test/java`), every standard KMP source set (`commonMain`,
/// `androidMain`, `iosMain`, `jvmMain`, `wasmJsMain`, …), and any user-defined source
/// set without requiring an allowlist update.
///
/// These paths are typically already covered by the workspace root scan, but listing them
/// explicitly ensures consistent indexing when the workspace root is set to a parent dir.
pub(crate) fn detect_build_layout_source_paths(workspace_root: &Path) -> Vec<PathBuf> {
    let has_gradle = ["build.gradle.kts", "build.gradle"]
        .iter()
        .any(|f| workspace_root.join(f).exists());
    let has_settings = ["settings.gradle.kts", "settings.gradle"]
        .iter()
        .any(|f| workspace_root.join(f).exists());
    let has_maven = workspace_root.join("pom.xml").exists();

    if !has_gradle && !has_settings && !has_maven {
        return Vec::new();
    }

    let mut roots: Vec<PathBuf> = Vec::new();

    // Subproject dirs from settings.gradle(.kts)
    let subprojects = settings_subprojects(workspace_root);

    // Probe candidates for each directory scope.
    let scan_dirs: Vec<PathBuf> = if subprojects.is_empty() {
        vec![workspace_root.to_owned()]
    } else {
        subprojects.iter().map(|s| workspace_root.join(s)).collect()
    };

    // Always include the root itself (root build.gradle may have sources too).
    let mut all_dirs = vec![workspace_root.to_owned()];
    for d in &scan_dirs {
        if d != workspace_root && !all_dirs.contains(d) {
            all_dirs.push(d.clone());
        }
    }

    for dir in &all_dirs {
        for path in probe_source_set_roots(dir) {
            if !roots.contains(&path) {
                roots.push(path);
            }
        }
    }

    if !roots.is_empty() {
        log::info!("build-layout: auto-discovered {} source roots", roots.len());
    }
    roots
}

/// Returns every `src/<set>/kotlin` and `src/<set>/java` directory under `module_dir`.
///
/// Discovery is structural: any child of `src/` that has a `kotlin/` or `java/` subdir
/// is treated as a source root. Catches plain layouts (`src/main/kotlin`, `src/test/java`),
/// all standard KMP source sets, and user-defined sets without an allowlist.
fn probe_source_set_roots(module_dir: &Path) -> Vec<PathBuf> {
    const SOURCE_LANG_DIRS: &[&str] = &["kotlin", "java"];
    let src = module_dir.join("src");
    let Ok(entries) = std::fs::read_dir(&src) else {
        return Vec::new();
    };

    let mut sets: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    sets.sort();

    let mut roots = Vec::new();
    for set_dir in sets {
        for lang in SOURCE_LANG_DIRS {
            let candidate = set_dir.join(lang);
            if candidate.is_dir() {
                roots.push(candidate);
            }
        }
    }
    roots
}

/// Extracts subproject directory names from `settings.gradle` / `settings.gradle.kts`.
///
/// Handles both forms:
/// - `include(":app", ":core")` — Gradle convention (colon prefix)
/// - `include("app", "core")` — variant without colon
/// - Nested: `include(":feature:login")` → maps to `feature/login`
fn settings_subprojects(workspace_root: &Path) -> Vec<String> {
    for filename in &["settings.gradle.kts", "settings.gradle"] {
        let path = workspace_root.join(filename);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        return parse_include_calls(&content);
    }
    Vec::new()
}

/// Parses `include("...", "...")` calls and returns directory paths.
///
/// Handles both double- and single-quoted project names, and both Kotlin DSL
/// (`include(":app")`) and Groovy (`include ':app'`) styles. Lines starting
/// with `includeBuild` or `includeFlat` are intentionally ignored.
fn parse_include_calls(content: &str) -> Vec<String> {
    let mut result = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // Only match `include(` — reject `includeBuild(`, `includeFlat(`, etc.
        if !trimmed.starts_with("include(") {
            continue;
        }
        // Extract all single- or double-quoted strings on this line.
        let mut chars = trimmed.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '"' || c == '\'' {
                let quote = c;
                let token: String = chars.by_ref().take_while(|&d| d != quote).collect();
                // ":app" → "app", ":feature:login" → "feature/login"
                let dir = token
                    .trim_start_matches(':')
                    .replace(':', std::path::MAIN_SEPARATOR_STR);
                if !dir.is_empty() && !result.contains(&dir) {
                    result.push(dir);
                }
            }
        }
    }
    result
}

/// Auto-detect Android SDK source directories.
///
/// Checks, in order:
/// 1. `sdk.dir` property in `<workspace_root>/local.properties`
/// 2. `$ANDROID_HOME` environment variable
/// 3. `$ANDROID_SDK_ROOT` environment variable
///
/// When an SDK directory is found, returns the highest API-level
/// `sources/android-XX` subdirectory that exists on disk, which
/// contains the Android platform Java sources.  Returns an empty `Vec`
/// when no SDK is found or the SDK has no `sources/` directory.
pub(crate) fn detect_android_sdk_source_paths(workspace_root: &Path) -> Vec<PathBuf> {
    let Some(sdk) = resolve_android_sdk_root(workspace_root) else {
        return Vec::new();
    };

    let sources_root = sdk.join("sources");
    if !sources_root.is_dir() {
        return Vec::new();
    }

    // Find highest android-XX API level present under sdk/sources/.
    let best = highest_api_level_dir(&sources_root);

    match best {
        Some(path) => {
            log::info!("android-sdk: auto-detected sources at {}", path.display());
            vec![path]
        }
        None => Vec::new(),
    }
}

/// Auto-detect the Android platform's compiled `android.jar`.
///
/// Uses the same SDK-root discovery as `detect_android_sdk_source_paths`
/// (`local.properties`' `sdk.dir`, then `$ANDROID_HOME`, then
/// `$ANDROID_SDK_ROOT`), but returns the highest `platforms/android-XX/
/// android.jar` present — a different subdirectory of the SDK than
/// `sources/android-XX`, since a developer can have a platform installed
/// without its optional sources component, or vice versa. Returns an empty
/// `Vec` when no SDK, no `platforms/` directory, or no `android.jar` inside
/// any `android-XX` platform directory is found.
pub(crate) fn detect_android_sdk_jar_path(workspace_root: &Path) -> Vec<PathBuf> {
    let Some(sdk) = resolve_android_sdk_root(workspace_root) else {
        return Vec::new();
    };

    let platforms_root = sdk.join("platforms");
    if !platforms_root.is_dir() {
        return Vec::new();
    }

    // Find the highest android-XX API level whose android.jar actually
    // exists on disk — a platform directory can exist without the JAR in a
    // partial SDK install, so presence must be checked, not assumed.
    let best = std::fs::read_dir(&platforms_root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().ok().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let api = parse_android_api_level(&e.file_name().to_string_lossy())?;
            let jar = e.path().join("android.jar");
            jar.is_file().then_some((api, jar))
        })
        .max_by_key(|(api, _)| *api)
        .map(|(_, jar)| jar);

    match best {
        Some(path) => {
            log::info!(
                "android-sdk: auto-detected compiled JAR at {}",
                path.display()
            );
            vec![path]
        }
        None => Vec::new(),
    }
}

/// Real AGP task-output directories that produce a module's own `R.jar` —
/// `compile_r_class_jar` for library modules, `compile_and_runtime_r_class_jar`
/// for application/test modules. Neither has one fixed variant subdirectory
/// name across a real project: a custom product-flavor setup produces names
/// like `tst1Debug`/`ppeDebug`/`prodDebug` instead of plain `debug`/`release`
/// (real, observed on a production multi-flavor app) — [`find_r_class_jar_for_module`]
/// globs every variant under both task dirs rather than assuming one name.
const R_CLASS_JAR_TASK_DIRS: &[&str] = &["compile_r_class_jar", "compile_and_runtime_r_class_jar"];

/// Locates each Gradle module's own AAPT-generated `R.jar` (the module's
/// resource-symbol class — `R`, plus its real nested classes `R$string`,
/// `R$drawable`, `R$id`, … since PR #301 indexes those correctly) under its
/// `build/` output, so `R.string.foo`-style references resolve like any
/// other JAR type.
///
/// `R.jar` is never a Gradle dependency — it never lands in the shared
/// Gradle cache [`scan_gradle_jars`] scans — and AGP regenerates it per
/// module, per build variant, under an unpredictable variant-named
/// subdirectory (see [`R_CLASS_JAR_TASK_DIRS`]'s doc). Picks at most ONE
/// variant's `R.jar` per module (preferring one whose variant name contains
/// "debug", else whichever is found first): a real, measured, and
/// acceptable trade-off — resource NAMES don't change with build variant
/// for the same module (an id's underlying numeric VALUE does, but that has
/// no bearing on name resolution, all this benchmark/goto-definition needs).
///
/// Reuses [`settings_subprojects`] for module identity — the same
/// `include(":module")` parsing [`detect_build_layout_source_paths`] already
/// uses — so a module directory that plain doesn't have a built `R.jar` yet
/// (never built, or a pure-Kotlin module with no `res/`) is silently
/// skipped, never an error.
pub(crate) fn detect_android_r_class_jars(workspace_root: &Path) -> Vec<PathBuf> {
    let subprojects = settings_subprojects(workspace_root);
    let module_dirs: Vec<PathBuf> = if subprojects.is_empty() {
        vec![workspace_root.to_owned()]
    } else {
        subprojects.iter().map(|s| workspace_root.join(s)).collect()
    };

    let jars: Vec<PathBuf> = module_dirs
        .iter()
        .filter_map(|module_dir| find_r_class_jar_for_module(module_dir))
        .collect();

    if !jars.is_empty() {
        log::info!(
            "android-r-class: auto-detected {} module R.jar(s)",
            jars.len()
        );
    }
    jars
}

/// One module's own `R.jar`, if its project has been built — see
/// [`detect_android_r_class_jars`]'s doc for the selection rule (prefer a
/// "debug"-named variant, else the first one found).
fn find_r_class_jar_for_module(module_dir: &Path) -> Option<PathBuf> {
    let mut fallback: Option<PathBuf> = None;
    for task_dir_name in R_CLASS_JAR_TASK_DIRS {
        let task_dir = module_dir.join("build/intermediates").join(task_dir_name);
        let Ok(variant_entries) = std::fs::read_dir(&task_dir) else {
            continue;
        };
        for variant_entry in variant_entries.filter_map(|e| e.ok()) {
            if !variant_entry
                .file_type()
                .map(|t| t.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let Ok(task_output_entries) = std::fs::read_dir(variant_entry.path()) else {
                continue;
            };
            for task_output_entry in task_output_entries.filter_map(|e| e.ok()) {
                let jar = task_output_entry.path().join("R.jar");
                if !jar.is_file() {
                    continue;
                }
                let is_debug_shaped = variant_entry
                    .file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .contains("debug");
                if is_debug_shaped {
                    return Some(jar);
                }
                fallback.get_or_insert(jar);
            }
        }
    }
    fallback
}

/// Resolve the local Android SDK root directory, checking (in order)
/// `local.properties`' `sdk.dir`, then `$ANDROID_HOME`, then
/// `$ANDROID_SDK_ROOT`. Shared by `detect_android_sdk_source_paths` and
/// `detect_android_sdk_jar_path` so the lookup logic exists in one place.
fn resolve_android_sdk_root(workspace_root: &Path) -> Option<PathBuf> {
    sdk_dir_from_local_properties(workspace_root)
        .or_else(|| std::env::var("ANDROID_HOME").ok().map(PathBuf::from))
        .or_else(|| std::env::var("ANDROID_SDK_ROOT").ok().map(PathBuf::from))
        .filter(|p| p.is_dir())
}

/// Read `sdk.dir` from `<workspace_root>/local.properties`.
fn sdk_dir_from_local_properties(workspace_root: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(workspace_root.join("local.properties")).ok()?;
    content.lines().find_map(|line| {
        let (key, val) = line.split_once('=')?;
        if key.trim() == "sdk.dir" {
            Some(PathBuf::from(val.trim()))
        } else {
            None
        }
    })
}

/// Returns the subdirectory of `root` named `android-XX` (or `android-XX.Y`)
/// with the highest API level, per `parse_android_api_level`.
fn highest_api_level_dir(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().ok().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let api = parse_android_api_level(&e.file_name().to_string_lossy())?;
            Some((api, e.path()))
        })
        .max_by_key(|(api, _)| *api)
        .map(|(_, path)| path)
}

/// Parses an `android-XX`, `android-XX.Y`, or `android-XX-extZZ` SDK
/// directory name into a comparable `(major, minor, ext)` API level. Modern
/// Android SDK installs use a decimal extension-level suffix for some
/// platforms (e.g. `android-36.1`, `android-37.0`, confirmed present on a
/// real developer machine alongside plain `android-36`) — this must sort
/// higher than a same-major plain directory, and plain directories are
/// treated as minor level `0`. SDK Extension APIs (a separate, additive
/// versioning axis — see Android's own SDK Extensions documentation) install
/// under a `-extZZ`-suffixed directory name instead (e.g. `android-36-ext14`
/// alongside plain `android-36`); an extension level must outrank the plain
/// base of the same major version, since its `android.jar` is additive to
/// the base platform, not a replacement for a different major/minor axis —
/// plain directories are treated as extension level `0`.
fn parse_android_api_level(dir_name: &str) -> Option<(u32, u32, u32)> {
    let level = dir_name.strip_prefix("android-")?;
    let (level, ext) = match level.split_once("-ext") {
        Some((base, ext_level)) => (base, ext_level.parse().ok()?),
        None => (level, 0),
    };
    match level.split_once('.') {
        Some((major, minor)) => Some((major.parse().ok()?, minor.parse().ok()?, ext)),
        None => Some((level.parse().ok()?, 0, ext)),
    }
}

#[cfg(test)]
#[path = "workspace_json_tests.rs"]
mod tests;
