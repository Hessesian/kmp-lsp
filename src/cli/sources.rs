//! CLI `sources` subcommand — list auto-discovered source roots.
//!
//! Shows what `workspace.json` and/or standard build layout detection
//! would contribute as source roots, which paths actually exist, and
//! where each path was found. Useful for verifying project setup without
//! starting the LSP server.

use std::path::Path;

use serde::Serialize;

#[derive(Debug, Serialize)]
pub(crate) struct SourceRoot {
    pub path: String,         // lossy-UTF8; always serializable
    pub origin: &'static str, // "workspace.json" | "build-layout"
    pub exists: bool,
}

/// Collect all auto-discovered source roots for the given workspace root.
pub(crate) fn discover(workspace_root: &Path) -> Vec<SourceRoot> {
    let mut roots: Vec<SourceRoot> = Vec::new();

    let json_paths = crate::workspace_json::load_source_paths(workspace_root);
    for path in &json_paths {
        roots.push(SourceRoot {
            exists: path.is_dir(),
            path: path.to_string_lossy().into_owned(),
            origin: "workspace.json",
        });
    }

    if json_paths.is_empty() {
        for path in crate::workspace_json::detect_build_layout_source_paths(workspace_root) {
            roots.push(SourceRoot {
                exists: path.is_dir(),
                path: path.to_string_lossy().into_owned(),
                origin: "build-layout",
            });
        }
    }

    roots
}

pub(crate) fn run_sources(workspace_root: &Path, json: bool, explain: bool) {
    let roots = discover(workspace_root);

    if roots.is_empty() {
        if !json {
            eprintln!(
                "No source roots found. Add a workspace.json or a build.gradle.kts / pom.xml."
            );
        } else {
            println!("[]");
        }
        std::process::exit(1);
    }

    if json {
        match serde_json::to_string(&roots) {
            Ok(json_str) => println!("{json_str}"),
            Err(e) => {
                eprintln!("error: failed to serialize sources: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // ── explain mode: detailed diagnostics ─────────────────────────────────────
    if explain {
        let has_settings = ["settings.gradle.kts", "settings.gradle"]
            .iter()
            .any(|f| workspace_root.join(f).exists());
        let has_gradle_build = ["build.gradle.kts", "build.gradle"]
            .iter()
            .any(|f| workspace_root.join(f).exists());
        let has_pom = workspace_root.join("pom.xml").exists();
        let has_workspace_json = workspace_root.join("workspace.json").exists();

        println!("── Source Root Diagnostics ──");
        println!("Workspace root: {}", workspace_root.display());
        println!();
        println!("Build files detected:");
        println!(
            "  workspace.json: {}",
            if has_workspace_json { "✅" } else { "❌" }
        );
        println!(
            "  settings.gradle: {}",
            if has_settings { "✅" } else { "❌" }
        );
        println!(
            "  build.gradle: {}",
            if has_gradle_build { "✅" } else { "❌" }
        );
        println!("  pom.xml: {}", if has_pom { "✅" } else { "❌" });
        println!();
        println!(
            "Source roots ({}/{} exist):",
            roots.iter().filter(|r| r.exists).count(),
            roots.len()
        );
        for root in &roots {
            println!(
                "  {} {} (origin: {})",
                if root.exists { "✅" } else { "❌" },
                root.path,
                root.origin,
            );
        }
        let miss_count = roots.iter().filter(|r| !r.exists).count();
        if miss_count > 0 {
            println!();
            println!("Missing paths — check that the directory exists or run `kotlin-lsp extract-sources`.");
        }
        if !has_settings && !has_gradle_build && !has_pom && !has_workspace_json {
            println!();
            println!("No build files found. Add a workspace.json, build.gradle.kts, or pom.xml.");
        }
        return;
    }

    // Text output: one existing path per line, no decoration. Missing paths
    // and tips go to stderr so stdout stays parseable by callers.
    for root in roots.iter().filter(|r| r.exists) {
        println!("{}", root.path);
    }

    let missing = roots.iter().filter(|r| !r.exists).count();
    if missing > 0 {
        eprintln!("{missing} path(s) configured but missing on disk (use --json for details).");
    }

    if roots.iter().any(|r| r.origin == "build-layout") {
        eprintln!(
            "Tip: `kotlin-lsp extract-sources` unpacks Gradle *-sources.jar for hover/go-to-def."
        );
    }
}
