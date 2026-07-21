# 6b-Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three real correctness gaps in shipped 6b find-references code (a Declaration-arm agreement bug, an IO-budget over-charge, and a hardcoded interactive-latency cap on hierarchy walks) so that 6c (rename) has a reliable override-detection signal to build on.

**Architecture:** Three independent, sequential fixes to `src/resolver/hierarchy.rs` and `src/features/references_verify.rs`, plus a naming cleanup on two still-unresolved PR #228 review threads. No new features — every fix is behavior-preserving for existing callers except where the spec explicitly calls out the precision improvement (Declaration-arm agreement, IO-budget accuracy).

**Tech Stack:** Rust, tree-sitter-backed CST walk, existing `Indexer`/`resolver` module structure.

## Global Constraints

- Design source of truth: `docs/superpowers/specs/2026-07-19-cst-navigation-design.md`, section "6b-hardening — prerequisite fixes before 6c". Do not re-derive reasoning already settled there — cite it, don't repeat it, in code comments only where the *why* is non-obvious.
- No abbreviated identifiers (AGENTS.md project rule — explicit ban list includes `u`, `idx`, `src`, `col`, `ty`, `sym`, `loc`). Use full words: `file_uri`, `indexer`, `source`, `column`.
- Every existing test must keep passing; this plan adds new tests, it does not remove coverage.
- `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` (the existing cap, value `3`) must remain the value every *existing* caller passes — this plan changes *who controls* the cap, not its default.
- Run `cargo test` (workspace-wide) at the end of every task, not just the touched module — signature changes ripple across `src/resolver/*.rs`.

---

### Task 1: Parameterize `walk_hierarchy`'s sidecar-promotion budget; put `receiver_type_agreement` on the `Resolver` catalogue

**Catalogue note:** `src/resolver/api.rs` is this codebase's resolution capability catalogue —
its own doc comment states the contract explicitly: "a consumer that needs to resolve something
looks here first... If it is missing, add a method to `Resolver` (and implement it by delegating
to the canonical function), so the next consumer finds it instead of reinventing it." Today
`receiver_type_agreement` is consumed as a raw free-function import
(`use crate::resolver::{receiver_type_agreement, ...}` in `references_verify.rs`) — bypassing the
catalogue entirely. This task is already changing that function's signature everywhere it's
called; do it right by also adding it to `Resolver` as the intent-named capability it actually
is ("does this candidate's type relate to that target type"), matching exactly how
`resolve_member` wraps `resolve_member_only` in the same trait. `walk_hierarchy` itself stays a
free function — it's a generic traversal primitive several *different* resolver submodules
(`complete.rs`, `infer.rs`, `resolve.rs`) use for unrelated purposes (ancestor-name collection,
inherited-member collection), not itself an intent-named "resolve X" capability the catalogue's
contract is about. Do not attempt a broader cleanup of `resolver/mod.rs`'s other loose
re-exports in this task — that's a separate, larger effort tracked elsewhere; scope this to what
this task is already touching.

**Files:**
- Modify: `src/resolver/hierarchy.rs:8-22` (doc comment), `:26-48` (`walk_hierarchy`), `:173-196` (`supertype_chain_contains`), `:201-218` (`receiver_type_agreement`)
- Modify: `src/resolver/api.rs` (add `receiver_type_agreement` to the `Resolver` trait + its `impl Resolver for Indexer`)
- Modify: `src/resolver/mod.rs:19,23` (re-exports — `ReceiverTypeAgreement` moves up next to `Resolver`; the free-function `receiver_type_agreement` is removed from the re-export list, since external consumers now go through the trait)
- Modify: `src/resolver/resolve.rs:1089-1095` (call site)
- Modify: `src/resolver/complete.rs:18-21` (import), `:291-298`, `:566-573`, `:791-798`, `:1777-1785`, `:1842-1849` (5 call sites)
- Modify: `src/resolver/infer.rs:581`, `:612-619` (local import + call site)
- Modify: `src/resolver/tests.rs:4292-4298` (call site)
- Modify: `src/resolver/hierarchy_tests.rs` (5 call sites — full rewrite alongside Task 4's naming cleanup; touched again there. These test the free function directly, same pattern as `resolve_member_only`'s own direct tests elsewhere — the trait method is a thin, untested-in-isolation wrapper, consistent with how the rest of `Resolver`'s methods are covered by testing the function they delegate to.)
- Modify: `src/features/references_verify.rs:7`, `:80-85` (import + call site — Reference arm, now via the trait)
- Test: `src/resolver/hierarchy_tests.rs` (existing 5 tests, updated to pass the new parameter — behavior must not change)

**Interfaces:**
- Produces: `walk_hierarchy<'a, T, F>(idx, start_class, start_uri, caller, max_depth, sidecar_budget: usize, collect) -> Vec<T>` — `sidecar_budget` is a new 6th positional parameter, inserted immediately before the `collect` closure. Stays a free function (see the catalogue note above).
- Produces: `supertype_chain_contains(indexer, candidate_type, candidate_uri, target_type, sidecar_budget: usize) -> bool` — new 5th parameter, appended. Stays a free function — an internal implementation detail `receiver_type_agreement` uses, not itself catalogued.
- Produces: `Resolver::receiver_type_agreement(&self, candidate_type, candidate_uri, target_type, sidecar_budget: usize) -> ReceiverTypeAgreement` — now a catalogue trait method (`use crate::resolver::Resolver;` to call it as `indexer.receiver_type_agreement(...)`), delegating to `hierarchy::receiver_type_agreement`. Task 2/3 (this plan) and 6c's override-detection (next plan) both call this via the trait with an explicit budget.
- Consumes: nothing new — this task only changes signatures of existing functions and adds one catalogue trait method.

This is a signature-only, behavior-preserving change: every call site below passes `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` explicitly, reproducing today's cap exactly. No test assertions change in this task — only call sites and the newly-catalogued entry point.

- [ ] **Step 1: Update `walk_hierarchy` and its doc comment**

In `src/resolver/hierarchy.rs`, replace the doc comment above the constant and the function itself:

```rust
/// Per-WALK cap on blocking sidecar-IPC promotion attempts for ancestor
/// classes living in not-yet-materialized JARs. The walk runs on paths that
/// carry no promotion budget of their own — per-name inference (inlay hints
/// fan `resolve_from_class_hierarchy` out across every visible name) and
/// bare completion's inherited-members collector — so an unbudgeted
/// promotion here bypassed every existing request cap: with a cold cache,
/// each distinct un-cached ancestor JAR paid a ~200ms blocking round trip,
/// unbounded across a walk. Cache-backed promotions bypass this (free, pure
/// in-memory); genuinely cold ancestors beyond the cap stay Tier-1 for this
/// walk and are covered by file-open import promotion or a later walk —
/// `materialized`/`materialization_failed` memoize outcomes, so per session
/// each JAR pays at most one attempt. Per-REQUEST budget threading (one
/// budget shared across all the walks a request triggers) is deferred to
/// the accessor-function refactor.
///
/// This is the *default* every interactive, keystroke-latency-sensitive
/// caller should pass. `walk_hierarchy` takes the budget as a parameter,
/// not a hardcoded internal — a caller with a different latency tolerance
/// (e.g. a user-initiated rename, not a per-keystroke completion) may pass
/// a larger budget so its walk can run to actual completion instead of
/// guessing under a cap sized for a different use case.
pub(crate) const MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK: usize = 3;

/// Walk the class hierarchy starting from `start_class`, collecting items at each level.
/// `T` is what the visitor produces per symbol. `max_depth` prevents infinite loops.
/// `sidecar_budget` bounds blocking JAR-promotion round trips for this walk — pass
/// [`MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK`] for interactive callers, or a
/// caller-specific budget for operations that can tolerate more latency.
pub(crate) fn walk_hierarchy<'a, T, F>(
    idx: &'a Indexer,
    start_class: &str,
    start_uri: &str,
    caller: CallerContext<'a>,
    max_depth: usize,
    sidecar_budget: usize,
    collect: F,
) -> Vec<T>
where
    F: Fn(&Indexer, &str, &str, CallerContext<'_>) -> Vec<T>,
{
    let mut walker = HierarchyWalker {
        idx,
        caller,
        max_depth,
        collect,
        visited: HashSet::from([(start_uri.to_owned(), start_class.to_owned())]),
        items: Vec::new(),
        sidecar_budget,
    };
    walker.recurse(start_class, start_uri, 0);
    walker.items
}
```

- [ ] **Step 2: Update `supertype_chain_contains` and `receiver_type_agreement`**

Still in `src/resolver/hierarchy.rs`:

```rust
/// Ascending walk from `candidate_type`: does `target_type` appear among
/// its supertypes? Same mechanism `resolve_from_class_hierarchy` already
/// uses for the string engine's inherited-member lookups, applied in
/// reverse — not "find the member," but "does this ancestor chain contain
/// that type." `sidecar_budget` bounds blocking JAR-promotion round trips —
/// see [`walk_hierarchy`].
pub(crate) fn supertype_chain_contains(
    indexer: &Indexer,
    candidate_type: &str,
    candidate_uri: &str,
    target_type: &str,
    sidecar_budget: usize,
) -> bool {
    walk_hierarchy(
        indexer,
        candidate_type,
        candidate_uri,
        CallerContext::default(),
        12,
        sidecar_budget,
        |_, super_name, _, _| {
            if super_name == target_type {
                vec![()]
            } else {
                vec![]
            }
        },
    )
    .into_iter()
    .next()
    .is_some()
}

/// The full receiver-type-agreement decision: exact match (cheap, no walk),
/// else — only if `candidate_type` is genuinely indexed, so a negative
/// result is trustworthy — an ascending supertype walk. `sidecar_budget`
/// bounds blocking JAR-promotion round trips the walk may spend — see
/// [`walk_hierarchy`].
pub(crate) fn receiver_type_agreement(
    indexer: &Indexer,
    candidate_type: &str,
    candidate_uri: &str,
    target_type: &str,
    sidecar_budget: usize,
) -> ReceiverTypeAgreement {
    if candidate_type == target_type {
        return ReceiverTypeAgreement::Exact;
    }
    if !indexer.has_type_definition(candidate_type) {
        return ReceiverTypeAgreement::Unresolvable;
    }
    if supertype_chain_contains(indexer, candidate_type, candidate_uri, target_type, sidecar_budget) {
        ReceiverTypeAgreement::Inherited
    } else {
        ReceiverTypeAgreement::Unrelated
    }
}
```

- [ ] **Step 3: Add `receiver_type_agreement` to the `Resolver` catalogue, and update `src/resolver/mod.rs`'s re-exports**

In `src/resolver/api.rs`, add to the `Resolver` trait (after `method_return_type`'s signature, before the trait's closing `}`):

```rust
    /// Does `candidate_type`'s receiver relate to `target_type` — exact
    /// match, a proven supertype/subtype relationship (in either direction a
    /// caller separately checks), a proven exclusion, or "the index can't
    /// prove anything either way"? `candidate_uri` is where `candidate_type`
    /// is declared (needed to walk its supertype chain). `sidecar_budget`
    /// bounds blocking JAR-promotion round trips the walk may spend — pass
    /// [`crate::resolver::MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK`] for
    /// interactive callers, or a larger budget for latency-tolerant ones.
    fn receiver_type_agreement(
        &self,
        candidate_type: &str,
        candidate_uri: &str,
        target_type: &str,
        sidecar_budget: usize,
    ) -> ReceiverTypeAgreement;
```

And to `impl Resolver for Indexer` (after `method_return_type`'s implementation, before the impl's closing `}`):

```rust
    fn receiver_type_agreement(
        &self,
        candidate_type: &str,
        candidate_uri: &str,
        target_type: &str,
        sidecar_budget: usize,
    ) -> ReceiverTypeAgreement {
        super::hierarchy::receiver_type_agreement(
            self,
            candidate_type,
            candidate_uri,
            target_type,
            sidecar_budget,
        )
    }
```

Add the import at the top of `src/resolver/api.rs`:

```rust
use super::ReceiverTypeAgreement;
```

In `src/resolver/mod.rs`, move `ReceiverTypeAgreement` up next to the trait it now belongs with, and drop the free-function `receiver_type_agreement` from the loose re-export list (its only external consumer, `references_verify.rs`, now calls it via the trait — Step 6 below):

Line 19, before:
```rust
pub(crate) use api::{Resolver, ReturnType};
```
after:
```rust
pub(crate) use api::{Resolver, ReturnType};
pub(crate) use hierarchy::ReceiverTypeAgreement;
```
Line 23, before:
```rust
pub(crate) use hierarchy::{receiver_type_agreement, walk_hierarchy, ReceiverTypeAgreement};
```
after:
```rust
pub(crate) use hierarchy::{walk_hierarchy, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK};
```

- [ ] **Step 4: Update every existing `walk_hierarchy` call site to pass the constant explicitly**

`src/resolver/resolve.rs:1089` — before:
```rust
    let results = walk_hierarchy(
        indexer,
        "",
        from_uri.as_str(),
        CallerContext::default(),
        12,
        |index, _, class_uri, _| find_name_in_uri(index, name, class_uri),
    );
```
after:
```rust
    let results = walk_hierarchy(
        indexer,
        "",
        from_uri.as_str(),
        CallerContext::default(),
        12,
        MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
        |index, _, class_uri, _| find_name_in_uri(index, name, class_uri),
    );
```
Add `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` to the existing `use super::hierarchy::walk_hierarchy;` import at line 36:
```rust
use super::hierarchy::{walk_hierarchy, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK};
```

`src/resolver/complete.rs` — add the constant to the existing `use super::{ ... };` block (line 18):
```rust
use super::{
    already_imported, ensure_file_data, fqns_for_name, resolve_symbol_no_rg, walk_hierarchy,
    MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK, Resolver,
};
```
Then at each of its 5 `walk_hierarchy(...)` calls (lines 291, 566, 791, 1777, 1842), insert `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,` as a new argument immediately after the existing `max_depth` argument (`8`, `4`, `4`, `4`, `8` respectively) and before the closure. Example for line 291:
```rust
        let supers = walk_hierarchy(
            indexer,
            receiver_type,
            &class_uri,
            caller,
            8,
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
            |_idx, super_name, _super_uri, _caller| vec![super_name.to_owned()],
        );
```
Apply the identical one-line insertion (constant before the closure, after the depth argument) at lines 566, 791, 1777, and 1842.

`src/resolver/infer.rs:581` — before:
```rust
    use super::walk_hierarchy;
```
after:
```rust
    use super::{walk_hierarchy, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK};
```
Then at line 612, insert the constant after `caller,` and before the closure (depth argument here is `8`):
```rust
        let supers: Vec<String> = walk_hierarchy(
            indexer,
            class_name,
            class_uri,
            caller,
            8,
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
            |_idx, super_name, _super_uri, _caller| vec![super_name.to_owned()],
        );
```

`src/resolver/tests.rs:4292` — this file already has `use super::*;` (line 1), so `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` is already in scope once Step 3 re-exports it. Update the call:
```rust
        let _ = crate::resolver::walk_hierarchy(
            &idx,
            "Base0",
            "file:///sdk/Base0.kt",
            crate::types::CallerContext::default(),
            CHAIN_LENGTH,
            crate::resolver::MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
            |_, _, _, _| Vec::<()>::new(),
        );
```

- [ ] **Step 5: Update `hierarchy_tests.rs`'s 5 call sites (behavior-preserving; naming cleanup happens in Task 4)**

In `src/resolver/hierarchy_tests.rs`, add the constant to the import at line 1:
```rust
use super::{
    receiver_type_agreement, supertype_chain_contains, ReceiverTypeAgreement,
    MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
};
```
Then append `, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` as the new last argument to every `receiver_type_agreement(...)` call (lines 20, 35, 45, 56, 70) and every `supertype_chain_contains(...)` call (lines 28, 43). Example (line 20):
```rust
        receiver_type_agreement(&idx, "User", u.as_str(), "User", MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK),
```
Apply the same trailing-argument insertion to the other 6 call sites listed above.

- [ ] **Step 6: Update `references_verify.rs`'s call site to go through the `Resolver` catalogue**

In `src/features/references_verify.rs`, the `receiver_type_agreement` call (around line 80) — before:
```rust
                match indexer.receiver_type_agreement(
                    &candidate_type,
                    candidate.uri.as_str(),
                    &query_declaring_type,
                ) {
```
after (method-call syntax via the `Resolver` trait, not the free function):
```rust
                match indexer.receiver_type_agreement(
                    &candidate_type,
                    candidate.uri.as_str(),
                    &query_declaring_type,
                    MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
                ) {
```
Replace the existing import at the top of the file — before:
```rust
use crate::resolver::{receiver_type_agreement, ReceiverType, ReceiverTypeAgreement};
```
after:
```rust
use crate::resolver::{
    ReceiverType, ReceiverTypeAgreement, Resolver, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
};
```
`Resolver` must be in scope for `indexer.receiver_type_agreement(...)`'s method-call syntax to resolve — this is the same import every other `Resolver`-catalogue consumer in the codebase already uses (search `grep -rn "use crate::resolver::.*Resolver" src/` for the established pattern before writing this step's diff, to match existing style exactly).

- [ ] **Step 7: Build and run the full test suite**

Run: `cargo build 2>&1 | tail -30`
Expected: clean build, no errors (this step is purely mechanical signature threading — any compile error means a call site was missed).

Run: `cargo test --lib 2>&1 | tail -40`
Expected: every existing test passes, same pass count as before this task (no behavior change).

- [ ] **Step 8: Commit**

```bash
git add src/resolver/hierarchy.rs src/resolver/mod.rs src/resolver/resolve.rs \
        src/resolver/complete.rs src/resolver/infer.rs src/resolver/tests.rs \
        src/resolver/hierarchy_tests.rs src/features/references_verify.rs
git commit -m "refactor(resolver): parameterize walk_hierarchy's sidecar-promotion budget

Every existing caller passes MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
explicitly, reproducing today's cap exactly. Lets a future caller with a
different latency tolerance (6c rename's override-detection walk) pass
its own budget instead of guessing under a cap sized for interactive,
keystroke-latency callers."
```

---

### Task 2: Declaration-arm agreement fix

**Files:**
- Modify: `src/features/references_verify.rs:95-112` (the `SymbolRole::Declaration` match arm in `verify_candidates`)
- Test: `src/features/references_verify.rs` (new test in the existing `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: `Resolver::receiver_type_agreement(&self, candidate_type, candidate_uri, target_type, sidecar_budget) -> ReceiverTypeAgreement` (Task 1's catalogue method — call as `indexer.receiver_type_agreement(...)`).
- Produces: no new public interface — this task changes `verify_candidates`'s internal classification only. Its externally-visible effect: an override's own declaration now classifies `CstResolved` (via `Inherited`) rather than `NameScan`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/features/references_verify.rs` (after the existing `inherited_candidate_is_kept_as_cst_resolved` test):

```rust
    /// The Declaration-arm bug this task fixes: an override's OWN declaration
    /// must classify the same way a reference *through* the subtype does
    /// (`Inherited` -> `CstResolved`), not fall to `NameScan` just because its
    /// enclosing class name isn't a byte-for-byte match against the query type.
    #[test]
    fn override_declaration_is_kept_as_cst_resolved_not_name_scan() {
        let source = "open class User { fun save() {} }\n\
                      class DerivedUser : User() { override fun save() {} }\n";
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);
        let column = source.lines().nth(1).unwrap().find("save").unwrap() as u32;
        let candidate = location(&file_uri, 1, column, column + 4);

        let result = verify_candidates(&indexer, Some("User"), vec![candidate.clone()]);
        assert!(result.rejected.is_empty());
        assert!(
            matches!(
                result.kept.as_slice(),
                [NavigationSource::CstResolved(location)] if *location == candidate
            ),
            "override's own declaration must be CstResolved, got {:?}",
            result.kept
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib override_declaration_is_kept_as_cst_resolved -- --nocapture`
Expected: FAIL — the candidate lands in `NameScan`, not `CstResolved` (the exact-string match `"DerivedUser" != "User"` fails today).

- [ ] **Step 3: Fix the Declaration arm**

In `src/features/references_verify.rs`, replace the `SymbolRole::Declaration` arm:

```rust
            crate::indexer::SymbolRole::Declaration { .. } => {
                let enclosing_class =
                    indexer.enclosing_class_at(&candidate.uri, candidate.range.start.line);
                match enclosing_class {
                    Some(class_name) => {
                        let candidate_type = ReceiverType::from_raw(class_name).leaf;
                        match indexer.receiver_type_agreement(
                            &candidate_type,
                            candidate.uri.as_str(),
                            &query_declaring_type,
                            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
                        ) {
                            ReceiverTypeAgreement::Exact | ReceiverTypeAgreement::Inherited => {
                                kept.push(NavigationSource::CstResolved(candidate));
                            }
                            // A mismatch here is a *weaker* signal than a proven type
                            // mismatch — two same-named unrelated declarations aren't
                            // the "wrong receiver type" case `ReceiverTypeAgreement`
                            // models — so err toward keeping (`NameScan`), never
                            // reject here, same as before this fix.
                            ReceiverTypeAgreement::Unrelated
                            | ReceiverTypeAgreement::Unresolvable => {
                                kept.push(NavigationSource::NameScan(candidate));
                            }
                        }
                    }
                    None => kept.push(NavigationSource::NameScan(candidate)),
                }
            }
```

This replaces the exact-string-equality comparison (`ReceiverType::from_raw(class_name).leaf == Some(query_declaring_type.clone())`) with the same `receiver_type_agreement` call the `Reference` arm above it already uses — `Exact` for the query's own declaration, `Inherited` for a proven override, `Unrelated`/`Unresolvable` both fall to `NameScan` unchanged (no new rejections — a same-named unrelated declaration is a weaker signal than a proven type mismatch, per the existing comment this fix preserves).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib override_declaration_is_kept_as_cst_resolved -- --nocapture`
Expected: PASS

- [ ] **Step 5: Run the full references_verify test module to confirm no regression**

Run: `cargo test --lib references_verify:: -- --nocapture`
Expected: all tests pass, including the pre-existing `unrelated_candidate_is_rejected_not_dropped_silently`, `inherited_candidate_is_kept_as_cst_resolved`, `no_query_identity_passes_every_candidate_through_as_name_scan`, and `budget_exhaustion_never_rejects_only_skips_verification`.

- [ ] **Step 6: Commit**

```bash
git add src/features/references_verify.rs
git commit -m "fix(references): Declaration-arm agreement uses receiver_type_agreement

The Declaration arm of verify_candidates compared the candidate's
enclosing class against the query's declaring type by exact string
equality, unlike the Reference arm just above it which uses
receiver_type_agreement's supertype walk. This put every override's OWN
declaration into NameScan -- indistinguishable from 'could not verify' --
because e.g. \"DerivedUser\" != \"User\" as strings, even though
DerivedUser IS User's subtype. Now both arms apply the same standard:
Exact/Inherited -> CstResolved, Unrelated/Unresolvable -> NameScan
unchanged (no new rejections)."
```

---

### Task 3: IO-budget charged only when a walk will actually run

**Files:**
- Modify: `src/features/references_verify.rs` (the budget-charge sites in both the `Reference` and `Declaration` arms of `verify_candidates`)
- Test: `src/features/references_verify.rs` (new test in the existing test module)

**Interfaces:**
- Consumes: nothing new.
- Produces: no new public interface — `verify_candidates`'s IO-budget accounting becomes more precise (an `Exact` or `Unresolvable` agreement result, which performs no walk, no longer decrements `io_budget`).

- [ ] **Step 1: Write the failing test**

Add to `src/features/references_verify.rs`'s test module. The construction deliberately keeps every candidate in ONE already-`index_content`-ed file, so `file_already_indexed` is `true` for all of them and the disk-read charge never fires for anyone — this isolates exactly what the fix changes (the agreement-walk charge) from the unrelated disk-read charge, instead of conflating the two:

```rust
    /// Budget precision: an `Unresolvable` agreement result (candidate type
    /// not indexed, so `receiver_type_agreement` returns before any walk)
    /// must NOT spend a budget unit. Spend the whole budget on
    /// `MAX_VERIFICATION_IO_OPERATIONS` such candidates, then prove one more
    /// `Exact`-match candidate (also 0 walk cost) still resolves
    /// `CstResolved` rather than falling to `NameScan` for lack of budget.
    /// Every candidate lives in the SAME already-indexed file, so the
    /// disk-read charge never fires for any of them -- only the
    /// agreement-walk charge this fix touches is in play.
    #[test]
    fn unresolvable_and_exact_agreement_do_not_spend_walk_budget() {
        let source = "class User { fun save() {} }\n\
                      fun filler(x: Ghost) { x.save() }\n\
                      fun caller(user: User) { user.save() }\n";
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        let filler_column = source.lines().nth(1).unwrap().find("save").unwrap() as u32;
        let filler_candidate = location(&file_uri, 1, filler_column, filler_column + 4);

        let real_column = source.lines().nth(2).unwrap().find("save").unwrap() as u32;
        let real_candidate = location(&file_uri, 2, real_column, real_column + 4);

        // MAX_VERIFICATION_IO_OPERATIONS copies of the SAME Unresolvable
        // candidate (position is all that matters -- classify_symbol_at is a
        // pure function of (uri, position), duplicates classify identically)
        // plus the one real Exact-match candidate at the end.
        let mut candidates: Vec<Location> =
            std::iter::repeat(filler_candidate).take(MAX_VERIFICATION_IO_OPERATIONS).collect();
        candidates.push(real_candidate.clone());

        let result = verify_candidates(&indexer, Some("User"), candidates);
        assert!(
            result
                .kept
                .iter()
                .any(|kept_source| matches!(
                    kept_source,
                    NavigationSource::CstResolved(location) if *location == real_candidate
                )),
            "the Exact-match candidate must resolve CstResolved even after \
             MAX_VERIFICATION_IO_OPERATIONS Unresolvable candidates precede it, \
             because neither Unresolvable nor Exact spends a walk-budget unit, \
             got {:?}",
            result.kept
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib unresolvable_and_exact_agreement_do_not_spend_walk_budget -- --nocapture`
Expected: FAIL — before this fix, each of the `MAX_VERIFICATION_IO_OPERATIONS` filler candidates spends 1 unit on the unconditional agreement charge even though its result is `Unresolvable` (no walk ran), so the budget is fully spent by the time the real candidate is reached; its own agreement check never runs (`io_budget == 0` short-circuits to `NameScan` before calling `receiver_type_agreement` at all), even though an `Exact` match costs nothing real.

- [ ] **Step 3: Fix the budget charge in both arms**

In `src/features/references_verify.rs`, the `Reference` arm currently charges unconditionally before calling `receiver_type_agreement`:

```rust
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
                match indexer.receiver_type_agreement(
                    &candidate_type,
                    candidate.uri.as_str(),
                    &query_declaring_type,
                    MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
                ) {
```

Replace with a charge that only fires when a walk will actually run — i.e. when the candidate type differs from the query type AND is indexed (the same two conditions `receiver_type_agreement` itself checks before walking):

```rust
            crate::indexer::SymbolRole::Reference {
                receiver_type: Some(receiver_type),
                ..
            } => {
                let candidate_type = ReceiverType::from_raw(receiver_type.clone()).leaf;
                // Only charge the agreement-walk unit when a walk will
                // actually run: `Exact` (same type, string equality) and
                // `Unresolvable` (candidate type not indexed) both return
                // from `receiver_type_agreement` before any supertype walk,
                // so charging for them exhausted the budget faster than the
                // real IO cost warranted.
                let will_walk = candidate_type != query_declaring_type
                    && indexer.has_type_definition(&candidate_type);
                if will_walk {
                    if io_budget == 0 {
                        kept.push(NavigationSource::NameScan(candidate));
                        continue;
                    }
                    io_budget -= 1;
                }
                match indexer.receiver_type_agreement(
                    &candidate_type,
                    candidate.uri.as_str(),
                    &query_declaring_type,
                    MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
                ) {
```

Apply the identical `will_walk`-gated charge in the `Declaration` arm Task 2 just added (replacing its unconditional call to `receiver_type_agreement` with the same pattern):

```rust
            crate::indexer::SymbolRole::Declaration { .. } => {
                let enclosing_class =
                    indexer.enclosing_class_at(&candidate.uri, candidate.range.start.line);
                match enclosing_class {
                    Some(class_name) => {
                        let candidate_type = ReceiverType::from_raw(class_name).leaf;
                        let will_walk = candidate_type != query_declaring_type
                            && indexer.has_type_definition(&candidate_type);
                        if will_walk {
                            if io_budget == 0 {
                                kept.push(NavigationSource::NameScan(candidate));
                                continue;
                            }
                            io_budget -= 1;
                        }
                        match indexer.receiver_type_agreement(
                            &candidate_type,
                            candidate.uri.as_str(),
                            &query_declaring_type,
                            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
                        ) {
                            ReceiverTypeAgreement::Exact | ReceiverTypeAgreement::Inherited => {
                                kept.push(NavigationSource::CstResolved(candidate));
                            }
                            ReceiverTypeAgreement::Unrelated
                            | ReceiverTypeAgreement::Unresolvable => {
                                kept.push(NavigationSource::NameScan(candidate));
                            }
                        }
                    }
                    None => kept.push(NavigationSource::NameScan(candidate)),
                }
            }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib unresolvable_and_exact_agreement_do_not_spend_walk_budget -- --nocapture`
Expected: PASS

- [ ] **Step 5: Run the full references_verify test module, including the pre-existing budget-exhaustion test**

Run: `cargo test --lib references_verify:: -- --nocapture`
Expected: all pass, including `budget_exhaustion_never_rejects_only_skips_verification` — that test's candidates are all provably `Unrelated` (a real walk that finds no match), so `will_walk` is `true` for every one of them and the budget math (`MAX_VERIFICATION_IO_OPERATIONS / 2` candidates fully verified) is unaffected by this fix.

- [ ] **Step 6: Commit**

```bash
git add src/features/references_verify.rs
git commit -m "fix(references): only charge IO budget when a walk actually runs

receiver_type_agreement's Exact (string equality) and Unresolvable
(has_type_definition short-circuit) results both return before any
supertype walk -- no IO is spent -- but the budget was charged
unconditionally before the call in both the Reference and (post
Declaration-arm-fix) Declaration arms. This accelerated exhaustion for
no real IO reason. Now the charge only fires when candidate_type !=
query_declaring_type and the candidate type is indexed -- the same two
conditions receiver_type_agreement checks before it walks."
```

---

### Task 4: Naming cleanup on unresolved PR #228 review threads

**Files:**
- Modify: `src/resolver/hierarchy_tests.rs` (full file — rename `u`/`idx` to `file_uri`/`indexer`)
- Modify: `src/features/references_verify.rs` (test module — rename `src`/`u`/`idx`/`col` to `source`/`file_uri`/`indexer`/`column`, in both pre-existing tests and the two this plan added)

**Interfaces:** None — pure identifier renames, zero behavior change.

- [ ] **Step 1: Rewrite `src/resolver/hierarchy_tests.rs`**

```rust
use super::{
    receiver_type_agreement, supertype_chain_contains, ReceiverTypeAgreement,
    MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
};
use crate::indexer::Indexer;
use tower_lsp::lsp_types::Url;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///t{path}")).unwrap()
}

fn indexed(path: &str, source: &str) -> (Url, Indexer) {
    let file_uri = uri(path);
    let indexer = Indexer::new();
    indexer.index_content(&file_uri, source);
    (file_uri, indexer)
}

#[test]
fn exact_type_match_is_exact() {
    let (file_uri, indexer) = indexed("/D.kt", "class User\n");
    assert_eq!(
        receiver_type_agreement(
            &indexer,
            "User",
            file_uri.as_str(),
            "User",
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
        ),
        ReceiverTypeAgreement::Exact
    );
}

#[test]
fn subtype_of_target_is_inherited() {
    let (file_uri, indexer) = indexed("/D.kt", "open class User\nclass DerivedUser : User()\n");
    assert!(supertype_chain_contains(
        &indexer,
        "DerivedUser",
        file_uri.as_str(),
        "User",
        MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
    ));
    assert_eq!(
        receiver_type_agreement(
            &indexer,
            "DerivedUser",
            file_uri.as_str(),
            "User",
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
        ),
        ReceiverTypeAgreement::Inherited
    );
}

#[test]
fn unrelated_indexed_type_is_unrelated() {
    let (file_uri, indexer) = indexed("/D.kt", "class User\nclass File\n");
    assert!(!supertype_chain_contains(
        &indexer,
        "File",
        file_uri.as_str(),
        "User",
        MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
    ));
    assert_eq!(
        receiver_type_agreement(
            &indexer,
            "File",
            file_uri.as_str(),
            "User",
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
        ),
        ReceiverTypeAgreement::Unrelated
    );
}

#[test]
fn unindexed_type_is_unresolvable_not_unrelated() {
    let (file_uri, indexer) = indexed("/D.kt", "class User\n");
    // "Ghost" is never declared anywhere -- has_type_definition fails, so we
    // must NOT claim to have proven it's unrelated to User.
    assert_eq!(
        receiver_type_agreement(
            &indexer,
            "Ghost",
            file_uri.as_str(),
            "User",
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
        ),
        ReceiverTypeAgreement::Unresolvable
    );
}

/// House decoy: a two-level hierarchy -- the target is a grandparent, not the
/// immediate supertype.
#[test]
fn transitive_supertype_is_inherited() {
    let (file_uri, indexer) = indexed(
        "/D.kt",
        "open class Base\nopen class Middle : Base()\nclass Leaf : Middle()\n",
    );
    assert_eq!(
        receiver_type_agreement(
            &indexer,
            "Leaf",
            file_uri.as_str(),
            "Base",
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
        ),
        ReceiverTypeAgreement::Inherited
    );
}
```

- [ ] **Step 2: Rewrite `src/features/references_verify.rs`'s test module identifiers**

In the same file, rename every abbreviated local binding across all 6 tests (the 4 pre-existing ones plus the 2 this plan added in Tasks 2 and 3):
- `src` → `source`
- `u` → `file_uri`
- `idx` → `indexer`
- `col` → `column`

The helper functions at the top of the module:

```rust
    fn uri(path: &str) -> Url {
        Url::parse(&format!("file:///t{path}")).unwrap()
    }

    fn location(file_uri: &Url, line: u32, column_start: u32, column_end: u32) -> Location {
        Location {
            uri: file_uri.clone(),
            range: Range::new(
                Position::new(line, column_start),
                Position::new(line, column_end),
            ),
        }
    }
```

Apply the same renames throughout `unrelated_candidate_is_rejected_not_dropped_silently`, `inherited_candidate_is_kept_as_cst_resolved`, `no_query_identity_passes_every_candidate_through_as_name_scan`, `budget_exhaustion_never_rejects_only_skips_verification`, `override_declaration_is_kept_as_cst_resolved_not_name_scan` (Task 2), and `unresolvable_and_exact_agreement_do_not_spend_walk_budget` (Task 3) — every `let src = "..."` becomes `let source = "..."`, every `let u = uri(...)` becomes `let file_uri = uri(...)`, every `let idx = Indexer::new()` becomes `let indexer = Indexer::new()`, every `let col = ...` becomes `let column = ...`, and every downstream reference to those bindings updates to match.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test --lib hierarchy_tests:: references_verify:: -- --nocapture`
Expected: all pass, identical assertions to before this task — this step is a pure rename with no logic change.

Run: `cargo build 2>&1 | grep -i warning`
Expected: no new warnings (confirms no stray unused-import or shadowing issue from the rename).

- [ ] **Step 4: Commit**

```bash
git add src/resolver/hierarchy_tests.rs src/features/references_verify.rs
git commit -m "style(resolver,references): expand abbreviated test identifiers

u -> file_uri, idx -> indexer, src -> source, col -> column, across
hierarchy_tests.rs and references_verify.rs's test module. AGENTS.md
disallows abbreviated identifiers; these were flagged by Copilot review
on PR #228 and left unresolved at merge."
```

---

## Self-Review Notes

**Spec coverage:** all four "6b-hardening" items from the spec have a task — Declaration-arm fix (Task 2), IO-budget charge-only-when-walking (Task 3), `walk_hierarchy` sidecar-budget parameterization (Task 1), naming cleanup (Task 4). The spec's dropped "item 2a" (the `live_trees` claim that turned out moot on inspection) has no task — correctly, since the critique found the code already behaves correctly there and touching it would be an unrequested, unjustified change.

**Ordering:** Task 1 must land before Tasks 2/3 (they call the now-5-parameter `receiver_type_agreement`). Task 2 must land before Task 3 (Task 3's Declaration-arm charge fix operates on the call Task 2 introduces). Task 4 touches the same test files as Tasks 2/3 and must land last to avoid merge friction on in-flight abbreviated names.

**Type consistency:** the free-function `hierarchy::receiver_type_agreement(indexer: &Indexer, candidate_type: &str, candidate_uri: &str, target_type: &str, sidecar_budget: usize) -> ReceiverTypeAgreement` and its catalogue wrapper `Resolver::receiver_type_agreement(&self, candidate_type: &str, candidate_uri: &str, target_type: &str, sidecar_budget: usize) -> ReceiverTypeAgreement` are consistent everywhere this plan touches them: production code (`references_verify.rs`) calls the trait method (`indexer.receiver_type_agreement(...)`); `hierarchy_tests.rs` continues testing the free function directly. The next plan (6c rename) calls the SAME trait method with a non-default (`usize::MAX`) budget — it must not re-import the free function.
