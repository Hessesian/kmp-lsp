//! POC: missing-import diagnostic + precision measurement over a workspace.
//!
//! Flags a bare class / function reference when BOTH hold:
//!   1. it is importable — `fqns_for_name` knows at least one concrete FQN for it
//!      (which excludes stdlib/default-import names, since their sources aren't
//!      indexed, and anything from an unindexed jar);
//!   2. it is NOT reachable from the file's own scope (`resolve_in_scope_strict`):
//!      no local/param decl, explicit import, same-package, or non-stdlib star import.
//!
//! Run against a *compiling* project (e.g. nowInAndroid): every flag is by definition
//! a false positive, so the aggregate flag count is a direct precision signal — the
//! ideal is zero. Flagged-name frequencies point at systematic gaps in our scope model.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use tower_lsp::lsp_types::Url;
use tree_sitter::Node;

use crate::indexer::live_tree::{lang_for_path, parse_live};
use crate::indexer::{all_lambda_receivers_at, Indexer};
use crate::queries::{KIND_CALL_EXPR, KIND_SIMPLE_IDENT, KIND_TYPE_IDENT};
use crate::resolver::{fqns_for_name, receiver_provides_member, resolve_in_scope_strict};
use crate::types::CursorPos;

/// A flagged reference: the bare name and where it occurs.
struct Flag {
    name: String,
    line: u32,
}

/// A candidate bare reference plus the extension-receiver types in scope at that point.
struct Candidate {
    name: String,
    line: u32,
    col: u32,
    receivers: Vec<String>,
}

/// The extension-receiver type of `fun Receiver.name(...)`, if any — the `user_type`
/// immediately followed by `.` before the function name.
fn extension_receiver_of(fn_node: Node, src: &[u8]) -> Option<String> {
    let mut c = fn_node.walk();
    let children: Vec<Node> = fn_node.children(&mut c).collect();
    for (i, child) in children.iter().enumerate() {
        if child.kind() == "user_type"
            && children
                .get(i + 1)
                .map(|n| n.kind() == ".")
                .unwrap_or(false)
        {
            let mut cc = child.walk();
            for sub in child.children(&mut cc) {
                if sub.kind() == KIND_TYPE_IDENT {
                    return sub.utf8_text(src).ok().map(|s| s.to_owned());
                }
            }
        }
    }
    None
}

/// Names declared by a `type_parameters` node (`<State, Effect: Bound>` → State,
/// Effect). The name is the first identifier child of each `type_parameter`; the
/// (deeper) bound identifier is left for normal collection.
fn collect_type_param_names(tp_node: Node, src: &[u8], out: &mut HashSet<String>) {
    let mut c = tp_node.walk();
    for tp in tp_node.children(&mut c) {
        if tp.kind() != "type_parameter" {
            continue;
        }
        let mut cc = tp.walk();
        for child in tp.children(&mut cc) {
            if child.kind() == KIND_TYPE_IDENT || child.kind() == KIND_SIMPLE_IDENT {
                if let Ok(t) = child.utf8_text(src) {
                    out.insert(t.to_owned());
                }
                break; // first identifier is the parameter name
            }
        }
    }
}

/// Walk the CST collecting candidate bare references: call-expression callees
/// (functions/constructors) and type identifiers (classes). Qualified/member refs,
/// import/package headers, and generic type parameters in scope are skipped to stay
/// high-confidence.
fn collect_candidates(
    node: Node,
    src: &[u8],
    type_params: &HashSet<String>,
    receivers: &[String],
    out: &mut Vec<Candidate>,
) {
    let kind = node.kind();

    // Don't descend into import/package declarations — their identifiers aren't uses.
    if kind == "import_header" || kind == "package_header" {
        return;
    }

    // Type parameters declared on this node (`class Foo<T>` / `fun <R> bar()`) are in
    // scope for its whole subtree and must never be flagged as missing imports.
    let mut child_scope: Option<HashSet<String>> = None;
    {
        let mut c = node.walk();
        for child in node.children(&mut c) {
            if child.kind() == "type_parameters" {
                let mut s = type_params.clone();
                collect_type_param_names(child, src, &mut s);
                child_scope = Some(s);
                break;
            }
        }
    }
    let active = child_scope.as_ref().unwrap_or(type_params);

    // An extension-function receiver (`fun Receiver.f()`) puts the receiver type's
    // members in scope for the function body.
    let mut child_receivers: Option<Vec<String>> = None;
    if kind == "function_declaration" {
        if let Some(r) = extension_receiver_of(node, src) {
            let mut v = receivers.to_vec();
            v.push(r);
            child_receivers = Some(v);
        }
    }
    let active_receivers = child_receivers.as_deref().unwrap_or(receivers);

    if kind == KIND_CALL_EXPR {
        if let Some(callee) = node.child(0) {
            // Only a *bare* callee (`Foo(...)` / `bar(...)`), not `recv.method(...)`.
            if callee.kind() == KIND_SIMPLE_IDENT {
                if let Ok(text) = callee.utf8_text(src) {
                    out.push(Candidate {
                        name: text.to_owned(),
                        line: callee.start_position().row as u32,
                        col: callee.start_position().column as u32,
                        receivers: active_receivers.to_vec(),
                    });
                }
            }
        }
    } else if kind == KIND_TYPE_IDENT {
        // Skip the trailing segment of a qualified type (`a.b.Foo`): if the previous
        // sibling is a `.`, this identifier is already qualified and needs no import.
        let qualified = node
            .prev_sibling()
            .map(|s| s.kind() == ".")
            .unwrap_or(false);
        if !qualified {
            if let Ok(text) = node.utf8_text(src) {
                // A generic type parameter in scope (`<State, Effect>`) needs no import.
                if !active.contains(text) {
                    out.push(Candidate {
                        name: text.to_owned(),
                        line: node.start_position().row as u32,
                        col: node.start_position().column as u32,
                        receivers: active_receivers.to_vec(),
                    });
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_candidates(child, src, active, active_receivers, out);
    }
}

/// Detect missing-import flags for one already-indexed workspace file.
fn flags_for_file(indexer: &Indexer, uri: &Url, source: &str) -> Vec<Flag> {
    let Some(lang) = lang_for_path(uri.path()) else {
        return vec![];
    };
    let Some(doc) = parse_live(source, lang) else {
        return vec![];
    };
    let bytes = source.as_bytes();
    // Store a live tree so the implicit-receiver inference uses the CST path (robust
    // to multi-line builder calls), as it would in the running LSP. Removed below.
    indexer.store_live_tree(uri, source);
    let mut candidates = Vec::new();
    collect_candidates(
        doc.tree.root_node(),
        bytes,
        &HashSet::new(),
        &[],
        &mut candidates,
    );

    // `importable` / `in-scope` are per-name; only the receiver check is per-occurrence,
    // so cache the first two and dedupe flags by name (keep the first occurrence).
    let mut importable: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
    let mut in_scope: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
    let mut flagged: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    for c in &candidates {
        // (1) importable — we know a concrete FQN to add (excludes stdlib/unindexed).
        if !*importable
            .entry(&c.name)
            .or_insert_with(|| !fqns_for_name(indexer, &c.name).is_empty())
        {
            continue;
        }
        // (2) reachable from the file's own scope.
        if *in_scope
            .entry(&c.name)
            .or_insert_with(|| resolve_in_scope_strict(indexer, &c.name, uri))
        {
            continue;
        }
        // (3) provided by an enclosing extension receiver (`fun Receiver.f()`).
        if c.receivers
            .iter()
            .any(|r| receiver_provides_member(indexer, r, &c.name))
        {
            continue;
        }
        // (4) provided by an implicit lambda receiver (`LazyColumn { item {} }`):
        // check every enclosing receiver in scope at this position — Kotlin resolves
        // an implicit-receiver call against *every* enclosing receiver (innermost
        // first), so a bare `item()` inside `with(x) { }` nested in a builder belongs
        // to the outer `LazyListScope` even if `x` lacks it.
        let pos = CursorPos {
            line: c.line as usize,
            utf16_col: c.col as usize,
        };
        if all_lambda_receivers_at(pos, indexer, uri)
            .iter()
            .any(|r| receiver_provides_member(indexer, r, &c.name))
        {
            continue;
        }
        flagged.entry(&c.name).or_insert(c.line);
    }
    indexer.remove_live_tree(uri); // bound memory across the corpus
    flagged
        .into_iter()
        .map(|(name, line)| Flag {
            name: name.to_owned(),
            line,
        })
        .collect()
}

/// Run the POC over every indexed workspace `.kt`/`.java` file under `root` and print
/// per-file flags plus an aggregate precision summary.
pub(crate) async fn run_missing_imports(root: &Path) {
    eprintln!("Indexing {}...", root.display());
    let index = super::run::build_index(root, true).await;
    eprintln!(
        "Indexed: {} files, {} symbols",
        index.files.len(),
        index.definitions.len()
    );

    // The LSP warms the compiled-JAR index in the background; the CLI must do it
    // explicitly, otherwise every library import (`androidx.*`, etc.) looks unresolved.
    let gradle_paths = crate::indexer::jar::scan_gradle_jars(None);
    if !gradle_paths.is_empty() {
        let mut sidecar = index.jar_sidecar.lock().unwrap_or_else(|e| e.into_inner());
        let n = crate::indexer::jar::index_jars(&index, &gradle_paths, &mut sidecar);
        eprintln!(
            "Indexed {n} compiled-jar symbols from {} gradle jars",
            gradle_paths.len()
        );
    } else {
        eprintln!("(no gradle jars found — library imports will look unresolved)");
    }

    // Workspace source files only (exclude library/JAR URIs).
    let mut uris: Vec<String> = index
        .files
        .iter()
        .map(|e| e.key().clone())
        .filter(|u| u.starts_with("file://") && !index.library_uris.contains(u))
        .filter(|u| u.ends_with(".kt") || u.ends_with(".java"))
        .collect();
    uris.sort();

    let mut total_files = 0usize;
    let mut files_with_flags = 0usize;
    let mut total_flags = 0usize;
    let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
    let mut sample: BTreeMap<String, String> = BTreeMap::new();

    for uri_str in &uris {
        let Ok(uri) = Url::parse(uri_str) else {
            continue;
        };
        let Ok(path) = uri.to_file_path() else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        total_files += 1;
        let flags = flags_for_file(&index, &uri, &source);
        if flags.is_empty() {
            continue;
        }
        files_with_flags += 1;
        total_flags += flags.len();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        for f in &flags {
            *by_name.entry(f.name.clone()).or_insert(0) += 1;
            sample
                .entry(f.name.clone())
                .or_insert_with(|| format!("{rel}:{}", f.line + 1));
            println!("{rel}:{} [missing-import]: {}", f.line + 1, f.name);
        }
    }

    // Aggregate: on a compiling project every flag is a false positive.
    eprintln!("\n──────── missing-import POC summary ────────");
    eprintln!("files scanned     : {total_files}");
    eprintln!("files with flags  : {files_with_flags}");
    eprintln!("total flags (= FP): {total_flags}");
    eprintln!("distinct names    : {}", by_name.len());
    eprintln!("\ntop flagged names (frequency — systematic FP sources):");
    let mut ranked: Vec<(&String, &usize)> = by_name.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (name, count) in ranked.iter().take(30) {
        let where_ = sample.get(*name).map(String::as_str).unwrap_or("");
        eprintln!("  {count:>4}  {name:<32} e.g. {where_}");
    }
}
