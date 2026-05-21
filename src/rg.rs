//! ripgrep / glob helpers — workspace-wide symbol search.
//!
//! This module owns every item that shells out to `rg`:
//! - [`IgnoreMatcher`]   — compile and apply workspace ignore patterns
//! - [`SOURCE_EXTENSIONS`] — file extensions searched by `rg`/`fd`
//! - [`build_rg_pattern`] — build the regex passed to `rg -e`
//! - [`effective_rg_root`] — pick the best search root for a given open file
//! - [`rg_find_definition`] — locate declaration sites
//! - [`rg_find_references`] — locate all usages
//! - [`rg_find_implementors`] — heuristic implementor finder
//! - [`parse_rg_line`]   — parse one `rg --with-filename` output line

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::StrExt;

// ─── Ignore pattern matcher ───────────────────────────────────────────────────

/// Compiled workspace-level ignore patterns from `initializationOptions`.
///
/// Patterns follow gitignore glob semantics:
/// - A bare pattern with no `/` (e.g. `bazel-*`) matches at any depth.
/// - A pattern containing `/` (e.g. `build/**`) matches relative to the workspace root.
/// - Absolute paths under the workspace root are normalized to relative before matching.
pub(crate) struct IgnoreMatcher {
    /// Original patterns as provided by the client (passed to `fd --exclude` as-is).
    pub patterns: Vec<String>,
    /// Arc-wrapped so the compiled set can be shared into `filter_entry` closures.
    glob_set: Arc<globset::GlobSet>,
    /// Workspace root this matcher was built for — used to relativize absolute paths.
    root: std::path::PathBuf,
}

impl IgnoreMatcher {
    /// Build an `IgnoreMatcher` from raw client patterns against `root`.
    pub(crate) fn new(patterns: Vec<String>, root: &Path) -> Self {
        let mut builder = globset::GlobSetBuilder::new();
        for pat in &patterns {
            // Normalize absolute paths that fall under the workspace root.
            let normalized = if Path::new(pat.as_str()).is_absolute() {
                match Path::new(pat.as_str()).strip_prefix(root) {
                    Ok(rel) => rel.to_string_lossy().into_owned(),
                    Err(_) => {
                        log::warn!("ignorePatterns: absolute path {:?} is not under workspace root, skipping", pat);
                        continue;
                    }
                }
            } else {
                pat.clone()
            };

            // Bare patterns (no `/`) match at any depth.
            // Compile two variants:
            //   `**/pattern`    — matches the directory entry itself (used in walkdir filter_entry)
            //   `**/pattern/**` — matches all files inside a matching directory (used in filter_locs)
            let glob_pats: Vec<String> = if !normalized.contains('/') {
                vec![
                    format!("**/{}", normalized),
                    format!("**/{}/", normalized), // trailing / for dir match
                    format!("**/{normalized}/**"),
                ]
            } else {
                vec![normalized]
            };

            for glob_pat in glob_pats {
                match globset::Glob::new(&glob_pat) {
                    Ok(g) => {
                        builder.add(g);
                    }
                    Err(e) => {
                        log::warn!("ignorePatterns: invalid pattern {:?}: {}", pat, e);
                    }
                }
            }
        }
        let glob_set = builder.build().unwrap_or_else(|e| {
            log::warn!("ignorePatterns: failed to build glob set: {}", e);
            globset::GlobSetBuilder::new().build().unwrap()
        });
        Self {
            patterns,
            glob_set: Arc::new(glob_set),
            root: root.to_path_buf(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Returns `true` if `rel_path` (relative to workspace root) should be excluded.
    pub(crate) fn matches(&self, rel_path: &Path) -> bool {
        self.glob_set.is_match(rel_path)
    }

    /// Clone the Arc-wrapped glob set for use in `filter_entry` closures.
    pub(crate) fn glob_set(&self) -> Arc<globset::GlobSet> {
        Arc::clone(&self.glob_set)
    }

    /// Remove locations whose file path is inside an ignored directory.
    /// Paths are relativized against the workspace root this matcher was built for.
    pub(crate) fn filter_locs(&self, locs: Vec<Location>) -> Vec<Location> {
        locs.into_iter()
            .filter(|loc| {
                if let Ok(path) = loc.uri.to_file_path() {
                    let rel = path.strip_prefix(&self.root).unwrap_or(&path);
                    !self.matches(rel)
                } else {
                    true
                }
            })
            .collect()
    }

    /// Remove file paths (absolute strings) that fall inside an ignored directory.
    pub(crate) fn filter_file_strings(&self, files: Vec<String>) -> Vec<String> {
        files
            .into_iter()
            .filter(|f| {
                let path = Path::new(f);
                let rel = path.strip_prefix(&self.root).unwrap_or(path);
                !self.matches(rel)
            })
            .collect()
    }

    /// Remove `PathBuf` entries that fall inside an ignored directory.
    pub(crate) fn filter_paths(&self, paths: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
        paths
            .into_iter()
            .filter(|p| {
                let rel = p.strip_prefix(&self.root).unwrap_or(p);
                !self.matches(rel)
            })
            .collect()
    }
}

// ─── Constants ────────────────────────────────────────────────────────────────

/// Supported file extensions for indexing and rg/fd searches.
pub(crate) const SOURCE_EXTENSIONS: &[&str] = &["kt", "java", "swift"];

// ─── Pattern builder ─────────────────────────────────────────────────────────

/// Build the regex pattern used by `rg` for declaration sites.
///
/// Matches both Kotlin and Java declaration keywords followed by `NAME`.
///
/// Kotlin: `fun`, `class`, `object`, `val`, `var`, `typealias`, `enum class`,
///         extension functions `fun ReceiverType.name`
/// Java:   `class`, `interface`, `enum` (standalone, no `class` suffix),
///         with any leading access/modifier keywords ignored
pub(crate) fn build_rg_pattern(name: &str) -> String {
    let safe: String = name
        .chars()
        .flat_map(|c| {
            if c.is_alphanumeric() || c == '_' {
                vec![c]
            } else {
                vec!['\\', c]
            }
        })
        .collect();
    // Kotlin: standard keywords + `enum class` + extension function receiver
    // Java:   `enum NAME` (Java enums have no `class` after `enum`)
    // Swift:  struct, protocol, extension, let (in addition to shared keywords)
    format!(
        r"(?:(?:class|struct|interface|object|protocol|fun|func|val|var|let|typealias|enum\s+class)\s+|fun\s+\w[\w.]*\.|(?:public|private|protected|fileprivate|open|internal|static|abstract|final|\s)+(?:enum|class|struct|protocol)\s+|extension\s+){safe}\b"
    )
}

// ─── Root helpers ─────────────────────────────────────────────────────────────

/// Walk up from `file` until a `.git` directory is found, returning that
/// ancestor as the project root.  Returns `None` if no `.git` is found.
fn walk_to_git_root(file: &Path) -> Option<PathBuf> {
    let mut cur = file.parent()?;
    loop {
        if cur.join(".git").exists() {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
}

/// Return the best search root for rg/fd fallbacks given:
/// - `workspace_root` — the globally configured root (may point to a different project)
/// - `open_file`      — the file the user has open right now
///
/// If `open_file` lives inside `workspace_root`, use `workspace_root`.
/// Otherwise walk up from `open_file` to find a `.git` root and use that,
/// so rg searches the *actual* project even when the workspace config is stale.
pub(crate) fn effective_rg_root(
    workspace_root: Option<&Path>,
    open_file: Option<&Path>,
) -> Option<PathBuf> {
    match (workspace_root, open_file) {
        (Some(root), Some(fp)) if fp.starts_with(root) => Some(root.to_path_buf()),
        (_, Some(fp)) => walk_to_git_root(fp)
            .or_else(|| fp.parent().map(|p| p.to_path_buf()))
            .or_else(|| std::env::current_dir().ok()),
        (Some(root), None) => Some(root.to_path_buf()),
        (None, None) => std::env::current_dir().ok(),
    }
}

// ─── Public rg search functions ──────────────────────────────────────────────

/// Run `rg` to find definition sites for `name`, scoped to `root`.
///
/// When `root` is an absolute path, rg outputs absolute paths in results.
/// Passing workspace root here is essential; without it rg would search
/// from CWD which may not be the project when spawned by the editor.
///
/// When `source_paths` is non-empty, rg searches only those directories instead
/// of `root`. `root` is still used as the base for resolving relative entries in
/// `source_paths` and as a fallback if every configured path is missing on disk.
///
/// Results in directories matched by `matcher` are filtered out.
pub(crate) fn rg_find_definition(
    name: &str,
    root: Option<&Path>,
    source_paths: &[String],
    matcher: Option<&IgnoreMatcher>,
) -> Vec<Location> {
    let pattern = build_rg_pattern(name);

    // Use the provided root, or fall back to CWD (which editors like Helix
    // set to the workspace root when spawning the LSP server).
    let fallback_root: std::borrow::Cow<Path> = match root {
        Some(r) => std::borrow::Cow::Borrowed(r),
        None => std::borrow::Cow::Owned(std::env::current_dir().unwrap_or_default()),
    };

    let locs = RgSearch::scoped(source_paths, fallback_root.as_ref())
        .with_pattern(pattern)
        .locations();

    if let Some(m) = matcher {
        m.filter_locs(locs)
    } else {
        locs
    }
}

/// Request parameters for a ripgrep reference search.
pub(crate) struct RgSearchRequest<'a> {
    name: &'a str,
    parent_class: Option<&'a str>,
    declared_pkg: Option<&'a str>,
    /// Outer-outer class for a lowercase method declared inside a doubly-nested
    /// class (e.g. `create` inside `Factory` inside `RegularReducer`).
    ///
    /// When set, file discovery searches for files that mention this class (via
    /// `\bOwnerClass\b`) rather than using import or package patterns.  This
    /// ensures callers that reference the outer class via a variable name
    /// (`factory.create()`) are found, while sibling factories in the same
    /// package are excluded because they do not reference the outer class.
    owner_class: Option<&'a str>,
    /// Declaring class for a class member (field, property, or method) reference
    /// (e.g. `"FamilyAccount"` for a `val value` or `fun load()` declared inside
    /// `FamilyAccount`).
    ///
    /// When set, file discovery finds files mentioning the declaring class, then
    /// searches for the member name within those files.  Unlike `owner_class`, the
    /// declaring file is NOT restricted to only the declaration — bare member access
    /// inside the class body is valid.  Declaration lines in other files are
    /// filtered out to avoid picking up same-named members in other classes.
    field_owner: Option<&'a str>,
    /// 0-based line of the actual field declaration in `from_uri`.
    ///
    /// When `field_owner` is set, declaration-pattern hits in the declaring file
    /// at lines OTHER than this one are filtered out (same-named fields in other
    /// classes inside the same multi-class file).
    field_decl_line: Option<u32>,
    search_root: std::borrow::Cow<'a, Path>,
    /// Source-root directories from workspace config; when non-empty, rg is
    /// scoped to these directories instead of the full workspace root.
    source_paths: &'a [String],
    include_decl: bool,
    from_uri: &'a Url,
    decl_files: &'a [String],
    /// Pre-computed candidate files from the in-memory import index.
    /// When non-empty, the rg import-pattern pass in `parent_scoped_reference_locations`
    /// is skipped and these files are used directly as bare-name scan candidates.
    /// Only contains files that explicitly import `Parent.Name` or `Parent.*`.
    pub(crate) index_candidate_files: Vec<String>,
    /// Pre-computed candidate files for the qualified pass (`\bParent\.\bName\b`).
    /// Superset of `index_candidate_files`: also includes files that import the
    /// parent class directly (e.g. `import com.a.ReducerA`) which can write
    /// `ReducerA.Factory` without a `ReducerA.Factory` import.
    /// When non-empty, the qualified rg pass is scoped to these files instead of
    /// searching the whole workspace, preventing cross-package same-short-name FPs.
    pub(crate) index_qualified_candidate_files: Vec<String>,
}

enum RgTarget<'a> {
    Root(&'a Path),
    /// Workspace source-root directories (paths under the workspace root).
    /// When set, rg searches only these directories instead of the full workspace root.
    /// Relative paths are resolved against `parse_root` at command-build time.
    SourcePaths(&'a [String]),
    Files(&'a [String]),
}

struct RgSearch<'a> {
    parse_root: &'a Path,
    target: RgTarget<'a>,
    patterns: Vec<String>,
    word_regexp: bool,
    list_files: bool,
}

impl<'a> RgSearch<'a> {
    fn rooted(root: &'a Path) -> Self {
        Self {
            parse_root: root,
            target: RgTarget::Root(root),
            patterns: Vec::new(),
            word_regexp: false,
            list_files: false,
        }
    }

    /// Search only within `source_paths` directories (when configured via `sourceRoots`).
    /// Falls back to `fallback_root` when `source_paths` is empty.
    fn scoped(source_paths: &'a [String], fallback_root: &'a Path) -> Self {
        if source_paths.is_empty() {
            return Self::rooted(fallback_root);
        }
        Self {
            parse_root: fallback_root,
            target: RgTarget::SourcePaths(source_paths),
            patterns: Vec::new(),
            word_regexp: false,
            list_files: false,
        }
    }

    fn files(files: &'a [String]) -> Self {
        Self {
            parse_root: Path::new("/"),
            target: RgTarget::Files(files),
            patterns: Vec::new(),
            word_regexp: false,
            list_files: false,
        }
    }

    fn with_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.patterns.push(pattern.into());
        self
    }

    fn with_patterns<I, S>(mut self, patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.patterns.extend(patterns.into_iter().map(Into::into));
        self
    }

    fn word_regexp(mut self) -> Self {
        self.word_regexp = true;
        self
    }

    fn list_files(mut self) -> Self {
        self.list_files = true;
        self
    }

    fn build_command(&self) -> Command {
        let mut command = Command::new("rg");
        if self.list_files {
            command.arg("-l");
        } else {
            command.args([
                "--no-heading",
                "--with-filename",
                "--line-number",
                "--column",
            ]);
        }
        if self.word_regexp {
            command.arg("--word-regexp");
        }
        for ext in SOURCE_EXTENSIONS {
            command.args(["--glob", &format!("*.{ext}")]);
        }
        for pattern in &self.patterns {
            command.args(["-e", pattern]);
        }
        match &self.target {
            RgTarget::Root(root) => {
                command.arg(root);
            }
            RgTarget::SourcePaths(paths) => {
                let mut any_added = false;
                for p in paths.iter() {
                    let path = Path::new(p);
                    let abs = if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        self.parse_root.join(path)
                    };
                    if abs.is_dir() {
                        command.arg(&abs);
                        any_added = true;
                    }
                }
                // If all configured source paths are missing, fall back to workspace root
                // so rg doesn't silently return zero results.
                if !any_added {
                    command.arg(self.parse_root);
                }
            }
            RgTarget::Files(files) => {
                command.arg("--");
                command.args(*files);
            }
        }
        command
    }

    fn output(&self) -> Option<std::process::Output> {
        let mut command = self.build_command();
        match command.output() {
            Ok(output) if output.status.success() && !output.stdout.is_empty() => Some(output),
            _ => None,
        }
    }

    fn locations_with_content(&self) -> Vec<(Location, String)> {
        let Some(output) = self.output() else {
            return vec![];
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| parse_rg_line_with_content_rooted(line, self.parse_root))
            .collect()
    }

    fn locations(&self) -> Vec<Location> {
        self.locations_with_content()
            .into_iter()
            .map(|(location, _)| location)
            .collect()
    }

    fn files_with_matches(&self) -> Vec<String> {
        let Some(output) = self.output() else {
            return vec![];
        };
        let root = match &self.target {
            RgTarget::Root(root) => *root,
            RgTarget::Files(_) => return vec![],
            RgTarget::SourcePaths(_) => {
                // Apply the same relative-path normalization as the Root branch so that
                // a source root passed as relative (or rg run from a different cwd) doesn't
                // produce relative filenames that later fail URI construction.
                let parse_root = self.parse_root;
                return String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(|line| {
                        let path = Path::new(line);
                        if path.is_absolute() {
                            line.to_owned()
                        } else {
                            parse_root.join(line).to_string_lossy().into_owned()
                        }
                    })
                    .collect();
            }
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| {
                let path = Path::new(line);
                if path.is_absolute() {
                    line.to_owned()
                } else {
                    root.join(line).to_string_lossy().into_owned()
                }
            })
            .collect()
    }
}

impl<'a> RgSearchRequest<'a> {
    pub(crate) fn new(
        name: &'a str,
        parent_class: Option<&'a str>,
        declared_pkg: Option<&'a str>,
        root: Option<&'a Path>,
        include_decl: bool,
        from_uri: &'a Url,
        decl_files: &'a [String],
    ) -> Self {
        let search_root = match root {
            Some(root) => std::borrow::Cow::Borrowed(root),
            None => std::borrow::Cow::Owned(std::env::current_dir().unwrap_or_default()),
        };
        Self {
            name,
            parent_class,
            declared_pkg,
            owner_class: None,
            field_owner: None,
            field_decl_line: None,
            search_root,
            source_paths: &[],
            include_decl,
            from_uri,
            decl_files,
            index_candidate_files: Vec::new(),
            index_qualified_candidate_files: Vec::new(),
        }
    }

    pub(crate) fn with_index_candidates(mut self, candidates: Vec<String>) -> Self {
        self.index_candidate_files = candidates;
        self
    }

    pub(crate) fn with_index_qualified_candidates(mut self, candidates: Vec<String>) -> Self {
        self.index_qualified_candidate_files = candidates;
        self
    }

    pub(crate) fn with_source_paths(mut self, source_paths: &'a [String]) -> Self {
        self.source_paths = source_paths;
        self
    }

    pub(crate) fn with_owner_class(mut self, owner_class: &'a str) -> Self {
        self.owner_class = Some(owner_class);
        self
    }

    pub(crate) fn with_field_owner(mut self, field_owner: &'a str) -> Self {
        self.field_owner = Some(field_owner);
        self
    }

    pub(crate) fn with_field_decl_line(mut self, line: u32) -> Self {
        self.field_decl_line = Some(line);
        self
    }
}

const REFERENCE_DECLARATION_KEYWORDS: &[&str] = &[
    "class ",
    "interface ",
    "object ",
    "fun ",
    "val ",
    "var ",
    "typealias ",
    "enum class ",
    "enum ",
    "struct ",
    "protocol ",
    "func ",
    "let ",
    "extension ",
];

fn build_rg_patterns(request: &RgSearchRequest<'_>) -> Vec<String> {
    let safe_name = regex_escape(request.name);
    if let Some(parent_class) = request.parent_class {
        let safe_parent = regex_escape(parent_class);
        let qualified_pattern = format!(r"\b{}\.\b{}\b", safe_parent, safe_name);
        // For uppercase nested types (e.g. `Event` inside `IntroContract`), only files
        // that explicitly import `ParentClass.Name` or `ParentClass.*` can use `Name` as
        // a bare identifier.  Using the broad `import.*ParentClass` pattern here causes
        // false positives: any file that imports `IntroContract` for an unrelated member
        // (e.g. `IntroContract.State`) also contains bare occurrences of other `Event`
        // classes, ballooning results to hundreds of false hits.
        //
        // Qualified references (`IntroContract.Event`) are already captured by
        // `qualified_pattern` in the first rg pass, so they don't depend on this path.
        //
        // For lowercase method names, the caller uses `repo.getRate()` where `repo` is of
        // type `ParentClass` — no specific import of the method is needed, so any file
        // that imports `ParentClass` is a legitimate candidate for bare-name scanning.
        //
        // Note: `\b` does not anchor after `*`, so the star-import alternative is written
        // as a separate branch rather than `(?:Name|\*)`.
        //
        // The name branch uses `(?:\s|;|$)` instead of `\b` so that deeper imports like
        // `import ...Parent.Name.Companion` are NOT treated as candidate files — `\b` is
        // true before `.`, which would wrongly mark those files as able to use bare `Name`.
        let import_pattern = if request.name.starts_with_uppercase() {
            format!(
                r"import[^\n]*\b{safe_parent}\.{safe_name}(?:\s|;|$)|import[^\n]*\b{safe_parent}\.\*"
            )
        } else {
            format!(r"import[^\n]*\b{safe_parent}\b")
        };
        vec![qualified_pattern, import_pattern, safe_name]
    } else if let Some(declared_pkg) = request.declared_pkg {
        let safe_package = regex_escape(declared_pkg);
        let import_pattern = format!(
            r"import[^\n]*\b{safe_package}\b[^\n]*\b{safe_name}\b|import[^\n]*\b{safe_package}\b\.\*"
        );
        let package_pattern = format!(r"^\s*package\s+{safe_package}\s*;?\s*$");
        vec![import_pattern, package_pattern, safe_name]
    } else {
        vec![safe_name]
    }
}

fn should_skip_reference(loc: &Location, content: &str, request: &RgSearchRequest<'_>) -> bool {
    let trimmed = content.trim_start();
    if trimmed.starts_with("import ") || trimmed.starts_with("package ") {
        return true;
    }
    if request.include_decl {
        return false;
    }
    let is_decl = is_declaration_occurrence_at(content, loc.range.start.character);
    if !is_decl {
        return false;
    }
    // Always drop the original declaration in the cursor file.
    if loc.uri == *request.from_uri {
        return true;
    }
    // For uppercase names (class/interface/type references), drop declarations in
    // other files too.  Same-name classes in other containers (e.g. Other.Inner.Factory
    // vs Outer.Inner.Factory) would otherwise appear as false positives — there is no
    // dot-qualifier to disambiguate a bare `interface Factory` declaration.
    //
    // For lowercase method names (fun/val/var), declarations in other files are
    // *override* implementations and are valid references that should be included.
    request
        .name
        .chars()
        .next()
        .is_some_and(|c| c.is_uppercase())
}

/// Returns `true` if the match at byte offset `col` in `content` is the name
/// token of a declaration (immediately preceded by a declaration keyword such
/// as `fun `, `val `, `class `, etc., after stripping trailing spaces).
///
/// `col` is the 0-based byte offset returned by `rg --column`, stored directly
/// in `Location::range::start::character` for rg-produced locations.
pub(crate) fn is_declaration_occurrence_at(content: &str, col: u32) -> bool {
    let col = col as usize;
    if col == 0 {
        return false;
    }
    let prefix = &content[..col.min(content.len())];
    // Trim trailing spaces so `fun  name` (extra space) also matches.
    let prefix_trimmed = prefix.trim_end_matches(' ');
    REFERENCE_DECLARATION_KEYWORDS.iter().any(|kw| {
        let kw_trimmed = kw.trim_end(); // "fun " → "fun", "val " → "val"
        if !prefix_trimmed.ends_with(kw_trimmed) {
            return false;
        }
        // Ensure the character before the keyword is not an identifier char
        // so `notafun` does not match keyword `fun`.
        let before = &prefix_trimmed[..prefix_trimmed.len() - kw_trimmed.len()];
        before.is_empty() || !before.ends_with(|c: char| c.is_alphanumeric() || c == '_')
    })
}

fn run_rg_search(request: &RgSearchRequest<'_>, patterns: &[String]) -> Vec<Location> {
    let mut search = RgSearch::scoped(request.source_paths, request.search_root.as_ref())
        .with_patterns(patterns.iter().cloned());
    if request.parent_class.is_none() && request.declared_pkg.is_none() {
        search = search.word_regexp();
    }
    search
        .locations_with_content()
        .into_iter()
        .filter_map(|(loc, content)| {
            (!should_skip_reference(&loc, &content, request)).then_some(loc)
        })
        .collect()
}

fn filter_candidate_files(
    candidate_files: Vec<String>,
    matcher: Option<&IgnoreMatcher>,
) -> Vec<String> {
    match matcher {
        Some(matcher) => matcher.filter_file_strings(candidate_files),
        None => candidate_files,
    }
}

fn extend_unique_files(files: &mut Vec<String>, new_files: Vec<String>) {
    for file in new_files {
        if !files.contains(&file) {
            files.push(file);
        }
    }
}

fn merge_decl_files(candidate_files: &mut Vec<String>, decl_files: &[String]) {
    let mut existing: std::collections::HashSet<String> = candidate_files.iter().cloned().collect();
    for decl_file in decl_files {
        if existing.insert(decl_file.clone()) {
            candidate_files.push(decl_file.clone());
        }
    }
}

/// When `source_paths` is non-empty, filter `decl_files` to only those within the
/// configured source roots so declaration files outside the scope don't bypass scoping.
fn scope_decl_files<'a>(
    decl_files: &'a [String],
    source_paths: &'a [String],
) -> std::borrow::Cow<'a, [String]> {
    if source_paths.is_empty() {
        return std::borrow::Cow::Borrowed(decl_files);
    }
    // Use Path::starts_with (component-based) rather than str::starts_with to avoid
    // sibling-path false positives: "/src/main/kotlin2" must not match "/src/main/kotlin".
    let source_paths_buf: Vec<&Path> = source_paths.iter().map(|s| Path::new(s.as_str())).collect();
    let filtered: Vec<String> = decl_files
        .iter()
        .filter(|f| {
            let fp = Path::new(f.as_str());
            source_paths_buf.iter().any(|sp| fp.starts_with(sp))
        })
        .cloned()
        .collect();
    std::borrow::Cow::Owned(filtered)
}

/// Returns `true` if `c` is a valid identifier or qualifier-chain character.
///
/// Used when walking backward over text to extract the dot-qualified chain
/// preceding a name (e.g. `"ReducerA"` in `ReducerA.Factory` or
/// `"Outer.Inner"` in `Outer.Inner.Factory`).
fn is_qualifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

/// Returns `true` if the `.<name>` occurrence whose name starts at **byte** offset
/// `name_byte_col` has a qualifier that doesn't match `expected_parent`.
///
/// This inspects only the single occurrence at `name_byte_col`, preventing false
/// positives on lines that contain multiple qualified names
/// (e.g. `ReducerA.Factory, ReducerC.Factory` — the hit for `ReducerA.Factory`
/// at its specific column should not be dropped because `ReducerC.Factory` appears
/// later on the same line).
///
/// `name_byte_col` is the 0-based byte offset of the start of `name` within
/// `content` (matching the `character` field in [`Location`] as returned by rg).
pub(crate) fn has_wrong_qualifier_at_col(
    content: &str,
    name: &str,
    expected_parent: &str,
    name_byte_col: u32,
) -> bool {
    let col = name_byte_col as usize;
    // Verify the occurrence is actually `name` at this position (guards against
    // byte-offset mismatches with multi-byte content).
    if content.get(col..col + name.len()).is_none_or(|s| s != name) {
        return false;
    }
    // A dot immediately before the name signals a qualified reference.
    if col > 0 && content.as_bytes().get(col - 1) == Some(&b'.') {
        let qualifier: String = content[..col - 1]
            .chars()
            .rev()
            .take_while(|&c| is_qualifier_char(c))
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let qualifier = qualifier.trim_start_matches('.');
        return !qualifier.is_empty() && qualifier != expected_parent;
    }
    // No dot immediately before: bare name usage at this position — always allowed.
    false
}

fn append_unique_reference_hits(
    locations: &mut Vec<Location>,
    hits: Vec<(Location, String)>,
    request: &RgSearchRequest<'_>,
) {
    let mut seen: std::collections::HashSet<(String, u32, u32)> = locations
        .iter()
        .map(|location| {
            (
                location.uri.to_string(),
                location.range.start.line,
                location.range.start.character,
            )
        })
        .collect();

    for (location, content) in hits {
        if should_skip_reference(&location, &content, request) {
            continue;
        }
        // Java-specific: filter bare-name hits that are method declarations in
        // unrelated classes.  The `should_skip_reference` path above handles
        // Kotlin/Swift via keyword detection; Java method declarations lack fixed
        // keywords before the name and need structural detection instead.
        // Only applied to other files (from_uri's own declaration is kept).
        if location.uri.as_str().ends_with(".java")
            && location.uri.as_str() != request.from_uri.as_str()
            && is_java_method_declaration_at(&content, request.name, location.range.start.character)
        {
            continue;
        }
        if let Some(parent) = request.parent_class {
            // Qualifier filtering only applies to uppercase names (class/type references,
            // e.g. `ReducerA.Factory`).  For lowercase names (methods, properties), the
            // qualifier before the call is a runtime variable (`repo.getRate()`,
            // `this.load()`) — without type info we cannot verify it matches the declaring
            // class, so all variable-qualified calls are valid hits.
            let is_type_ref = request
                .name
                .chars()
                .next()
                .is_some_and(|c| c.is_uppercase());
            if is_type_ref
                && has_wrong_qualifier_at_col(
                    &content,
                    request.name,
                    parent,
                    location.range.start.character,
                )
            {
                continue;
            }
        }

        let key = (
            location.uri.to_string(),
            location.range.start.line,
            location.range.start.character,
        );
        if seen.insert(key) {
            locations.push(location);
        }
    }
}

fn parent_scoped_reference_locations(
    request: &RgSearchRequest<'_>,
    patterns: &[String],
    matcher: Option<&IgnoreMatcher>,
) -> Vec<Location> {
    // Step 1: discover bare-name candidates — files that explicitly import
    // `Parent.Name` or `Parent.*`.  Falls back to rg import-pattern scan when
    // the index is not yet populated (cold start).
    let mut candidate_files = if !request.index_candidate_files.is_empty() {
        filter_candidate_files(request.index_candidate_files.clone(), matcher)
    } else {
        filter_candidate_files(
            rg_files_with_matches_scoped(
                &patterns[1],
                request.source_paths,
                request.search_root.as_ref(),
            ),
            matcher,
        )
    };

    // Step 2: for uppercase nested types, same-package files can reference
    // `ParentClass.Name` without an import (Kotlin in-package visibility), so
    // they are legitimate qualified-pass candidates.  However, they cannot use
    // `Name` as a bare identifier without an explicit import — so same-package
    // files are NOT added to the bare-name candidate set for uppercase types.
    // For lowercase method names both passes use the same expanded candidate set.
    let same_pkg_files: Vec<String> = if let Some(pkg) = request.declared_pkg {
        let safe_pkg = regex_escape(pkg);
        let pkg_pattern = format!(r"^\s*package\s+{safe_pkg}\s*;?\s*$");
        filter_candidate_files(
            rg_files_with_matches_scoped(
                &pkg_pattern,
                request.source_paths,
                request.search_root.as_ref(),
            ),
            matcher,
        )
    } else {
        vec![]
    };

    // Step 3: merge decl files into bare-name candidates only.
    merge_decl_files(
        &mut candidate_files,
        &scope_decl_files(request.decl_files, request.source_paths),
    );

    // Step 4: qualified pass.
    // When index-derived qualified candidates are available, scope the qualified
    // rg search to those files (+ same-package files) to prevent cross-package
    // same-short-name false positives (e.g. `com.b.IntroContract.Event` matching
    // when searching for `com.a.IntroContract.Event`).
    // For lowercase methods or cold-start (no index candidates), keep the
    // workspace-wide qualified pass that `run_rg_search` provides.
    let is_uppercase_nested = request.name.starts_with_uppercase();
    let qualified_hits =
        if is_uppercase_nested && !request.index_qualified_candidate_files.is_empty() {
            let mut qfiles =
                filter_candidate_files(request.index_qualified_candidate_files.clone(), matcher);
            extend_unique_files(&mut qfiles, same_pkg_files.clone());
            // Also include bare-name candidates (they already have the right import)
            extend_unique_files(&mut qfiles, candidate_files.clone());
            rg_pattern_in_files(&patterns[0], &qfiles)
        } else {
            // Cold start or lowercase: search workspace-wide as before.
            if !is_uppercase_nested {
                extend_unique_files(&mut candidate_files, same_pkg_files);
            }
            run_rg_search(request, &patterns[..1])
                .into_iter()
                .map(|loc| (loc, String::new()))
                .collect()
        };

    let mut locations: Vec<Location> = qualified_hits
        .into_iter()
        .filter_map(|(loc, content)| {
            if content.is_empty() {
                // Came from run_rg_search which already applied should_skip_reference
                Some(loc)
            } else {
                (!should_skip_reference(&loc, &content, request)).then_some(loc)
            }
        })
        .collect();

    // Step 5: bare-name pass in import-based candidates only.
    if !candidate_files.is_empty() {
        let bare_hits = rg_word_in_files(&patterns[2], &candidate_files);
        append_unique_reference_hits(&mut locations, bare_hits, request);
    }
    locations
}

fn package_scoped_reference_locations(
    request: &RgSearchRequest<'_>,
    patterns: &[String],
    matcher: Option<&IgnoreMatcher>,
) -> Vec<Location> {
    let mut candidate_files = rg_files_with_matches_scoped(
        &patterns[0],
        request.source_paths,
        request.search_root.as_ref(),
    );
    extend_unique_files(
        &mut candidate_files,
        rg_files_with_matches_scoped(
            &patterns[1],
            request.source_paths,
            request.search_root.as_ref(),
        ),
    );
    let candidate_files = filter_candidate_files(candidate_files, matcher);
    if candidate_files.is_empty() {
        return vec![];
    }
    rg_word_in_files(&patterns[2], &candidate_files)
        .into_iter()
        .filter_map(|(location, content)| {
            if should_skip_reference(&location, &content, request) {
                return None;
            }
            // For Java files that are not the declaring file, filter out hits
            // that look like another class's method or field declaration.
            // `package_scoped_reference_locations` finds same-package files which
            // may have identically-named members; those are FPs, not call sites.
            let is_from_uri = location.uri.as_str() == request.from_uri.as_str();
            if !is_from_uri {
                let col = location.range.start.character;
                if std::path::Path::new(location.uri.path())
                    .extension()
                    .and_then(|e| e.to_str())
                    == Some("java")
                {
                    if is_java_method_declaration_at(&content, request.name, col) {
                        return None;
                    }
                    if is_java_field_declaration_at(&content, request.name, col) {
                        return None;
                    }
                }
            }
            Some(location)
        })
        .collect()
}

/// Find references to a lowercase method declared inside a doubly-nested class
/// (e.g. `create` inside `Factory` inside `RegularReducer`).
///
/// Callers use variable-name syntax (`factory.create()`) rather than qualified
/// syntax (`RegularReducer.create()`), so standard parent-class scoping misses
/// them.  Instead we find candidate files by searching for any mention of the
/// outer class, then do a bare-word search for the method name within those
/// files.  Sibling factories in the same package are naturally excluded because
/// they do not reference the outer class.
fn owner_scoped_reference_locations(
    request: &RgSearchRequest<'_>,
    matcher: Option<&IgnoreMatcher>,
) -> Vec<Location> {
    let owner_class = request.owner_class.expect("owner_class must be set");
    let safe_owner = regex_escape(owner_class);
    let safe_name = regex_escape(request.name);

    // Find files that mention the outer class (as a type, import, or constructor param).
    let owner_pattern = format!(r"\b{safe_owner}\b");
    let candidate_files = filter_candidate_files(
        rg_files_with_matches_scoped(
            &owner_pattern,
            request.source_paths,
            request.search_root.as_ref(),
        ),
        matcher,
    );

    if candidate_files.is_empty() {
        return vec![];
    }

    // Bare search for the method name in candidate files; qualifier filter is
    // intentionally skipped since callers use variable names, not class names.
    //
    // Filtering rules:
    // - `from_uri`: the declaring file. Only the declaration line is relevant;
    //   all other `create()` calls in it are to OTHER injected factory instances
    //   (the declaring file doesn't call its own Factory.create()).
    // - Other files: skip declaration lines of the same method name (e.g. sibling
    //   Factory.create() in a file that also imports the outer class).
    //   Additionally apply a naming-convention heuristic: if there is an explicit
    //   dot-qualifier before the name (e.g. `overviewMapperFactory.create`) and
    //   that qualifier does not contain the outer class name, the call is almost
    //   certainly to a different factory.
    rg_word_in_files(&safe_name, &candidate_files)
        .into_iter()
        .filter_map(|(loc, content)| {
            if should_skip_reference(&loc, &content, request) {
                return None;
            }
            let is_from_uri = loc.uri.as_str() == request.from_uri.as_str();
            if is_from_uri {
                // In the declaring file, only the declaration itself is relevant.
                return if request.include_decl && is_declaration_of(&content, request.name) {
                    Some(loc)
                } else {
                    None
                };
            }
            if is_declaration_of(&content, request.name) {
                return None;
            }
            if !qualifier_hints_owner(&content, loc.range.start.character as usize, owner_class) {
                return None;
            }
            Some(loc)
        })
        .collect()
}

/// Find references to a class member (field, property, or method) declared inside
/// a class or interface.
///
/// Scopes file discovery to files that mention the declaring class (by name),
/// then searches those files for the member name.
///
/// Differs from [`owner_scoped_reference_locations`] (for doubly-nested methods):
/// 1. The declaring file is **not** restricted to the declaration line — bare
///    member access inside the class body is valid.
/// 2. The `qualifier_hints_owner` heuristic is **not** applied — any occurrence
///    is kept, because instance variable names don't carry the declaring class name.
/// 3. Method implementations (`override fun someMethod()`) in other files are
///    **kept** — they are valid references, not false positives.
fn field_scoped_reference_locations(
    request: &RgSearchRequest<'_>,
    matcher: Option<&IgnoreMatcher>,
) -> Vec<Location> {
    let field_owner = request.field_owner.expect("field_owner must be set");
    let safe_owner = regex_escape(field_owner);
    let safe_name = regex_escape(request.name);

    // Candidate files: any file that mentions the declaring class/interface name.
    let owner_pattern = format!(r"\b{safe_owner}\b");
    let mut candidate_files = filter_candidate_files(
        rg_files_with_matches_scoped(
            &owner_pattern,
            request.source_paths,
            request.search_root.as_ref(),
        ),
        matcher,
    );
    // Always include the declaring file(s) — the class body can access the member
    // without the class name appearing elsewhere in the file.
    merge_decl_files(
        &mut candidate_files,
        &scope_decl_files(request.decl_files, request.source_paths),
    );

    if candidate_files.is_empty() {
        return vec![];
    }

    rg_word_in_files(&safe_name, &candidate_files)
        .into_iter()
        .filter_map(|(loc, content)| {
            if should_skip_reference(&loc, &content, request) {
                return None;
            }
            // In the declaring file: allow all hits except same-named declarations
            // in other classes (multi-class file false positives).
            let is_from_uri = loc.uri.as_str() == request.from_uri.as_str();
            if is_from_uri {
                if is_declaration_of(&content, request.name) {
                    // Keep the actual declaration; drop same-named decls in other classes.
                    let is_the_decl = request
                        .field_decl_line
                        .is_none_or(|dl| loc.range.start.line == dl);
                    return is_the_decl.then_some(loc);
                }
                return Some(loc);
            }
            // In other files: skip declaration-like lines for the same name to
            // avoid picking up `val value: String` in some unrelated class.
            if is_declaration_of(&content, request.name) {
                return None;
            }
            // Java-specific: filter out method/field declarations in unrelated
            // classes.  Kotlin/Swift declarations are caught above by keyword
            // detection; Java lacks fixed keywords before the member name, so we
            // use structural heuristics instead.  Only applied to `.java` files;
            // Kotlin files are deliberately unaffected.
            if loc.uri.as_str().ends_with(".java") {
                let col = loc.range.start.character;
                if is_java_method_declaration_at(&content, request.name, col)
                    || is_java_field_declaration_at(&content, request.name, col)
                {
                    return None;
                }
            }
            Some(loc)
        })
        .collect()
}

/// Naming-convention heuristic: returns `false` when the dot-qualifier before
/// `name_byte_col` in `content` is a non-empty identifier that does NOT contain
/// `owner_class` as a substring (case-insensitive).
///
/// Example: for `overviewMapperFactory.create(...)` with owner `DashboardProductsReducer`,
/// the qualifier is `overviewMapperFactory` which does NOT contain `dashboardproductsreducer`
/// → returns `false` (skip — different factory).
///
/// Returns `true` (keep) for: bare names (no qualifier), or when the qualifier
/// contains the owner name (e.g. `dashboardProductsReducerFactory`).
fn qualifier_hints_owner(content: &str, name_byte_col: usize, owner_class: &str) -> bool {
    let col = name_byte_col;
    if col == 0 || content.as_bytes().get(col - 1) != Some(&b'.') {
        return true; // bare name — no qualifier to check
    }
    // Walk back over alphanumeric/underscore to extract the immediate qualifier token.
    let qualifier: String = content[..col - 1]
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if qualifier.is_empty() {
        return true; // can't determine — allow
    }
    // Only apply the heuristic for long qualifiers (≥10 chars) that look like they
    // are derived from a class name.  Short names (e.g. `f`, `it`, `factory`) could
    // be generic variables for ANY factory — keep them to avoid false negatives.
    if qualifier.len() < 10 {
        return true;
    }
    qualifier
        .to_lowercase()
        .contains(&owner_class.to_lowercase())
}

/// Returns `true` if the match at byte-column `col` in `content` looks like a
/// Java **method** declaration: `name(…) {` or `name(…) throws` or `name(…) default`.
///
/// The check relies on two observable facts that distinguish a declaration from
/// a call in Java:
/// - The name is **not** immediately preceded by `.` (which would make it a
///   receiver-qualified call such as `payment.getAmount()`).
/// - The closing `)` of the parameter list is followed by `{`, `throws`, or
///   `default` on the same line (method body / checked-exception / interface
///   default marker).  Calls always end with `);` or `)` followed by an
///   operator — never `{`.
///
/// Scoped to `.java` files by callers; **not** applied to Kotlin/Swift.
pub(crate) fn is_java_method_declaration_at(content: &str, name: &str, col: u32) -> bool {
    let col = col as usize;
    let name_end = col + name.len();
    // Must be followed by `(`.
    if content.as_bytes().get(name_end) != Some(&b'(') {
        return false;
    }
    // Must NOT be preceded by `.` (qualified call → not a declaration).
    if col > 0 && content.as_bytes().get(col - 1) == Some(&b'.') {
        return false;
    }
    // Word boundary at start of name.
    if col > 0 {
        let prev = content.as_bytes()[col - 1];
        if prev.is_ascii_alphanumeric() || prev == b'_' {
            return false;
        }
    }
    // After the closing `)` there must be `{`, `throws`, or `default` to
    // distinguish a declaration from a call that ends with `);`.
    // Use balanced-paren matching (not rfind) so that parens inside parameter
    // types or comments after the closing `)` don't confuse us.
    let rest = &content[name_end + 1..]; // content right after the `(`
    if let Some(close) = balanced_paren_close(rest) {
        let after = rest[close + 1..].trim_start();
        after.starts_with('{') || after.starts_with("throws") || after.starts_with("default")
    } else {
        false
    }
}

/// Find the index of the `)` that closes the opening `(` already consumed.
///
/// `s` is the text *after* the opening `(`.  Returns the byte offset of the
/// matching `)` in `s`, or `None` if the parentheses are unbalanced (e.g. the
/// line was truncated).
fn balanced_paren_close(s: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Returns `true` if the match at byte-column `col` in `content` looks like a
/// Java **field or local-variable declaration**: the name is followed (after
/// optional whitespace) by `;`, `=`, or `,` and is **not** immediately preceded
/// by `.`.
///
/// This catches:
/// - `private BigDecimal mAmount;`
/// - `private BigDecimal mAmount = BigDecimal.ZERO;`
/// - `BigDecimal mAmount = payment.getAmount();`   ← local-var decl in another class
///
/// Not applied to the declaring file itself; scoped to `.java` files by callers.
pub(crate) fn is_java_field_declaration_at(content: &str, name: &str, col: u32) -> bool {
    let col = col as usize;
    let name_end = col + name.len();
    // Must NOT be preceded by `.`.
    if col > 0 && content.as_bytes().get(col - 1) == Some(&b'.') {
        return false;
    }
    // Word boundary at start of name.
    if col > 0 {
        let prev = content.as_bytes()[col - 1];
        if prev.is_ascii_alphanumeric() || prev == b'_' {
            return false;
        }
    }
    let after = content[name_end..].trim_start();
    matches!(
        after.as_bytes().first(),
        Some(b';') | Some(b'=') | Some(b',')
    )
}

/// Returns `true` if `content` declares `name` specifically (e.g. `fun create()`),
/// as opposed to a line that merely calls `name` inside a different declaration
/// (e.g. `fun build() = factory.create()` or `fun createWidget() = factory.create()`).
///
/// Requires a word boundary *after* `name` to avoid matching declarations of
/// longer identifiers that share a prefix — e.g. `fun createWidget` must not be
/// treated as a declaration of `create`.
///
/// Used by [`owner_scoped_reference_locations`] to filter out sibling
/// declarations of the same method name that appear in files which also reference
/// the outer class.
pub(crate) fn is_declaration_of(content: &str, name: &str) -> bool {
    REFERENCE_DECLARATION_KEYWORDS.iter().any(|kw| {
        let prefix = format!("{kw}{name}");
        if let Some(idx) = content.find(&prefix) {
            let end = idx + prefix.len();
            // Word-boundary check: name must not be followed by more identifier chars.
            content
                .as_bytes()
                .get(end)
                .is_none_or(|&b| !b.is_ascii_alphanumeric() && b != b'_')
        } else {
            false
        }
    })
}

/// Run `rg` to find all *usages* of `name` in the project.
///
/// Uses `--word-regexp` so only whole-word matches are returned.
/// If `include_decl` is false, lines in `from_uri` that contain declaration
/// keywords (e.g. `fun`, `val`, `class`) alongside `name` are filtered out.
/// Other lines from `from_uri` are still included (e.g. call sites in the
/// same file).
///
/// Results in directories matched by `matcher` are filtered out.
pub(crate) fn rg_find_references(
    request: &RgSearchRequest<'_>,
    matcher: Option<&IgnoreMatcher>,
) -> Vec<Location> {
    let result = if request.field_owner.is_some() {
        field_scoped_reference_locations(request, matcher)
    } else if request.owner_class.is_some() {
        owner_scoped_reference_locations(request, matcher)
    } else {
        let patterns = build_rg_patterns(request);
        if request.parent_class.is_some() {
            parent_scoped_reference_locations(request, &patterns, matcher)
        } else if request.declared_pkg.is_some() {
            package_scoped_reference_locations(request, &patterns, matcher)
        } else {
            run_rg_search(request, &patterns)
        }
    };

    if let Some(matcher) = matcher {
        matcher.filter_locs(result)
    } else {
        result
    }
}

/// Quick heuristic rg-based implementor finder. Scans files that mention `name`
/// and returns locations where the line looks like a declaration/implementation
/// of that type (Kotlin/Java `class Foo : Interface`, `implements`, Swift
/// `class Foo: Protocol`, `struct Foo: Protocol`). This is a fallback when the
/// subtype index is empty during cold indexing.
///
/// Results in directories matched by `matcher` are filtered out.
pub(crate) fn rg_find_implementors(
    name: &str,
    root: Option<&Path>,
    source_paths: &[String],
    matcher: Option<&IgnoreMatcher>,
) -> Vec<Location> {
    let safe = name.to_string();
    let root = match root {
        Some(r) => r,
        None => return vec![],
    };
    // Search for the name in source files.
    let locs: Vec<Location> = RgSearch::scoped(source_paths, root)
        .with_pattern(safe)
        .locations_with_content()
        .into_iter()
        .filter_map(|(loc, content)| {
            let line = content.trim();
            // Heuristics: declaration-like lines
            // Kotlin/Java: class Foo, interface Foo, enum class Foo, class Foo : Interface
            // Java implements: class Foo implements Interface
            // Swift: class Foo: Protocol, struct Foo: Protocol, extension Foo: Protocol
            let lower = line.to_lowercase();
            if lower.contains("class ")
                || lower.contains("struct ")
                || lower.contains("interface")
                || lower.contains("enum")
                || lower.contains("extension ")
            {
                // Check that the name appears as a word and near a declaration keyword
                if line.contains(name) {
                    return Some(loc);
                }
            }
            None
        })
        .collect();
    match matcher {
        Some(m) => m.filter_locs(locs),
        None => locs,
    }
}

/// Find `override fun method_name` locations across files that mention `declaring_class`.
///
/// Cold-start fallback for [`find_method_implementations`] when implementors are
/// not yet indexed.  Scopes candidate files to those that reference the declaring
/// class by name, then keeps only lines that look like an override declaration.
pub(crate) fn rg_find_method_overrides(
    method_name: &str,
    declaring_class: &str,
    root: Option<&Path>,
    source_paths: &[String],
    matcher: Option<&IgnoreMatcher>,
) -> Vec<Location> {
    let Some(root) = root else {
        return vec![];
    };

    // Candidate files: files that mention the declaring class.
    let safe_class = regex_escape(declaring_class);
    let candidate_files = filter_candidate_files(
        rg_files_with_matches_scoped(&format!(r"\b{safe_class}\b"), source_paths, root),
        matcher,
    );
    if candidate_files.is_empty() {
        return vec![];
    }

    // Keep only override declaration lines for the method name.
    let safe_method = regex_escape(method_name);
    rg_word_in_files(&safe_method, &candidate_files)
        .into_iter()
        .filter_map(|(loc, content)| {
            let lang = crate::Language::from_path(loc.uri.path());
            if lang.is_override_declaration(content.trim(), method_name) {
                Some(loc)
            } else {
                None
            }
        })
        .collect()
}

/// Parse one line of `rg --no-heading --with-filename --line-number --column`
/// output and return a [`Location`].
///
/// Expects the format `/abs/path/to/File.kt:line:col:content`.
/// Returns `None` if `file` is a relative path (rg sometimes emits relative
/// paths when invoked with a relative root; callers that need relative-path
/// support should use [`parse_rg_line_with_content_rooted`] instead).
pub(crate) fn parse_rg_line(line: &str) -> Option<Location> {
    // format: /abs/path/to/File.kt:line:col:content
    let mut parts = line.splitn(4, ':');
    let file = parts.next()?;
    let line_num: u32 = parts.next()?.trim().parse().ok()?;
    let col: u32 = parts.next()?.trim().parse().ok()?;

    let path = std::path::Path::new(file);
    // Silently skip if rg somehow gave us a relative path.
    if !path.is_absolute() {
        return None;
    }

    let uri = Url::from_file_path(path).ok()?;
    let pos = Position::new(line_num.saturating_sub(1), col.saturating_sub(1));
    Some(Location {
        uri,
        range: Range::new(pos, pos),
    })
}

// ─── Private helpers ─────────────────────────────────────────────────────────

/// Escape a string for use as a regex literal (non-alphanumeric chars → `\c`).
pub(crate) fn regex_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_alphanumeric() || c == '_' {
                vec![c]
            } else {
                vec!['\\', c]
            }
        })
        .collect()
}

fn rg_files_with_matches_scoped(
    pattern: &str,
    source_paths: &[String],
    root: &Path,
) -> Vec<String> {
    RgSearch::scoped(source_paths, root)
        .list_files()
        .with_pattern(pattern.to_owned())
        .files_with_matches()
}

/// Run `rg --word-regexp NAME` restricted to specific files.
fn rg_word_in_files(safe_name: &str, files: &[String]) -> Vec<(Location, String)> {
    if files.is_empty() {
        return vec![];
    }
    RgSearch::files(files)
        .word_regexp()
        .with_pattern(safe_name.to_owned())
        .locations_with_content()
}

/// Run a raw regex `pattern` (no `--word-regexp`) restricted to specific files.
///
/// Used for the qualified pass (`\bParent\.Name\b`) where `--word-regexp` cannot
/// be used because the pattern contains a dot.
fn rg_pattern_in_files(pattern: &str, files: &[String]) -> Vec<(Location, String)> {
    if files.is_empty() {
        return vec![];
    }
    RgSearch::files(files)
        .with_pattern(pattern.to_owned())
        .locations_with_content()
}

/// Plain word-boundary search for all occurrences of `name` under `root`.
///
/// Used by the CLI `refs --fast` subcommand.  Less precise than
/// `rg_find_references` (no package/class context) but zero-cost to run —
/// no index required.
///
/// When `source_paths` is non-empty, the search is scoped to those directories
/// instead of `root`, mirroring the scoping behaviour of other rg search functions.
pub(crate) fn rg_word_search(name: &str, root: &Path, source_paths: &[String]) -> Vec<Location> {
    RgSearch::scoped(source_paths, root)
        .word_regexp()
        .with_pattern(regex_escape(name))
        .locations()
}

fn parse_rg_line_with_content_rooted(line: &str, root: &Path) -> Option<(Location, String)> {
    let mut parts = line.splitn(4, ':');
    let file = parts.next()?;
    let line_num: u32 = parts.next()?.trim().parse().ok()?;
    let col: u32 = parts.next()?.trim().parse().ok()?;
    let content = parts.next().unwrap_or("").to_string();

    let path = std::path::Path::new(file);
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };

    // Only canonicalize when the path is not already absolute (e.g. relative workspace root).
    // Avoid the syscall-per-result cost on large workspaces where the root is always absolute.
    let abs_path = if abs_path.is_absolute() {
        abs_path
    } else {
        abs_path.canonicalize().unwrap_or(abs_path)
    };
    let uri = Url::from_file_path(&abs_path).ok()?;
    let pos = Position::new(line_num.saturating_sub(1), col.saturating_sub(1));
    Some((
        Location {
            uri,
            range: Range::new(pos, pos),
        },
        content,
    ))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "rg_tests.rs"]
mod tests;
