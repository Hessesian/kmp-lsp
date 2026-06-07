//! CLI `doctor` subcommand — project diagnostics for agents.
//!
//! Checks:
//! - Source roots are auto-discovered
//! - Cache is fresh / available
//! - Library sources are extracted (`~/.kotlin-lsp/sources/`)
//! - No suspicious gitignored .kt/.java files
//! - Workspace root is set

use std::path::{Path, PathBuf};

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn home() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
    #[cfg(not(unix))]
    {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
    }
}

fn cache_dir() -> Option<PathBuf> {
    // XDG_CACHE_HOME or ~/.cache
    std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".cache")))
}

/// Count files recursively with the given extension.
fn count_files(dir: &Path, ext: &str) -> usize {
    if !dir.is_dir() {
        return 0;
    }
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|e| {
                    let e = e.to_string_lossy();
                    ext == &*e || ext.trim_start_matches('.') == &*e
                })
                .unwrap_or(false)
        })
        .count()
}

/// Compute total size of a directory in bytes.
fn dir_size(dir: &Path) -> u64 {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Check if a tool is available on PATH.
fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(name);
            if full.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::metadata(&full).ok().and_then(|m| {
                        if m.permissions().mode() & 0o111 != 0 {
                            Some(full)
                        } else {
                            None
                        }
                    })
                }
                #[cfg(not(unix))]
                Some(full)
            } else {
                None
            }
        })
    })
}

/// Quick check if a path has a .git ancestor.
fn has_git_ancestor(path: &Path) -> bool {
    path.ancestors().any(|a| a.join(".git").exists())
}

// ─── Doctor runner ──────────────────────────────────────────────────────────

pub(crate) fn run_doctor(root: Option<&Path>, verbose: bool) {
    let root = root
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    println!("kotlin-lsp doctor");
    println!("  workspace root: {}", root.display());
    println!();

    let mut all_ok = true;

    // ── 1. Workspace root exists ─────────────────────────────────────────
    let root_exists = root.exists();
    if root_exists {
        println!("[✓] workspace root exists");
    } else {
        println!("[✗] workspace root does not exist: {}", root.display());
        all_ok = false;
    }

    // ── 2. Kotlin/Java/Swift files found ────────────────────────────────
    if root_exists {
        let kt_files = count_files(&root, ".kt");
        let java_files = count_files(&root, ".java");
        let swift_files = count_files(&root, ".swift");
        let total = kt_files + java_files + swift_files;
        if total > 0 {
            println!(
                "[✓] {} source files found ({} .kt, {} .java, {} .swift)",
                total, kt_files, java_files, swift_files
            );
        } else {
            println!(
                "[!] no .kt, .java, or .swift files found under {}",
                root.display()
            );
        }
    }

    // ── 3. Library sources extracted ────────────────────────────────────
    let sources_dir = home().map(|h| h.join(".kotlin-lsp").join("sources"));
    let sources_extracted = match &sources_dir {
        Some(d) if d.exists() => {
            let count = count_files(d, ".jar");
            if count > 0 {
                println!("[✓] {} library source jars extracted", count);
                true
            } else {
                println!(
                    "[!] library sources directory exists but no jars found: {}",
                    d.display()
                );
                false
            }
        }
        Some(d) => {
            println!("[!] library sources not extracted (run `kotlin-lsp extract-sources`)");
            if verbose {
                println!("     expected: {}", d.display());
            }
            false
        }
        None => {
            println!("[!] cannot determine home directory for library sources");
            false
        }
    };
    if !sources_extracted {
        all_ok = false;
    }

    // ── 4. Index cache status ───────────────────────────────────────────
    let cache_dir = cache_dir().map(|c| c.join("kotlin-lsp"));
    if let Some(cd) = &cache_dir {
        if cd.exists() {
            let size = dir_size(cd);
            println!("[✓] index cache: {} ({} KB)", cd.display(), size / 1024);
            if verbose {
                if let Ok(entries) = std::fs::read_dir(cd) {
                    for e in entries.flatten() {
                        let path = e.path();
                        if path.is_dir() {
                            let sz = dir_size(&path);
                            println!(
                                "     └─ {} ({} KB)",
                                path.file_name().unwrap_or_default().to_string_lossy(),
                                sz / 1024
                            );
                        }
                    }
                }
            }
        } else {
            println!("[!] no index cache found (run `kotlin-lsp index` to build one)");
            if verbose {
                println!("     expected: {}", cd.display());
            }
        }
    }

    // ── 5. Ignored .kt files under common source dirs ───────────────────
    if verbose && root_exists {
        let common_src_dirs = ["src", "app/src", "shared/src", "androidApp/src"];
        for dir_name in &common_src_dirs {
            let candidate = root.join(dir_name);
            if candidate.exists() && candidate.is_dir() && !has_git_ancestor(&candidate) {
                println!(
                    "[!] source directory not git-tracked: {}",
                    candidate.display()
                );
            }
        }
    }

    // ── 6. Runtime tools ────────────────────────────────────────────────
    let rg_available = which("rg").is_some();
    if rg_available {
        println!("[✓] rg (ripgrep) found on PATH");
    } else {
        println!("[!] rg (ripgrep) not found — cross-file searches will fail");
        all_ok = false;
    }

    let fd_available = which("fd").is_some();
    if fd_available {
        println!("[✓] fd found on PATH");
    } else if verbose {
        println!("[!] fd not found — file discovery may be slower");
    }

    // ── Summary ─────────────────────────────────────────────────────────
    println!();
    if all_ok {
        println!("All checks passed.");
    } else {
        println!("Some checks failed — see [✗] items above.");
    }
}
