# Find-References Verification Layer — Design (Slice 6b)

Status: **approved design** (brainstormed with the user 2026-07-20; rechecked twice against
`AGENTS.md` and the parent CST design's "Type-driven correctness" section — once for this
design, once for slice 6a's already-shipped code, surfacing two real fixes applied ahead of
this slice, see "Retrofit" below). Slice 6b of
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
2. Reject candidates the CST *proves* belong to an unrelated type — the actual precision gain.
3. Never reduce recall: every candidate that isn't proven unrelated stays in the result,
   labeled by how it was determined (`NavigationSource<Location>` internally).
4. Retrofit 6a's own `resolve_identity` `Reference` arm to use the same type normalization 6b
   introduces (see "Retrofit").

**Non-goals**

- General local-variable verification — same scope cut as 6a; bare/local reference candidates
  are unaffected by this slice.
- Changing rg's scope-narrowing inputs (`parent_class`/`declared_pkg`/`owner_class`) — those
  stay exactly as today; verification is a pass *after* recall, not a replacement for it.

## Design

### Query identity

Thread `position.character` through to `find_references_with_qualifier` (today only `.line` is
passed) and call `classify_symbol_at(indexer, uri, cursor)` on the request's own cursor —
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

**Retrofit**: 6a's `resolve_identity` `Reference` arm currently passes the raw `receiver_type`
string straight into `find_definition_qualified` — for a generic-typed receiver
(`items: List<String>`, `items.someExtension()`) this looks up a class literally named
`"List<String>"`, finds nothing, and falls back to `NameScan` (safe — the `locs.is_empty()`
guard catches it, never a wrong jump, just a missed precision opportunity). 6b's
normalization pass fixes this at its source: `resolve_identity` normalizes via
`ReceiverType::from_raw(receiver_type).leaf` before the lookup, so both 6a's own precision and
6b's new verification share one normalization path.

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
   requires `type_extends_or_equals`, which may spend hierarchy-walk IO
   (budget-metered — this is the sidecar-IPC-costed path, not the disk-read one, but shares
   the SAME request-scoped budget counter per the locked decision above).
4. Map the outcome: `Exact`/`Inherited` → `NavigationSource::CstResolved(candidate)`;
   `Unrelated` → dropped from the result entirely; `Unresolvable` → `NavigationSource::NameScan(candidate)`.
5. Once the IO budget is exhausted, every remaining candidate is left as
   `NavigationSource::NameScan(candidate)` without attempting further classification —
   recall never drops because of the budget, only additional precision stops accruing.

**Declaration-role candidates** (a candidate that is itself a declaration site — an override
in a subclass, say): verified by exact `(name, enclosing class)` match against the query's own
`(name, declaring type)` — no hierarchy walk. An override in a subclass is a *different*
declaration and is correctly excluded, matching how other LSPs scope find-references
(goto-implementation, already shipped in 6a, is the feature for "show me every override").

### Result type and the LSP boundary

The verification pass produces `Vec<NavigationSource<Location>>` — carried as a real type
through the whole internal pipeline, never an implicit "resolved ones happen to be sorted
first" convention (the flaw caught rechecking this design against the base plan the first
time). Exactly one explicit, named flatten step at the very end converts this to the
`Vec<Location>` the LSP wire format requires, `CstResolved` entries first: this mirrors
`resolve_identity` → `locs_to_opt_response` in 6a — a deliberate, documented ordering decision
made once at the boundary, not a fact a caller has to infer from array position.

### Error handling

- Classification failure, unresolvable types, and budget exhaustion all degrade to `NameScan`
  — never an error, never a dropped-without-cause result.
- Only a *proven* `Unrelated` agreement drops a candidate. Every other outcome keeps it.

### Testing

- House decoy, extended: `User.save()` / `File.save()`, find-references on `User.save` must
  exclude every `File.save()` call site (the actual precision proof) while an inherited
  reference through a `DerivedUser : User` subtype instance must still appear (`Inherited`
  proof).
- Budget decoy: construct a request where the IO cap is hit before all candidates are
  verified; assert the un-verified tail is present as `NameScan`, never dropped.
- Retrofit decoy: a generic-typed receiver (`items: List<String>`, calling a workspace
  extension function on it) resolves `CstResolved` via go-to-definition after the
  normalization retrofit, where it previously fell back to `NameScan`.
- Existing `references.rs` test suite is the recall-parity floor — every existing test's
  candidate SET must be unchanged; only intra-set labeling/filtering changes.
- Live probe on the real project before merge: find-references on a same-named member across
  two unrelated classes, and on an inherited member accessed through a subtype instance.
