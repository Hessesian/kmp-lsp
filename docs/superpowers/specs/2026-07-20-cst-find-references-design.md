# Find-References Verification Layer — Design (Slice 6b)

Status: **approved design** (brainstormed with the user 2026-07-20; rechecked twice against
`AGENTS.md` and the parent CST design's "Type-driven correctness" section — once for this
design, once for slice 6a's already-shipped code, surfacing two real fixes applied ahead of
this slice; independently critiqued and amended 3× before implementation, see "Critique
findings applied" below). Slice 6b of
[CST resolution unification](2026-06-30-cst-resolution-unification-design.md), continuing
[slice 6's navigation design](2026-07-19-cst-navigation-design.md). Branch:
`refactor/cst-navigation-6b` off `refactor/unified-resolution` (post-#227).

## Context (why)

Today's `find_references_with_qualifier` (`features/references.rs`) is a pure recall engine:
resolve a scope once (`parent_class`/`declared_pkg`, derived from the cursor's word/qualifier
text), then let `rg` text-search that scope and return every match, filtered only by
line-level qualifier heuristics (`has_wrong_qualifier_at_col`). It does no per-occurrence
identity verification — two unrelated classes with a same-named member (`User.save()` /
`File.save()`) can both surface in one `save` query if rg's scope narrowing doesn't separate
them, exactly the class of false positive the CST classifier built in 6a exists to eliminate
for go-to-definition, goto-implementation, and highlight.

## Critique findings applied

An independent adversarial critique (before implementation, same discipline as slices 4 and
6a) verified every claim in this spec against the actual code and found three issues, now
folded into the design below:

1. **The original "Retrofit" goal was dropped — its motivating scenario is unreachable.**
   The critique traced two independent layers that already prevent a generic-typed raw string
   from ever reaching `resolve_identity`'s `Reference` arm: `classify_symbol_at`'s own
   `has_type_definition` gate does an exact-string lookup that a `"List<String>"`-shaped type
   would already fail (falling to `receiver_type: None` before `resolve_identity` runs at
   all), and *upstream* of that, `infer_ident_type` (`indexer/infer/expr_type.rs`) already
   strips generics before the type string is produced, with a comment explaining exactly why.
   6a has no bug here to retrofit. `ReceiverType::from_raw` is still reused for normalization
   in 6b's own new comparison code (dotted nested types, trailing `?`), just not framed as
   fixing a pre-existing defect.
2. **Query-identity classification would have been a third hand-rolled copy of the same
   prologue.** `definition.rs::try_cst_resolved_definition` and
   `implementation.rs::find_implementation_at` (both slice 6a) already open with the identical
   `Position → CursorPos` conversion + `classify_symbol_at` call + `SymbolRole` match. 6b adds
   a shared `classify_cursor(indexer, uri, position) -> Option<SymbolAtCursor>` helper in
   `cst_symbol.rs` and migrates all three call sites onto it — consistent with how slice 6's
   own design already named and extracted `resolve_identity`/`local_scope_occurrences` for the
   identical reason.
3. **Rejected candidates were being silently dropped with no typed trace.** The grandparent
   design's "Type-driven correctness" rule #1 and slice 6's own "typed provenance ... testable"
   goal both argue against a *proven* fact (this candidate belongs to an unrelated type)
   disappearing into array-absence, indistinguishable from a future regression that
   misclassifies an `Inherited` candidate as `Unrelated`. The verification pass now produces a
   typed audit trail for rejections (see "Result type" below) so exclusion is a directly
   assertable fact, not an inference from what's missing.

## Decisions locked with the user

- **Recall is untouched.** The existing rg + index candidate search stays exactly as today —
  6b adds a verification pass on top, never changes what candidates get found.
- **Budget is about IO, not candidate count.** Verifying a candidate can require two IO-costed
  operations: a disk read (when the candidate's file isn't already indexed/open — the common
  case for a candidate rg found is already in-memory, so this is rare) and a blocking JAR
  sidecar IPC round trip (when the supertype walk needs an ancestor class from a
  not-yet-materialized JAR — `walk_hierarchy` already self-limits this to
  `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK = 3` per walk, but that cap is *per walk*, and a
  find-references request can trigger many walks). The concern, stated directly: avoid a
  bulk/agent-driven query (e.g. a startup workspace scan) silently spawning large numbers of
  these IO operations. One request-scoped budget covers both.

## Goals / non-goals

**Goals**

1. Verify each rg-found candidate's identity via `classify_symbol_at` (reused, unchanged) +
   a new receiver-type agreement check, budgeted on IO.
2. Reject candidates the CST *proves* belong to an unrelated type — the actual precision gain,
   tracked through a typed rejection trail (see "Result type"), not a silent drop.
3. Never reduce recall: every candidate that isn't proven unrelated stays in the result,
   labeled by how it was determined (`NavigationSource<Location>` internally).
4. Extract the `classify_symbol_at` + `Position→CursorPos` + `SymbolRole` prologue —
   already duplicated in `definition.rs` and `implementation.rs` — into one shared
   `classify_cursor` helper, and migrate both existing call sites onto it.

**Non-goals**

- General local-variable verification — same scope cut as 6a; bare/local reference candidates
  are unaffected by this slice.
- Changing rg's scope-narrowing inputs (`parent_class`/`declared_pkg`/`owner_class`) — those
  stay exactly as today; verification is a pass *after* recall, not a replacement for it.

## Design

### Shared prologue: `classify_cursor`

New in `cst_symbol.rs`, extracted from the identical prologue already duplicated in
`definition.rs::try_cst_resolved_definition` and `implementation.rs::find_implementation_at`:

```rust
pub(crate) fn classify_cursor(
    indexer: &Indexer, uri: &Url, position: Position,
) -> Option<SymbolAtCursor>
```

Does the `Position → CursorPos` conversion once and calls `classify_symbol_at`. Both existing
6a call sites migrate onto it (behavior-neutral — pure extraction) as part of this slice.

### Query identity

Thread `position.character` through to `find_references_with_qualifier` (today only `.line` is
passed) and call `classify_cursor(indexer, uri, position)` on the request's own cursor —
reusing 6a exactly, symmetric with how it verifies candidates below. The result gives a
**query declaring type**:
- `SymbolRole::Declaration { .. }` → the query's own enclosing class (via the existing
  `enclosing_class_at`).
- `SymbolRole::Reference { receiver_type: Some(receiver_type), .. }` → `receiver_type`,
  normalized (see below).
- Anything else (classification fails, bare reference, import segment) → no query identity;
  verification is skipped entirely and the recall set passes through unchanged, exactly like
  today. This is the same "CST can't narrow it" fallback shape 6a already established.

### Type normalization (reused, not reinvented)

Both the query declaring type and every candidate's `receiver_type` go through
`ReceiverType::from_raw(raw).leaf` (or `.qualified` when a dotted nested-type match matters)
before comparison — the exact type `resolver/infer.rs` already defines for this precise
purpose (raw/qualified/outer/leaf/nullable breakdown). No new normalization type is invented.
(Confirmed during critique: `classify_symbol_at`'s `receiver_type` is already generics-free
by construction — `infer_ident_type` strips generics upstream, and `has_type_definition`'s
exact-match gate would reject a generic-shaped string anyway — so this normalization step
exists for dotted-nested-type and nullable-suffix handling, not generics.)

### Verification outcome (named, not left as if/else)

```rust
enum ReceiverTypeAgreement {
    Exact,
    Inherited,     // candidate type is a subtype of the query's declaring type
    Unrelated,     // confirmed different type — reject
    Unresolvable,  // stays NameScan
}
```

`Exact` is a plain normalized string-equality check — no hierarchy walk. `Inherited` covers a
subtype accessing an inherited member and is checked only when `Exact` fails, via a new
`resolver::hierarchy::supertype_chain_contains(indexer, candidate_type, candidate_uri,
target_type) -> bool` — an ascending `walk_hierarchy` call from the candidate's type, checking
whether the query's declaring type appears among its supertypes (the exact mechanism
`resolve_from_class_hierarchy` already uses for the string engine's own inherited-member
lookups, applied here in the reverse direction: not "find the member," but "does this
ancestor chain contain that type").

### Per-candidate verification, IO-budgeted

For each candidate `Location` from the *unchanged* recall set:

1. Classify it: `classify_symbol_at(indexer, &candidate.uri, candidate.range.start.into())`.
   Acquisition (`live_doc_or_parse`) is free when the candidate's file is already
   indexed/open — the common case, since rg found it inside the workspace the scan already
   covers — and costs one disk read (budget-metered) otherwise.
2. If classification isn't a `Reference` with a resolvable `receiver_type` (or the candidate
   is a `Declaration` — handled separately below), it's inconclusive: `NavigationSource::NameScan(candidate)`, unchanged from today.
3. Otherwise compute `ReceiverTypeAgreement` against the query declaring type. `Inherited`
   requires `supertype_chain_contains`, which may spend hierarchy-walk IO
   (budget-metered — this is the sidecar-IPC-costed path, not the disk-read one, but shares
   the SAME request-scoped budget counter per the locked decision above).
4. Map the outcome: `Exact`/`Inherited` → kept as `NavigationSource::CstResolved(candidate)`;
   `Unresolvable` → kept as `NavigationSource::NameScan(candidate)`; `Unrelated` → moved to the
   rejection trail (see "Result type" below), not silently dropped.
5. Once the IO budget is exhausted, every remaining candidate is left as
   `NavigationSource::NameScan(candidate)` without attempting further classification —
   recall never drops because of the budget, only additional precision stops accruing.

**Declaration-role candidates** (a candidate that is itself a declaration site — an override
in a subclass, say): verified by exact `(name, enclosing class)` match against the query's own
`(name, declaring type)` — no hierarchy walk. An override in a subclass is a *different*
declaration and is correctly excluded, matching how other LSPs scope find-references
(goto-implementation, already shipped in 6a, is the feature for "show me every override").

### Result type and the LSP boundary

The verification pass returns a single struct, not a bare `Vec`, so accepted results and
rejections are each their own typed field rather than one collapsing into the absence of the
other:

```rust
pub(crate) struct VerifiedReferences {
    pub kept: Vec<NavigationSource<Location>>,
    /// Candidates the CST *proved* belong to an unrelated type — the actual
    /// precision gain, kept as an assertable fact (critique finding: a
    /// proven exclusion silently collapsing to array-absence is exactly
    /// what the project's "one outcome enum, never empty-Vec overloading"
    /// rule targets). Never surfaced to the LSP client.
    pub rejected: Vec<Location>,
}
```

`kept` is carried as a real type through the whole internal pipeline, never an implicit
"resolved ones happen to be sorted first" convention (the flaw caught rechecking this design
against the base plan the first time). Exactly one explicit, named flatten step at the very
end converts `kept` to the `Vec<Location>` the LSP wire format requires, `CstResolved` entries
first: this mirrors `resolve_identity` → `locs_to_opt_response` in 6a — a deliberate,
documented ordering decision made once at the boundary, not a fact a caller has to infer from
array position. `rejected` is test/tracing-only — a house decoy asserts a specific location
appears there, not merely that it's absent from `kept`.

### Error handling

- Classification failure, unresolvable types, and budget exhaustion all keep the candidate in
  `kept` as `NameScan` — never an error, never an unaccounted-for result.
- Only a *proven* `Unrelated` agreement moves a candidate to `rejected`. Every other outcome
  stays in `kept`.

### Testing

- House decoy, extended: `User.save()` / `File.save()`, find-references on `User.save` must
  put every `File.save()` call site in `VerifiedReferences::rejected` (asserted directly, not
  inferred from `kept`'s absence) while an inherited reference through a `DerivedUser : User`
  subtype instance must appear in `kept` as `CstResolved` (`Inherited` proof).
- Budget decoy: construct a request where the IO cap is hit before all candidates are
  verified; assert the un-verified tail is present in `kept` as `NameScan`, never dropped and
  never in `rejected` (budget exhaustion is not evidence of unrelatedness).
- `classify_cursor` extraction decoy: `definition.rs`'s and `implementation.rs`'s existing
  test suites must pass unchanged after migrating both onto the shared helper — pure
  extraction, zero behavior change.
- Existing `references.rs` test suite is the recall-parity floor — every existing test's
  candidate SET must be unchanged; only intra-set labeling/filtering changes.
- Live probe on the real project before merge: find-references on a same-named member across
  two unrelated classes, and on an inherited member accessed through a subtype instance.
