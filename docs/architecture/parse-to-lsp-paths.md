# Parse → LSP Paths (reference)

*Generated reference — 2026-06-30. Grounded in source at that date; update when
pipelines change. Line numbers are 1-based.*

---

## Pipeline overview

```
   ┌─ textDocument/didOpen ─────────────────────────────────────────────────┐
   │  src/backend/mod.rs:161 → Event::FileOpened → Actor channel           │
   └─ textDocument/didChange ───────────────────────────────────────────────┘
      src/backend/mod.rs:178
      │   (synchronous, before event is queued)
      ├── Indexer::remove_live_tree   (invalidate stale CST)
      └── Indexer::set_live_lines     src/indexer.rs:693
                                      (stores raw content lines)
          │
          └── Event::FileChanged → Actor channel
                     │
              ┌──────┴──────────────────────────────────┐
              │  workspace/actor.rs                     │
              │  DocumentHandler / FileChangeHandler    │
              │  src/workspace/{document,file_change}_handler.rs
              └──────┬──────────────────────────────────┘
                     │
         ┌───────────┴────────────────────┐
         │  INDEX PATH (every file)       │  LIVE-TREE PATH (open files only)
         ▼                                ▼
  Indexer::index_content           parse_live (live_tree.rs)
  src/indexer/apply.rs:1042        src/indexer/live_tree.rs:57
         │                                │
  parse_file → parse_by_extension         │ tree-sitter Tree + raw bytes
  src/indexer/apply.rs:498                └──→ LiveDoc { tree, bytes }
  src/parser.rs:286                            stored in live_trees DashMap
         │                                     (key = URI, open files only)
  Language::parser().parse(content)
         │
  FileData { lines, symbols, imports,
             sigs, packages, syntax_errors }
         │
  apply_file_result
         │
  Shared maps in Arc<Indexer>:
    definitions       – name → [Location]
    packages          – uri → package
    subtypes          – type → [subtype]
    extension_by_receiver – receiver → [ext_fn]
    files             – uri → Arc<FileData>
    type_annotations  – uri → {name: type}
         │
         └──── JAR symbols added separately via sidecar (src/sidecar.rs)

                    ╔══════════════════════════════════════════╗
                    ║  Arc<Indexer>  (shared, concurrent read) ║
                    ╚═══════════════╤══════════════════════════╝
                                    │
              LSP request handler   │  (LanguageServer impl, mod.rs:71)
                                    │
               ┌────────────────────┼────────────────────────────────────┐
               │  CursorContext     │  src/backend/cursor.rs:42          │
               │  (identifier-based │  features: hover, goto-def, refs)  │
               │   word + qualifier + contextual ReceiverType            │
               │   infer_receiver_type (resolver/infer.rs) = BOTH       │
               └────────────────────┼────────────────────────────────────┘
                                    │
               ┌────────────────────┼────────────────────────────────────┐
               │  STRING engine     │  src/indexer/resolution.rs         │
               │  (index-based)     │  resolve_symbol_info:175           │
               │  locate_symbol → FileData → enrich_symbol → signature  │
               │  Also: resolver/resolve.rs, resolver/complete.rs,      │
               │        resolver/infer.rs (line-scan type inference)     │
               └────────────────────┼────────────────────────────────────┘
                                    │
               ┌────────────────────┼────────────────────────────────────┐
               │  CST engine        │  src/indexer/infer/*               │
               │  (live-tree-based) │  src/semantic_tokens/resolve.rs    │
               │  live_doc_or_parse → Tree walk                         │
               │  infer_expr_type, infer_lambda_param_type_at           │
               └────────────────────┴────────────────────────────────────┘
```

**Two distinct stores underlie every feature:**

- **Index store** (`Arc<Indexer>` maps): populated by `index_content` → `parse_by_extension`.
  String-keyed, survives file close. Drives symbol lookup, completion, references.
- **Live-tree store** (`live_trees` DashMap): populated by `parse_live` on `didOpen`/`didChange`.
  CST per open file. Required for positional tree-sitter queries. Evicted on close.

---

## Per-feature paths

> **Resolution engine key:**
> - **STRING** — operates on indexed `FileData` (lines, symbols, signatures) without a CST.
> - **CST** — requires a live tree-sitter `Tree` (`LiveDoc`).
> - **BOTH** — uses both; column "Resolution engine" notes which is primary.

| Feature | Handler entry (file:line) | State read | Resolution engine | Notes |
|---------|--------------------------|------------|-------------------|-------|
| **hover** | `src/backend/handlers.rs:8` → `features/hover.rs:19` | live_tree (CursorContext), index (FileData) | BOTH — STRING primary | 3 branches: `contextual_lambda_hover`, `contextual_receiver_hover`, `regular_symbol_hover`. All converge on `resolve_symbol_info` (STRING). CursorContext builds `contextual` via `infer_receiver_type` which has a CST fallback (`infer_variable_type_from_cst`). |
| **completion** | `src/backend/actions.rs:7` → `features/completion.rs:116` | live_lines + index | BOTH — STRING primary | `run_completions` → `complete_symbol_with_context` (`resolver/complete.rs`). Dot-completion resolves receiver via `infer_receiver_type_at` which walks the CST. Bare completion is index-only. |
| **semantic tokens** | `src/backend/mod.rs` (LanguageServer impl) → `semantic_tokens/mod.rs:138` | live_tree | BOTH — CST primary | Phase 1: pure CST syntax walk (kotlin.rs/java.rs). Phase 1b: param-use walk. Phase 2: `resolve::walk_references` → `expression_type` (`semantic_tokens/resolve.rs`) calls `infer_lambda_param_type_at` (CST, `indexer/infer/`) and `infer_variable_type` (STRING fallback). |
| **inlay hints** | `src/backend/handlers.rs:73` → `inlay_hints.rs:29` | live_tree (or re-parse from live_lines) | BOTH — CST primary | `cst_hints` walks live tree; for `it`/`this` nodes calls `infer_receiver_type` which delegates to `infer_lambda_param_type_at` (CST, `indexer/infer/receiver.rs`). Falls back to re-parsing `live_lines` if no live tree. |
| **diag: when-exhaustiveness** | actor `workspace/document_handler.rs:199` → `features/fill_when.rs:151` | live_tree + index | BOTH — CST primary | `collect_when_nodes` walks CST; `resolve_type_members` queries index for sealed/enum members. Pushed by actor on open/change/scan-complete. Suppressed during workspace scan. |
| **diag: call-arg count** | actor `workspace/document_handler.rs:201` → `features/call_arg_diagnostics.rs:22` | live_tree + index | BOTH — CST primary | `collect_call_nodes` walks CST; `check_call_args` looks up signatures via `sig_cache` (STRING, `indexer/infer/sig.rs`). Suppressed during JAR loading. |
| **diag: nullable-dot** | actor `workspace/document_handler.rs:202` → `features/nullable_call_diagnostics.rs:27` | live_tree + index | BOTH — CST primary | `collect_nav_nodes` walks CST for plain-`.` navigation expressions; `resolve_receiver` calls `Resolver::resolve_member` (STRING) to confirm nullable type. |
| **goto_definition** | `src/backend/nav.rs:10` → `features/definition.rs:130` | index | STRING (+ rg fallback) | `find_definition_qualified` → `resolve_locations` (resolver/resolve.rs). Falls back to `rg_resolve`. CursorContext.contextual provides pre-resolved location for `it`/`this`/lambda params (CST-assisted). |
| **goto_implementation** | `src/backend/nav.rs:24` → `features/implementation.rs:21` | index | STRING | `find_type_implementations` / `find_method_implementations` query subtypes map + rg. |
| **find_references** | `src/backend/handlers.rs:27` → `features/references.rs:16` | index + live_lines | STRING | `rg_locations` (ripgrep) + `add_current_file_locations` (line scan). Scope resolved via index. |
| **signature_help** | `src/backend/handlers.rs:118` → `features/signature_help.rs:19` | live_tree + index | BOTH — hard split | CST half: `call_info_at` → `cst_call_info` (indexer): extracts fn name + active param from live tree. STRING half: `find_fun_signature_with_receiver` (`indexer/infer/sig.rs`): looks up signature text from index. |
| **folding_range** | `src/backend/handlers.rs:139` → `features/folding.rs:12` → `indexer/cst_folding.rs:33` | live_tree | CST only | `cst_folding_ranges` walks `live_doc`; returns `None` if file not open. |
| **document_highlight** | `src/backend/handlers.rs:149` → `features/highlight.rs:8` | live_lines + index | STRING only | `word_and_qualifier_at` (line scan) + `definition_locations` (index map). All occurrences found by scanning live_lines. |
| **fill_when (code action)** | `src/backend/actions.rs:65` → `features/fill_when.rs:build_fill_when_action` | live_tree + index | BOTH — CST primary | Same `analyze_when` / `resolve_type_members` logic as when-exhaustiveness diagnostic; driven from code-action path instead of actor. |
| **document_symbol** | `src/backend/handlers.rs:61` | index (`FileData.symbols`) | STRING only | `file_symbols` from index; triggers on-demand `index_content` if file not indexed. |
| **workspace_symbols** | `src/backend/handlers.rs:109` → `features/workspace_symbols.rs` | index | STRING only | Fuzzy scan over `definitions` map. |
| **rename** | `src/backend/rename.rs` | index + live_lines | STRING | Index lookup for declaration site; rg to find all occurrences. |

---

## String vs CST split

This is the blast-radius map for the unified-resolution redesign.

### Pure CST (no STRING resolution)
- **folding_range** — CST tree walk only; no index needed.

### CST-primary (STRING used only for type/signature lookup)
- **semantic tokens** (phase 2)
- **inlay hints**
- **diag: when-exhaustiveness**
- **diag: call-arg count**
- **diag: nullable-dot**
- **fill_when (code action)**
- **signature_help** (CST for position, STRING for sig text — hard split; both are essential)

### STRING-primary (CST used only for receiver-type narrowing via `infer_receiver_type`)
- **hover** — STRING `resolve_symbol_info`; `CursorContext` build uses CST fallback
- **completion** — STRING `complete_dot`/`complete_bare`; `infer_receiver_type_at` uses CST for smart-cast narrowing

### Pure STRING (no CST dependency)
- **goto_definition** (+ rg fallback)
- **goto_implementation**
- **find_references** (+ rg)
- **document_highlight**
- **document_symbol**
- **workspace_symbols**
- **rename**

---

## Key cross-cutting observations

**`CursorContext` is the pre-compute seam** (`src/backend/cursor.rs:42`).
It is built once per handler invocation for identifier-based features and holds the
resolved `contextual` receiver type. Internally it calls `infer_receiver_type`
(`resolver/infer.rs`), which is itself BOTH: a string line-scan with a CST
initializer-fallback (`infer_variable_type_from_cst`). This means even nominally
STRING features (hover, goto-def) touch the CST indirectly through `CursorContext`.

**Diagnostics are actor-pushed, not handler-pulled.**
The three semantic diagnostic passes (when, call_arg, nullable_dot) run inside
`spawn_blocking` in `workspace/document_handler.rs` and `file_change_handler.rs`.
They are not driven by LSP `textDocument/diagnostic` requests.

**`live_doc_or_parse` bridges the two worlds.**
When a CST-primary feature is invoked but the live tree is absent (e.g. the file
has lines but hasn't been opened), `live_doc_or_parse`
(`src/indexer/live_tree_impl.rs:39`) reconstructs content from `live_lines` or
`FileData.lines` and re-parses. This makes CST features degrade gracefully rather
than silently return nothing.

**The redesign's primary target** is the STRING engine's fragmentation across
`resolver/resolve.rs`, `resolver/complete.rs`, `resolver/infer.rs`, and callers
that reach into `Indexer` directly. The CST engine (`indexer/infer/*`) is mostly
self-contained and not the focus of the unification.
