# Copilot Session Data - Extracted Memories (kotlin-lsp only)

Source: 75 Copilot sessions, 564,905 events, ~1000 MB
Extracted: 2026-05-27
Filtered: kotlin-lsp only (android/ios sessions excluded)

## Project

**Hessesian/kotlin-lsp** — Rust LSP server for Kotlin/Java/Swift, renaming to kmp-lsp.
Branch: main@48ed79f. Repo on GitHub.

## Key Architectural Decisions

### Indexer Architecture
- DashMap-based indexes: definitions, qualified, packages, subtypes, files
- FileData: Arc<Vec<String>> for source lines, supers: Vec<(u32, String)>, declared_names
- SymbolEntry: range (full declaration) + selection_range (identifier only)
- LSP uses TextDocumentSyncKind::FULL (didChange includes full document text)
- Workspace root: KOTLIN_LSP_WORKSPACE_ROOT env → client rootUri → ~/.config/kotlin-lsp/workspace
- Cache: ~/.cache/kotlin-lsp/<sha256-first-8-bytes>/index.bin
- Indexer::apply_workspace_result does full replace via reset_index_state() before inserting
- Queued reindex: pending_reindex AtomicBool + pending_reindex_root RwLock, last request wins

### MVI Actor Refactoring (Wave 5)
- Actor::run() and handle_event() are flat coordinators (one line per match arm)
- Handler structs: ScanHandler, FileChangeHandler, DocumentHandler
- set_root() encapsulates root_generation bump (was missing at one call site → race)
- OpQueue pattern for coalescing slow operations
- Event enum unifies all input sources (LSP, file-watcher, CLI)
- Event coalescing: drain batch queues in same loop turn

### Lambda Inference
- inline_lambda_param_type uses comma_count for multi-lambda calls
- Three parallel resolution paths being unified into CST-first pipeline
- resolve_lambda_param_type_cst: find call_expression → lambda position → function name → param type → generic substitution
- Receiver-aware resolution for extension functions on dot-chains

### Diagnostics
- call_arg_diagnostics: single-snapshot approach (&LiveDoc param)
- Previously read from two independent DashMap sources (live_trees + files) → races
- Debounce task parses tree locally from same text

### Refactoring Rules
- Move code verbatim: read fully, copy bodies, adjust self refs, delete old, verify
- Before writing new functions, grep existing implementations
- Pure functions extracted from monoliths
- Side effects at write site, not scattered at call sites
- Module-level #![allow(dead_code)] is always wrong

### Testing
- Tests in separate *_tests.rs files via #[cfg(test)] #[path="..."] mod tests;
- Never inline test blocks
- Indexer tests: src/indexer_tests.rs included from src/indexer.rs
- Env var tests: use test_helpers::with_env_var (guarded by ENV_VAR_LOCK)
- XDG_CACHE_HOME tests: use test_helpers::with_xdg_cache (guarded by XDG_CACHE_LOCK)
- Temp workspaces: write workspace.json with {"sourcePaths":[]}

### Build/Validation
- cargo test -q (or cargo test)
- cargo clippy -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines
- cargo install --path . --force (without --force, cargo skips if version unchanged)

### Code Conventions
- Visibility: pub(crate)/pub(super), crate has #![warn(unreachable_pub)]
- Extension traits: StrExt, NodeExt, LinesExt
- CursorPos { line, utf16_col } for LSP positions
- tree-sitter-kotlin 0.3: no field names, use cursor API not index arithmetic
- NodeExt::first_child_of_kind(), children_of_kind()
- Cache version bump on SymbolEntry schema changes

### Known Bugs
- Completion gate skips extension_fn_completions for non-Kotlin files (Java can call Kotlin extensions)
- CLI hover lacks extension function member support on dot-chains
- is_generic_param checks len ≤ 3 — misses StateType, EffectType

## kmp-lsp Rename (in progress)

All occurrences of "kotlin-lsp" → "kmp-lsp":
- Cargo.toml: name, bin name
- Env vars: KOTLIN_LSP_* → KMP_LSP_*
- File paths: .config/kotlin-lsp → .config/kmp-lsp, .cache/kotlin-lsp → .cache/kmp-lsp, .kotlin-lsp → .kmp-lsp
- LSP commands: kotlin-lsp/reindex → kmp-lsp/reindex, kotlin-lsp/clearCache → kmp-lsp/clearCache, kotlin-lsp/changeRoot → kmp-lsp/changeRoot
- Log prefix: [kotlin-lsp] → [kmp-lsp]

## kotlin-lsp Session Statistics (33 sessions)

| Session ID | Events | Size | First Prompt |
|---|---|---|---|
| 312296a9 | 222,841 | 405 MB | check repo issues |
| d5f583fc | 48,334 | 78 MB | load current status in ../lsp_tasks |
| af1fdd17 | 43,700 | 78 MB | check out forks |
| 53968820 | 28,890 | 53 MB | this branch was a mistake |
| 7a62e4fe | 10,684 | 20 MB | copilot struggles with refactors |
| 021ae3d0 | 8,408 | 17 MB | lsp extension modified |
| 0d270d69 | 5,450 | 11 MB | configure serena |
| 084f1ee8 | 3,838 | 7 MB | initialize serena mcp |
| 0706659b | 7,174 | 12 MB | started serena mcp |
| 9d0423af | 7,594 | 15 MB | (various) |
| de547885 | 21,354 | 39 MB | serena rg/fd storm |
| 7b95bcc5 | 307 | 0.7 MB | fix corrupted session |
| 4347b668 | 126 | 0.2 MB | mcp not working |
| (19 moresmall sessions) | | | |
