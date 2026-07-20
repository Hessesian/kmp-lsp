# Find-References Verification Layer Implementation Plan (Slice 6b)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a CST-based verification pass over find-references' existing rg-found candidates — proven-unrelated candidates get rejected (tracked, not silently dropped), everything else is labeled `CstResolved`/`NameScan` and kept, recall never regresses.

**Architecture:** A shared `classify_cursor` prologue (extracted from two already-duplicated call sites) feeds a new `receiver_type_agreement` primitive in `resolver/hierarchy.rs` (exact match, or an ascending supertype walk for inherited members). A new `features/references_verify.rs` module runs this per rg-found candidate under one request-scoped IO budget, producing a `VerifiedReferences { kept, rejected }` that `references.rs` flattens to the LSP's `Vec<Location>` at the very end. Spec: `docs/superpowers/specs/2026-07-20-cst-find-references-design.md`.

**Tech Stack:** Rust, tree-sitter, existing `InferDeps`/`classify_symbol_at`/`walk_hierarchy` seams.

## Global Constraints

- Recall is untouched: the existing rg + index candidate search (`rg_locations`, `add_current_file_locations`) produces the exact same candidate set as today. Verification runs strictly after.
- IO budget covers BOTH a candidate's disk-read fallback (uncommon — candidates are workspace files the scan usually already indexes) AND supertype-walk sidecar IPC (the real risk — `walk_hierarchy` self-limits to 3 JAR promotions per walk, but a find-references request can trigger many walks) under ONE request-scoped counter. Once exhausted, remaining candidates stay `NameScan` — budget exhaustion is never evidence of unrelatedness and must never move a candidate to `rejected`.
- Only a *proven* `Unrelated` agreement moves a candidate to `rejected`; every other outcome (`Exact`, `Inherited`, `Unresolvable`, budget-skipped) stays in `kept`.
- `rejected` is test/tracing-only — never surfaced to the LSP client.
- No abbreviated names (project rule — `ty`, `sym`-as-a-fresh-binding-name, etc. are banned; use `receiver_type`, `symbol`, full words throughout).
- Gates per commit: `cargo test` + pre-commit clippy. Final: both clippy profiles, e2e smoke, live probe.
- Branch: `refactor/cst-navigation-6b` (already exists, has 2 commits: the spec and a pre-emptive AGENTS.md compliance fix on 6a's shipped code) → PR to `refactor/unified-resolution`.

---

### Task 1: Extract `classify_cursor`; migrate `definition.rs` and `implementation.rs`

**Files:**
- Modify: `src/indexer/infer/cst_symbol.rs` (add `classify_cursor`)
- Modify: `src/indexer.rs` (add to the `cst_symbol::{...}` re-export list)
- Modify: `src/features/definition.rs:134-150` (`try_cst_resolved_definition`)
- Modify: `src/features/implementation.rs:30-54` (`find_implementation_at`)
- Test: existing suites in both files are the neutrality net — no new tests this task.

**Interfaces:**
- Produces:
```rust
// cst_symbol.rs — needs `use tower_lsp::lsp_types::Position;` added to its imports
pub(crate) fn classify_cursor(
    indexer: &Indexer,
    uri: &Url,
    position: Position,
) -> Option<SymbolAtCursor>
```

- [ ] **Step 1: Implement `classify_cursor`**

In `src/indexer/infer/cst_symbol.rs`, add `use tower_lsp::lsp_types::Position;` to the top imports, then add just above `classify_symbol_at`:

```rust
/// `classify_symbol_at`, but taking an LSP `Position` directly — the
/// `Position → CursorPos` conversion every navigation-feature call site
/// otherwise repeats.
pub(crate) fn classify_cursor(
    indexer: &Indexer,
    uri: &Url,
    position: Position,
) -> Option<SymbolAtCursor> {
    classify_symbol_at(
        indexer,
        uri,
        CursorPos {
            line: position.line as usize,
            utf16_col: position.character as usize,
        },
    )
}
```

- [ ] **Step 2: Re-export it**

In `src/indexer.rs`'s `cst_symbol::{...}` re-export line (added in slice 6a), add `classify_cursor` to the list.

- [ ] **Step 3: Migrate `definition.rs`**

Replace `try_cst_resolved_definition`'s body in `src/features/definition.rs`:

```rust
fn try_cst_resolved_definition(
    indexer: &Indexer,
    uri: &Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let sym = crate::indexer::classify_cursor(indexer, uri, position)?;
    match crate::indexer::resolve_identity(&sym, indexer, uri) {
        crate::indexer::NavigationSource::CstResolved(defs) if !defs.is_empty() => {
            locs_to_opt_response(defs.0)
        }
        _ => None,
    }
}
```

(Drops the manual `CursorPos` construction entirely — `classify_cursor` does it.)

- [ ] **Step 4: Migrate `implementation.rs`**

In `src/features/implementation.rs`, replace the top of `find_implementation_at`:

```rust
pub(crate) async fn find_implementation_at(
    indexer: &Indexer,
    uri: &Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    if let Some(sym) = classify_cursor(indexer, uri, position) {
        if let SymbolRole::Reference {
            receiver_type: Some(receiver_type),
            is_call: true,
        } = &sym.role
        {
            if let Some(response) =
                find_method_implementations(&sym.name, receiver_type, indexer, uri).await
            {
                return Some(response);
            }
        }
    }
    let (word, _) = indexer.word_and_qualifier_at(uri, position)?;
    find_implementation(&word, indexer, uri, position.line).await
}
```

Update the import line: `use crate::indexer::{classify_cursor, Indexer, SymbolRole};` (drop `classify_symbol_at`, `CursorPos` becomes unused — remove it from the `crate::types::{CursorPos, FileData}` import too if the compiler flags it; `FileData` stays).

- [ ] **Step 5: Run the full suite**

Run: `cargo test --bin kmp-lsp 2>&1 | grep -E "^test result|FAILED"`
Expected: identical pass count to before this task (pure extraction, zero behavior change) — 1551 passed per the current baseline.

- [ ] **Step 6: Commit**

```bash
git add -A src/
git commit -m "refactor(nav): extract classify_cursor — shared Position-taking prologue

definition.rs and implementation.rs both hand-rolled the identical
Position->CursorPos->classify_symbol_at prologue; slice 6b's own query-
identity code would have been a third copy (independent-critique finding)."
```

---

### Task 2: `ReceiverTypeAgreement` + `supertype_chain_contains` + `receiver_type_agreement`

**Files:**
- Modify: `src/resolver/hierarchy.rs`
- Test: create `src/resolver/hierarchy_tests.rs`, wire with `#[cfg(test)] #[path = "hierarchy_tests.rs"] mod tests;` at the bottom of `hierarchy.rs` (check whether this file already has a test module — if `hierarchy.rs` currently has none, this is a new file per the project's per-file test convention seen in `implementation_tests.rs`/`highlight_tests.rs`).

**Interfaces:**
- Consumes: `walk_hierarchy`, `CallerContext` (already in this file); `Indexer::has_type_definition` (via `crate::indexer::InferDeps`, needs importing).
- Produces:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReceiverTypeAgreement {
    Exact,
    Inherited,
    Unrelated,
    Unresolvable,
}

pub(crate) fn supertype_chain_contains(
    indexer: &Indexer,
    candidate_type: &str,
    candidate_uri: &str,
    target_type: &str,
) -> bool

pub(crate) fn receiver_type_agreement(
    indexer: &Indexer,
    candidate_type: &str,
    candidate_uri: &str,
    target_type: &str,
) -> ReceiverTypeAgreement
```

- [ ] **Step 1: Write the failing tests**

```rust
// src/resolver/hierarchy_tests.rs
use super::{receiver_type_agreement, supertype_chain_contains, ReceiverTypeAgreement};
use crate::indexer::Indexer;
use tower_lsp::lsp_types::Url;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///t{path}")).unwrap()
}

fn indexed(path: &str, src: &str) -> (Url, Indexer) {
    let u = uri(path);
    let idx = Indexer::new();
    idx.index_content(&u, src);
    (u, idx)
}

#[test]
fn exact_type_match_is_exact() {
    let (u, idx) = indexed("/D.kt", "class User\n");
    assert_eq!(
        receiver_type_agreement(&idx, "User", u.as_str(), "User"),
        ReceiverTypeAgreement::Exact
    );
}

#[test]
fn subtype_of_target_is_inherited() {
    let (u, idx) = indexed("/D.kt", "open class User\nclass DerivedUser : User()\n");
    assert!(supertype_chain_contains(&idx, "DerivedUser", u.as_str(), "User"));
    assert_eq!(
        receiver_type_agreement(&idx, "DerivedUser", u.as_str(), "User"),
        ReceiverTypeAgreement::Inherited
    );
}

#[test]
fn unrelated_indexed_type_is_unrelated() {
    let (u, idx) = indexed("/D.kt", "class User\nclass File\n");
    assert!(!supertype_chain_contains(&idx, "File", u.as_str(), "User"));
    assert_eq!(
        receiver_type_agreement(&idx, "File", u.as_str(), "User"),
        ReceiverTypeAgreement::Unrelated
    );
}

#[test]
fn unindexed_type_is_unresolvable_not_unrelated() {
    let (u, idx) = indexed("/D.kt", "class User\n");
    // "Ghost" is never declared anywhere — has_type_definition fails, so we
    // must NOT claim to have proven it's unrelated to User.
    assert_eq!(
        receiver_type_agreement(&idx, "Ghost", u.as_str(), "User"),
        ReceiverTypeAgreement::Unresolvable
    );
}

/// House decoy: a two-level hierarchy — the target is a grandparent, not the
/// immediate supertype.
#[test]
fn transitive_supertype_is_inherited() {
    let (u, idx) = indexed(
        "/D.kt",
        "open class Base\nopen class Middle : Base()\nclass Leaf : Middle()\n",
    );
    assert_eq!(
        receiver_type_agreement(&idx, "Leaf", u.as_str(), "Base"),
        ReceiverTypeAgreement::Inherited
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --bin kmp-lsp hierarchy 2>&1 | tail -15`
Expected: compile errors (types/fns not defined).

- [ ] **Step 3: Implement**

Add to `src/resolver/hierarchy.rs` (needs `use crate::indexer::InferDeps;` added to its imports if not already present — check the top of the file first):

```rust
/// How a candidate receiver's type relates to a target (query) declaring
/// type. `Exact`/`Inherited` are the two ways a candidate is *proven* to
/// belong; `Unrelated` is a proven exclusion; `Unresolvable` means the
/// index doesn't have enough data to prove anything either way (the
/// candidate type itself isn't indexed) — never treat this as `Unrelated`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReceiverTypeAgreement {
    Exact,
    Inherited,
    Unrelated,
    Unresolvable,
}

/// Ascending walk from `candidate_type`: does `target_type` appear among
/// its supertypes? Same mechanism `resolve_from_class_hierarchy` already
/// uses for the string engine's inherited-member lookups, applied in
/// reverse — not "find the member," but "does this ancestor chain contain
/// that type."
pub(crate) fn supertype_chain_contains(
    indexer: &Indexer,
    candidate_type: &str,
    candidate_uri: &str,
    target_type: &str,
) -> bool {
    walk_hierarchy(
        indexer,
        candidate_type,
        candidate_uri,
        CallerContext::default(),
        12,
        |_, super_name, _, _| if super_name == target_type { vec![()] } else { vec![] },
    )
    .into_iter()
    .next()
    .is_some()
}

/// The full receiver-type-agreement decision: exact match (cheap, no walk),
/// else — only if `candidate_type` is genuinely indexed, so a negative
/// result is trustworthy — an ascending supertype walk.
pub(crate) fn receiver_type_agreement(
    indexer: &Indexer,
    candidate_type: &str,
    candidate_uri: &str,
    target_type: &str,
) -> ReceiverTypeAgreement {
    if candidate_type == target_type {
        return ReceiverTypeAgreement::Exact;
    }
    if !indexer.has_type_definition(candidate_type) {
        return ReceiverTypeAgreement::Unresolvable;
    }
    if supertype_chain_contains(indexer, candidate_type, candidate_uri, target_type) {
        ReceiverTypeAgreement::Inherited
    } else {
        ReceiverTypeAgreement::Unrelated
    }
}
```

- [ ] **Step 4: Run and fix until green**

Run: `cargo test --bin kmp-lsp hierarchy 2>&1 | tail -20`
If `walk_hierarchy`'s `collect` closure signature doesn't match (check the real signature in this file first — it's `Fn(&Indexer, &str, &str, CallerContext<'_>) -> Vec<T>`, confirm parameter order against the actual file before assuming the sketch above is exactly right), adjust the closure. Use `to_sexp()`-style debugging (print the walk's visited set) if `subtype_of_target_is_inherited` doesn't pass on the first try — check `derive_supertypes`/how `class DerivedUser : User()` gets indexed as a supertype relationship (this is existing, working machinery reused, not new — the test should pass once wired correctly).

- [ ] **Step 5: Commit**

```bash
git add -A src/
git commit -m "feat(resolver): ReceiverTypeAgreement + supertype_chain_contains"
```

---

### Task 3: `VerifiedReferences` + budgeted per-candidate verification

**Files:**
- Create: `src/features/references_verify.rs`
- Modify: `src/features/mod.rs` (or wherever `references` is declared as a module — check the exact declaration site first) to add `mod references_verify;`
- Test: inline `#[cfg(test)]` module in the new file.

**Interfaces:**
- Consumes: `classify_cursor` (Task 1), `ReceiverTypeAgreement`/`receiver_type_agreement` (Task 2), `ReceiverType::from_raw` (`crate::resolver::ReceiverType`), `NavigationSource<T>` (`crate::indexer::NavigationSource`).
- Produces (Task 4 relies on these exact names):
```rust
pub(crate) struct VerifiedReferences {
    pub kept: Vec<crate::indexer::NavigationSource<Location>>,
    pub rejected: Vec<Location>,
}

pub(crate) fn verify_candidates(
    indexer: &Indexer,
    query_declaring_type: Option<&str>,
    candidates: Vec<Location>,
) -> VerifiedReferences
```

- [ ] **Step 1: Write the failing tests**

```rust
//! Per-candidate CST verification for find-references — see
//! `docs/superpowers/specs/2026-07-20-cst-find-references-design.md`.

use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::indexer::{Indexer, NavigationSource};
use crate::resolver::{receiver_type_agreement, ReceiverType, ReceiverTypeAgreement};

/// Per-request cap on IO-costed verification steps (a candidate's file
/// needing a fresh disk read, or a supertype walk that may spend blocking
/// JAR-sidecar IPC). Once exhausted, remaining candidates stay `NameScan`
/// unverified — never dropped, never rejected on budget grounds alone.
const MAX_VERIFICATION_IO_OPERATIONS: usize = 48;

pub(crate) struct VerifiedReferences {
    pub kept: Vec<NavigationSource<Location>>,
    pub rejected: Vec<Location>,
}

pub(crate) fn verify_candidates(
    indexer: &Indexer,
    query_declaring_type: Option<&str>,
    candidates: Vec<Location>,
) -> VerifiedReferences {
    let Some(query_declaring_type) = query_declaring_type else {
        // No query identity — every candidate is exactly today's behavior.
        return VerifiedReferences {
            kept: candidates.into_iter().map(NavigationSource::NameScan).collect(),
            rejected: Vec::new(),
        };
    };
    let query_declaring_type = ReceiverType::from_raw(query_declaring_type.to_owned()).leaf;

    let mut kept = Vec::new();
    let mut rejected = Vec::new();
    let mut io_budget = MAX_VERIFICATION_IO_OPERATIONS;

    for candidate in candidates {
        if io_budget == 0 {
            kept.push(NavigationSource::NameScan(candidate));
            continue;
        }
        let file_already_indexed = indexer.files.contains_key(candidate.uri.as_str())
            || indexer.live_lines.contains_key(candidate.uri.as_str());
        if !file_already_indexed {
            io_budget -= 1;
        }
        let Some(symbol) = crate::indexer::classify_symbol_at(
            indexer,
            &candidate.uri,
            crate::types::CursorPos {
                line: candidate.range.start.line as usize,
                utf16_col: candidate.range.start.character as usize,
            },
        ) else {
            kept.push(NavigationSource::NameScan(candidate));
            continue;
        };
        match &symbol.role {
            crate::indexer::SymbolRole::Reference {
                receiver_type: Some(receiver_type),
                ..
            } => {
                let candidate_type = ReceiverType::from_raw(receiver_type.clone()).leaf;
                // The supertype walk (Inherited case) may spend sidecar IPC —
                // charge it against the same budget before running it.
                if io_budget == 0 {
                    kept.push(NavigationSource::NameScan(candidate));
                    continue;
                }
                io_budget -= 1;
                match receiver_type_agreement(
                    indexer,
                    &candidate_type,
                    candidate.uri.as_str(),
                    &query_declaring_type,
                ) {
                    ReceiverTypeAgreement::Exact | ReceiverTypeAgreement::Inherited => {
                        kept.push(NavigationSource::CstResolved(candidate));
                    }
                    ReceiverTypeAgreement::Unrelated => rejected.push(candidate),
                    ReceiverTypeAgreement::Unresolvable => {
                        kept.push(NavigationSource::NameScan(candidate));
                    }
                }
            }
            crate::indexer::SymbolRole::Declaration { .. } => {
                // Verified by exact (name, enclosing class) match — see
                // Task 3 Step 3's full implementation; the initial RED test
                // below only exercises the Reference path.
                kept.push(NavigationSource::NameScan(candidate));
            }
            _ => kept.push(NavigationSource::NameScan(candidate)),
        }
    }

    VerifiedReferences { kept, rejected }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file:///t{path}")).unwrap()
    }

    fn location(uri: &Url, line: u32, col_start: u32, col_end: u32) -> Location {
        Location {
            uri: uri.clone(),
            range: Range::new(Position::new(line, col_start), Position::new(line, col_end)),
        }
    }

    /// House decoy: a candidate on `File.save()` must be REJECTED (present
    /// in `rejected`, absent from `kept`) when the query's declaring type
    /// is `User`.
    #[test]
    fn unrelated_candidate_is_rejected_not_dropped_silently() {
        let src = "class User { fun save() {} }\n\
                   class File { fun save() {} }\n\
                   fun f(file: File) { file.save() }\n";
        let u = uri("/D.kt");
        let idx = Indexer::new();
        idx.index_content(&u, src);
        idx.store_live_tree(&u, src);
        let col = src.lines().nth(2).unwrap().find("save").unwrap() as u32;
        let candidate = location(&u, 2, col, col + 4);

        let result = verify_candidates(&idx, Some("User"), vec![candidate.clone()]);
        assert!(result.kept.is_empty(), "must not be kept, got {:?}", result.kept.len());
        assert_eq!(result.rejected, vec![candidate], "must be in rejected, not silently absent");
    }

    /// House decoy, positive: an inherited-member reference through a
    /// subtype instance must be kept as `CstResolved`.
    #[test]
    fn inherited_candidate_is_kept_as_cst_resolved() {
        let src = "open class User { fun save() {} }\n\
                   class DerivedUser : User()\n\
                   fun f(derived: DerivedUser) { derived.save() }\n";
        let u = uri("/D.kt");
        let idx = Indexer::new();
        idx.index_content(&u, src);
        idx.store_live_tree(&u, src);
        let col = src.lines().nth(2).unwrap().find("save").unwrap() as u32;
        let candidate = location(&u, 2, col, col + 4);

        let result = verify_candidates(&idx, Some("User"), vec![candidate.clone()]);
        assert!(result.rejected.is_empty());
        assert!(matches!(
            result.kept.as_slice(),
            [NavigationSource::CstResolved(loc)] if *loc == candidate
        ));
    }

    #[test]
    fn no_query_identity_passes_every_candidate_through_as_name_scan() {
        let u = uri("/D.kt");
        let idx = Indexer::new();
        let candidate = location(&u, 0, 0, 4);
        let result = verify_candidates(&idx, None, vec![candidate.clone()]);
        assert!(result.rejected.is_empty());
        assert!(matches!(
            result.kept.as_slice(),
            [NavigationSource::NameScan(loc)] if *loc == candidate
        ));
    }

    /// Budget decoy: once the IO budget is exhausted, remaining candidates
    /// stay in `kept` as `NameScan` — never moved to `rejected`, even when
    /// they WOULD have been proven unrelated with more budget.
    #[test]
    fn budget_exhaustion_never_rejects_only_skips_verification() {
        let src = "class User { fun save() {} }\nclass File { fun save() {} }\n";
        let u = uri("/D.kt");
        let idx = Indexer::new();
        idx.index_content(&u, src);
        idx.store_live_tree(&u, src);
        // Many candidates on unindexed files so every one costs a disk-read
        // budget unit; with MAX_VERIFICATION_IO_OPERATIONS candidates all
        // needing 2 units each (disk read + agreement check), the tail
        // exhausts the budget.
        let candidates: Vec<Location> = (0..(MAX_VERIFICATION_IO_OPERATIONS as u32 + 5))
            .map(|line| location(&u, line, 0, 4))
            .collect();
        let result = verify_candidates(&idx, Some("User"), candidates.clone());
        assert!(
            result.rejected.len() < candidates.len(),
            "budget exhaustion must leave some candidates unverified, not reject them all"
        );
        assert_eq!(
            result.kept.len() + result.rejected.len(),
            candidates.len(),
            "no candidate may vanish — every one is either kept or rejected"
        );
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --bin kmp-lsp references_verify 2>&1 | tail -20`
Expected: compile errors first (module not registered / imports not resolving), then real assertion failures once it compiles.

- [ ] **Step 3: Register the module and implement the Declaration-role verification branch**

Add `mod references_verify;` to wherever `mod references;` is declared (grep `mod references;` in `src/features.rs` or `src/features/mod.rs` first to find the exact spot, and match its visibility — likely `pub(crate) mod references_verify;` alongside the sibling).

Replace the `SymbolRole::Declaration` arm's placeholder from Step 1 with the real check: a declaration-role candidate is kept as `CstResolved` only when its own enclosing class matches the query's declaring type (both already normalized), otherwise it's a different declaration and is treated as `Unresolvable` (kept, not rejected — this is a *weaker* signal than a proven type mismatch, since two same-named unrelated declarations aren't the "wrong receiver type" case `ReceiverTypeAgreement` models; err toward keeping, per the "only proven Unrelated rejects" global constraint):

```rust
            crate::indexer::SymbolRole::Declaration { .. } => {
                let enclosing_class = indexer.enclosing_class_at(&candidate.uri, candidate.range.start.line);
                let matches_query = enclosing_class
                    .as_deref()
                    .map(|class_name| ReceiverType::from_raw(class_name.to_owned()).leaf)
                    == Some(query_declaring_type.clone());
                if matches_query {
                    kept.push(NavigationSource::CstResolved(candidate));
                } else {
                    kept.push(NavigationSource::NameScan(candidate));
                }
            }
```

(`indexer.enclosing_class_at` is already used elsewhere in this codebase — `src/features/definition.rs` — confirm the exact trait/inherent-method path compiles; it's available directly on `&Indexer`.)

- [ ] **Step 4: Run and fix until green**

Run: `cargo test --bin kmp-lsp references_verify 2>&1 | tail -25`

- [ ] **Step 5: Commit**

```bash
git add -A src/
git commit -m "feat(references): VerifiedReferences + budgeted per-candidate verification"
```

---

### Task 4: Wire into `find_references_with_qualifier`

**Files:**
- Modify: `src/features/references.rs` (`find_references_with_qualifier`)
- Modify: `src/backend/handlers.rs` (the one production call site, ~line 55)
- Modify: `src/features/references_tests.rs` (27 call sites — mechanical signature update)

**Interfaces:**
- Consumes: `classify_cursor` (Task 1), `verify_candidates`/`VerifiedReferences` (Task 3).
- Produces: `find_references_with_qualifier`'s signature changes from `line: u32` to `position: Position` (the 4th parameter) — everywhere else unchanged.

- [ ] **Step 1: Change the signature and thread `position` through**

In `src/features/references.rs`, change:

```rust
pub(crate) async fn find_references_with_qualifier(
    name: &str,
    qualifier: Option<&str>,
    uri: &Url,
    line: u32,
    include_decl: bool,
    index: &(impl SymbolIndex + DocumentAccess + ScopeQuery + SearchAccess + Send + Sync),
) -> Vec<Location> {
```

to:

```rust
pub(crate) async fn find_references_with_qualifier(
    name: &str,
    qualifier: Option<&str>,
    uri: &Url,
    position: Position,
    include_decl: bool,
    index: &(impl SymbolIndex + DocumentAccess + ScopeQuery + SearchAccess + Send + Sync),
) -> Vec<Location> {
    let line = position.line;
```

as the function's first line (everything below that already uses `line` keeps compiling unchanged — pure rename-and-derive, verify by reading the rest of the function body for every existing `line` use before assuming nothing else needs touching).

At the very end of the function, after the existing `add_current_file_locations` call (which currently returns `locations` directly), insert the verification pass. Read the function's current tail (the last ~10 lines, after `let mut locations = rg_locations(...)`) to see the exact variable name and structure before editing — the sketch below assumes the final local is named `locations: Vec<Location>`:

```rust
    let query_declaring_type = match crate::indexer::classify_cursor(index_as_indexer, uri, position) {
        // Note: `index` here is the generic trait-bounded parameter — this
        // function needs a concrete `&Indexer` for classify_cursor. Check
        // whether `index` in THIS function is already `&Indexer` (it may be,
        // since `find_references_with_qualifier`'s only production caller
        // passes `&*self.indexer`) or whether the generic bound needs the
        // same simplification Tasks 4/5/6 of slice 6a already applied to
        // definition.rs/implementation.rs/highlight.rs — grep
        // `impl SymbolIndex for` / `impl DocumentAccess for` /
        // `impl ScopeQuery for` / `impl SearchAccess for` across src/ first;
        // if Indexer is still the only implementor (it was for the other
        // three traits as of 6a), simplify this function's signature to a
        // concrete `index: &Indexer` parameter instead of threading a
        // second parameter — same precedent, same reasoning.
        Some(symbol) => match &symbol.role {
            crate::indexer::SymbolRole::Declaration { .. } => index_as_indexer.enclosing_class_at(uri, line),
            crate::indexer::SymbolRole::Reference { receiver_type: Some(receiver_type), .. } => {
                Some(receiver_type.clone())
            }
            _ => None,
        },
        None => None,
    };

    let verified = crate::features::references_verify::verify_candidates(
        index_as_indexer,
        query_declaring_type.as_deref(),
        locations,
    );
    let mut resolved_first: Vec<Location> = Vec::with_capacity(verified.kept.len());
    let mut name_scanned: Vec<Location> = Vec::new();
    for source in verified.kept {
        match source {
            crate::indexer::NavigationSource::CstResolved(location) => resolved_first.push(location),
            crate::indexer::NavigationSource::NameScan(location) => name_scanned.push(location),
        }
    }
    resolved_first.append(&mut name_scanned);
    resolved_first
```

Resolve the `index_as_indexer` placeholder per the comment above during implementation: either the function already has concrete `&Indexer` access, or its signature needs simplifying — investigate and document the decision in the task report, matching how Tasks 4/5/6 of slice 6a each investigated and recorded this same question.

- [ ] **Step 2: Update the backend call site**

In `src/backend/handlers.rs` (~line 55), change:

```rust
        let locations = crate::features::references::find_references_with_qualifier(
            &ctx.word,
            ctx.qualifier.as_deref(),
            uri,
            position.line,
            params.context.include_declaration,
            &*self.indexer,
        )
        .await;
```

to pass `position` instead of `position.line`:

```rust
        let locations = crate::features::references::find_references_with_qualifier(
            &ctx.word,
            ctx.qualifier.as_deref(),
            uri,
            position,
            params.context.include_declaration,
            &*self.indexer,
        )
        .await;
```

- [ ] **Step 3: Mechanically update the 27 test call sites**

Run: `grep -n "find_references_with_qualifier(" src/features/references_tests.rs` to get the exact list. For each call site, the argument that was `line_number` (a bare `u32` literal or variable, e.g. `1`, `3`, `4`) becomes `Position::new(line_number, 0)` — column 0 is a safe, behavior-preserving default: these tests predate query-identity classification, and `classify_cursor` at column 0 on a line written for a DIFFERENT purpose (locating an rg scope, not landing precisely on the queried identifier) will typically classify nothing useful, so `query_declaring_type` comes back `None` and verification passes every candidate through unchanged — exactly what these tests currently assert. Add `use tower_lsp::lsp_types::Position;` to `references_tests.rs`'s imports if not already present (it likely already imports `Position` given the file uses `Location`/`Range` — check first).

This is mechanical (find each `, <number>,` 4th-argument slot, wrap it) — dispatch to a subagent with these exact instructions rather than hand-editing 27 sites, or use Serena's `replace_content` for a systematic regex-driven pass per the project's own tooling preference (see `AGENTS.md`).

- [ ] **Step 4: Run the full suite**

Run: `cargo test --bin kmp-lsp 2>&1 | grep -E "^test result|FAILED"`
Expected: identical pass count to before this task — the 27 existing tests assert on recall (candidate presence), which is untouched; only labeling/filtering changed, and none of these fixtures have a query-identity-classifiable cursor precise enough to trigger a rejection.

- [ ] **Step 5: Commit**

```bash
git add -A src/
git commit -m "feat(references): wire CST verification into find_references_with_qualifier"
```

---

### Task 5: End-to-end house decoys at the `find_references_with_qualifier` level

**Files:**
- Modify: `src/features/references_tests.rs`

**Interfaces:** none new — pure integration tests over Task 4's wiring.

- [ ] **Step 1: Write the tests**

```rust
/// End-to-end house decoy: find-references on `User.save` (invoked from a
/// call site) must exclude `File.save()`'s call site entirely from the
/// returned Vec<Location> — the actual precision proof at the public API
/// boundary, not just verify_candidates' internal VerifiedReferences.
#[tokio::test]
async fn find_references_excludes_unrelated_same_named_member() {
    let idx = Indexer::new();
    let user_uri = Url::parse("file:///t/User.kt").unwrap();
    let file_uri = Url::parse("file:///t/File.kt").unwrap();
    let caller_uri = Url::parse("file:///t/Caller.kt").unwrap();
    idx.index_content(&user_uri, "class User { fun save() {} }\n");
    idx.index_content(&file_uri, "class File { fun save() {} }\n");
    let caller_src = "fun f(user: User, file: File) {\n    user.save()\n    file.save()\n}\n";
    idx.index_content(&caller_uri, caller_src);
    idx.store_live_tree(&caller_uri, caller_src);
    let col = caller_src.lines().nth(1).unwrap().find("save").unwrap() as u32;

    let locations = find_references_with_qualifier(
        "save",
        None,
        &caller_uri,
        Position::new(1, col),
        false,
        &idx,
    )
    .await;

    assert!(
        locations.iter().all(|location| location.uri != file_uri || location.range.start.line != 2),
        "File.save() call site must not appear; got: {:?}",
        locations
    );
}
```

- [ ] **Step 2: Run to verify failure or success**

Run: `cargo test --bin kmp-lsp find_references_excludes_unrelated 2>&1 | tail -15`
This is RED-first in spirit but may already pass depending on whether rg's own scope narrowing happens to separate `User.kt` and `File.kt` for this fixture (both are separate files with no shared package/import linking them to `Caller.kt`'s bare-word `save` query) — if it passes immediately, that's fine (it still pins the end-to-end behavior); if it's genuinely RED, that confirms the wiring is load-bearing for this exact scenario. Record which in the task report either way — do not weaken the assertion to force a RED result artificially.

- [ ] **Step 3: Commit**

```bash
git add -A src/
git commit -m "test(references): end-to-end house decoy for unrelated-member exclusion"
```

---

### Task 6: Gates, live probe, PR

- [ ] **Step 1:** `cargo test 2>&1 | tail -5 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -2 && cargo clippy --release --all-targets -- -D warnings 2>&1 | tail -2 && cargo test --test lsp_smoke 2>&1 | tail -3`

- [ ] **Step 2:** Build (`cargo build`) and live-probe on Moneta (adapt `scratchpad/lsp_probe_nav6a.py`'s harness): find-references on a same-named member across two unrelated real classes in the project, and on an inherited member accessed through a subtype instance. BIN must point at the worktree `target/debug/kmp-lsp`.

- [ ] **Step 3:** Push, open PR → `refactor/unified-resolution`. PR body: what shipped (verification layer + the classify_cursor extraction that came out of the pre-implementation critique), the typed rejection trail, the IO budget rationale, decoy results, probe results.

- [ ] **Step 4:** Ledger entry + memory update. Note: 6c (rename) is next, gated on 6c's own live-probe measurement of cross-file refusal rate per the spec's flagged policy question (F5 in the original slice-6 critique).

## Self-review notes

- Spec coverage: "Shared prologue" = Task 1; "Type normalization" + "Verification outcome" = Task 2; "Per-candidate verification" + "Result type" = Task 3; "Query identity" + wiring = Task 4; "Testing" section's house/budget decoys = Tasks 3 (unit-level) and 5 (end-to-end); live probe = Task 6.
- Type consistency: `ReceiverTypeAgreement`/`supertype_chain_contains`/`receiver_type_agreement` (Task 2) used identically in Task 3; `VerifiedReferences`/`verify_candidates` (Task 3) used identically in Task 4; `classify_cursor` (Task 1) used in Tasks 1, 3 (indirectly via `classify_symbol_at` for per-candidate, which intentionally does NOT go through `classify_cursor` since candidates start from `Location`/`Range`, not an LSP `Position` — this is correct, not an inconsistency: `classify_cursor` exists specifically for the `Position`-shaped call sites).
- Known judgment points flagged for the implementer: Task 4 Step 1's generic-bound-vs-concrete-`Indexer` question for `find_references_with_qualifier` (same investigate-and-decide pattern as slice 6a Tasks 4-6); Task 4's CstResolved-first ordering implementation is sketched loosely on purpose — any clear implementation satisfying "resolved first, scanned after" is acceptable, avoid abbreviated names in whatever shape is chosen; Task 5's RED-vs-already-green uncertainty is called out explicitly rather than papered over.
