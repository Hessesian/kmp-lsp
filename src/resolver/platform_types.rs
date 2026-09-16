//! Kotlin/Swift stdlib package detection and the built-in-type → JVM-platform-type
//! fallback (see `docs/superpowers/specs/2026-08-27-kotlin-builtin-type-platform-mapping-design.md`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tower_lsp::lsp_types::{Location, Url};

use crate::indexer::Indexer;

use super::find::find_name_in_uri;

/// Returns true for packages whose sources aren't present in a typical project.
///
/// Kotlin automatically imports `kotlin.*` and `kotlin.collections.*` etc.
/// Android projects don't ship `android.*` / `androidx.*` sources by default.
/// Swift: framework imports like Foundation, UIKit, etc. have no local sources.
pub(crate) fn is_stdlib(pkg: &str) -> bool {
    // Check dotted prefixes before splitting.
    if pkg.starts_with("com.sun") {
        return true;
    }
    let first = pkg.split('.').next().unwrap_or("");
    matches!(
        first,
        "kotlin" | "java" | "javax" | "android" | "androidx" | "sun"
        // Swift standard frameworks
        | "Foundation" | "UIKit" | "SwiftUI" | "Combine" | "CoreData"
        | "CoreGraphics" | "CoreLocation" | "MapKit" | "AVFoundation"
        | "WebKit" | "StoreKit" | "GameKit" | "ARKit" | "RealityKit"
        | "Swift" | "ObjectiveC" | "Darwin" | "Dispatch" | "os"
    )
}

/// Kotlin's own "mapped types" — compiler-intrinsic built-in types with NO
/// compiled `.class` file in kotlin-stdlib's JAR at all. The Kotlin compiler
/// substitutes their real JVM platform-type equivalent directly into
/// bytecode, so a class-file-scanning indexer (this project's JAR sidecar)
/// can never find a class file that doesn't exist. Real corpus evidence: a
/// typical Android/Kotlin workspace has 13 same-named JAR/workspace
/// candidates for bare `String`, and NONE of them is the real class — see
/// `docs/superpowers/specs/2026-08-27-kotlin-builtin-type-platform-mapping-design.md`.
///
/// Deliberately narrow (`String`/`CharSequence`, the `kotlin.collections.*`
/// interfaces, and the 8 primitive scalar types, the ones directly
/// evidenced by measurement so far) — Kotlin has roughly 20 mapped types in
/// total (`Any`/`Throwable`/`Number`/`Comparable`/...), but adding the rest
/// speculatively, without real corpus evidence each one is actually hit,
/// would violate the same "evidenced-only, not a broad heuristic"
/// discipline [`DENYLISTED_PACKAGE_PREFIXES`] already established.
/// Extending this list is a mechanical follow-up once a real gap is
/// measured, not a redesign.
const KOTLIN_BUILTIN_TYPE_PLATFORM_EQUIVALENTS: &[(&str, &str)] = &[
    ("String", "java.lang.String"),
    ("CharSequence", "java.lang.CharSequence"),
    // kotlin.collections.* interfaces -- same "compiler-intrinsic mapped
    // type, no compiled .class anywhere in kotlin-stdlib's JAR" shape as
    // String/CharSequence above (verified the same way: `unzip -l
    // kotlin-stdlib-*.jar | grep List.class` etc. -> no output). Kotlin's
    // read-only/mutable pairs (`List`/`MutableList`, ...) are a compile-time
    // view over ONE real platform interface -- both map to the same target,
    // which is why e.g. `MutableList` and `List` share a value here.
    ("List", "java.util.List"),
    ("MutableList", "java.util.List"),
    ("Set", "java.util.Set"),
    ("MutableSet", "java.util.Set"),
    ("Map", "java.util.Map"),
    ("MutableMap", "java.util.Map"),
    ("Collection", "java.util.Collection"),
    ("MutableCollection", "java.util.Collection"),
    ("Iterable", "java.lang.Iterable"),
    ("MutableIterable", "java.lang.Iterable"),
    ("Iterator", "java.util.Iterator"),
    ("MutableIterator", "java.util.Iterator"),
    // Kotlin's 8 primitive scalar types -- same compiler-intrinsic shape:
    // verified none of `Int`/`Long`/`Double`/`Float`/`Boolean`/`Byte`/`Short`/`Char`
    // has a compiled `.class` file in kotlin-stdlib's JAR either. `Char` is
    // the one name mismatch (-> `Character`, not `Char`) -- handled the same
    // way `MutableList` -> `java.util.List` already is, by looking up the
    // platform type's own simple name rather than the original Kotlin one.
    ("Int", "java.lang.Integer"),
    ("Long", "java.lang.Long"),
    ("Double", "java.lang.Double"),
    ("Float", "java.lang.Float"),
    ("Boolean", "java.lang.Boolean"),
    ("Byte", "java.lang.Byte"),
    ("Short", "java.lang.Short"),
    ("Char", "java.lang.Character"),
];

/// Fallback for a Kotlin compiler-intrinsic built-in type name (see
/// [`KOTLIN_BUILTIN_TYPE_PLATFORM_EQUIVALENTS`]), tried in `resolve_chain`'s
/// tail *ahead of* the global-definitions lookup, not behind it: for the ~22
/// names in that table, the real platform declaration outranks any
/// same-named index/JAR candidate, because those names have no compiled
/// `.class` anywhere and every same-named candidate is therefore a decoy —
/// verified on the Moneta corpus, e.g. bare `List` has 32 same-named
/// candidates and the tie-break machinery picks a body-less
/// `kotlin.collections` decoy from among them, and `MutableSet` has exactly
/// one same-named candidate (a `kotlin-gradle-plugin` decoy) that wins
/// outright as a unique match. Steps 1–4.5 (local declaration, explicit
/// imports, same package, star imports, class hierarchy) still run first, so
/// a workspace file that genuinely declares its own `class Double` is
/// unaffected — this fallback only ever fires once those have already
/// declined.
///
/// Re-derives the Android SDK sources root via the already-existing
/// [`crate::workspace_json::detect_android_sdk_source_paths`] (no new
/// discovery mechanism — reuses Primitive B's own function) and, if the
/// expected `<root>/java/lang/String.java`-shaped file exists on disk,
/// indexes it on demand — the same `std::fs::read_to_string` +
/// `index_content` pattern `resolve_chain`'s own step 0.5 already uses for
/// "the caller's own file isn't indexed yet", just triggered by a known
/// built-in name instead. One-time cost per session per type: once indexed,
/// the file is permanently cached like any other, so this filesystem lookup
/// only ever runs for the first `String`/`CharSequence` resolution.
///
/// Scoped to Android projects for now — a plain JVM/non-Android Kotlin
/// project has no equivalent auto-detected `java.lang.*` source bundle;
/// locating a JDK's own bundled sources is a separate discovery problem,
/// not addressed here (see the design doc's explicit scope boundary).
pub(crate) fn resolve_kotlin_builtin_type_platform_equivalent(
    indexer: &Indexer,
    name: &str,
) -> Vec<Location> {
    let Some(&(_, platform_fqn)) = KOTLIN_BUILTIN_TYPE_PLATFORM_EQUIVALENTS
        .iter()
        .find(|&&(builtin, _)| builtin == name)
    else {
        return vec![];
    };
    // The platform type's own simple name, NOT `name` -- some entries
    // (`MutableList` -> `java.util.List`) map a Kotlin-only spelling onto a
    // real interface declared under a DIFFERENT simple name; searching the
    // target file for a symbol literally called "MutableList" would always
    // come up empty.
    let Some(platform_simple_name) = platform_fqn.rsplit('.').next() else {
        return vec![];
    };
    let Some(workspace_root) = indexer.workspace_root.get() else {
        return vec![];
    };
    let relative_path = platform_fqn.replace('.', "/") + ".java";
    for sdk_source_root in android_sdk_source_roots(&workspace_root) {
        let file_path = sdk_source_root.join(&relative_path);
        let Ok(file_uri) = Url::from_file_path(&file_path) else {
            continue;
        };
        let file_uri_str = file_uri.as_str();
        if !indexer.files.contains_key(file_uri_str) {
            let Ok(content) = std::fs::read_to_string(&file_path) else {
                continue;
            };
            indexer.index_content(&file_uri, &content);
        }
        let locs = find_name_in_uri(indexer, platform_simple_name, file_uri_str);
        if !locs.is_empty() {
            return locs;
        }
    }
    vec![]
}

/// Memoized [`crate::workspace_json::detect_android_sdk_source_paths`], keyed
/// on the workspace root it was computed for.
///
/// That function does real filesystem work on every call — a
/// `local.properties` read, an `is_dir`, and a `read_dir` of `sdk/sources/` —
/// plus a `log::info!` on every success. Harmless while the built-in-type
/// fallback only ran after the whole resolution chain had already declined;
/// not harmless now that it runs ahead of the tail for every name in
/// [`KOTLIN_BUILTIN_TYPE_PLATFORM_EQUIVALENTS`], which is the hottest name
/// class in a Kotlin corpus and sits on the hover/inlay path.
///
/// Keyed on the root path rather than cached once, because
/// `WorkspaceRoot::set` can change the root mid-session. An SDK installed
/// *after* the first probe is not picked up until the root changes — the same
/// staleness the workspace scan already has, since it reads these paths once
/// at startup (`src/workspace/mod.rs:133`, `src/cli/run.rs:284`).
fn android_sdk_source_roots(workspace_root: &Path) -> Vec<PathBuf> {
    static DETECTED_ROOTS: Mutex<Option<(PathBuf, Vec<PathBuf>)>> = Mutex::new(None);
    let Ok(mut detected) = DETECTED_ROOTS.lock() else {
        return crate::workspace_json::detect_android_sdk_source_paths(workspace_root);
    };
    if let Some((cached_root, cached_paths)) = detected.as_ref() {
        if cached_root == workspace_root {
            return cached_paths.clone();
        }
    }
    let paths = crate::workspace_json::detect_android_sdk_source_paths(workspace_root);
    *detected = Some((workspace_root.to_path_buf(), paths.clone()));
    paths
}
