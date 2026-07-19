# CST-Aware Navigation — Design (Slice 6)

Status: **approved design** (brainstormed with the user 2026-07-19; decisions: three sub-slices
under one spec; rename refuses unless CST-resolved). Slice 6 of
[CST resolution unification](2026-06-30-cst-resolution-unification-design.md), expanding the
sketch in `docs/superpowers/plans/2026-07-05-post-fable-roadmap-refinements.md` §6.
Branch: `refactor/cst-navigation` off `refactor/unified-resolution` (post-#224).
Sub-slices land as separate PRs: 6a → 6b → 6c, each live-verified before the next.

## Context (why)

The symbol-identity navigation family — go-to-definition, goto-implementation,
find-references, document-highlight, rename — answers "which symbol does this identifier
refer to?" name-based today. Same-named members on unrelated types collide, locals bleed
across scopes, and rename is a silent gamble in ambiguous states. The CST engine built up
through slices 1-4 (receiver typing via `CstQuery::expr_type`, uri-threaded member lookup,
repair-wired tree acquisition via `lambda_doc_at`) can now answer identity precisely — the
features just don't ask it.

Current surfaces: `features/definition.rs` (195 lines), `references.rs` (616),
`rename.rs` (476), `highlight.rs` (54), `implementation.rs` (219). Shared per-request context:
`backend/cursor.rs::CursorContext` — string-first (`word_and_qualifier_at` text scan) with a
CST bridge for contextual receivers.

## Decisions locked with the user

- **Decomposition:** one spec, three implementation cycles: 6a = shared core + read-only
  features (go-def, goto-impl, document-highlight); 6b = find-references; 6c = rename.
- **Rename policy:** rename requires `CstResolved` identity; any `NameScan` residue in the
  edit set ⇒ typed refusal surfaced as an LSP error with a human-readable reason. Never
  text-rename on a name-scan source.
- **Approach:** new classification layer in the CST domain (not an extension of
  `CursorContext`, not a hover rewrite). `CursorContext` stays for hover; goto-def migrates
  off it in 6a. The string engine remains the guaranteed fallback for every feature — cold
  start, unsynced projects, and agent find must keep working exactly as today.

## Goals / non-goals

**Goals**

1. One classifier: `classify_symbol_at(indexer, uri, pos) -> Option<SymbolAtCursor>` in the
   CST domain, reusing `cursor_node_at`, `lambda_doc_at`, and `CstQuery` resolution.
2. Typed provenance: `NavigationSource::{CstResolved, NameScan}` on every navigation result
   path — the fallback is visible in code, rankable, and testable.
3. 6a: precise jumps + scope-correct highlight. 6b: receiver-verified references without
   losing recall. 6c: rename that is either right or refuses with a reason.
4. Existing navigation behavior is the floor: `NameScan` reproduces today's results wherever
   the CST cannot establish identity.

**Non-goals**

- Hover, document-symbol, workspace-symbols — not in the identity family.
- Deleting `CursorContext` (hover keeps it; shrinking it is post-6a cleanup, own change).
- Unifying the string engine's internals (locked non-goal of the parent design).
- Cross-file type-hierarchy-wide rename semantics (e.g. renaming an override renames the
  base): follow Kotlin LSP conventions later; 6c renames the exact identity under the
  cursor and its verified references only, refusing when override relationships make the
  edit set ambiguous (detected via the existing supertype machinery).

## Design

### Core (built in 6a): `SymbolAtCursor` + `NavigationSource`

```rust
// indexer/infer/symbol_at_cursor.rs (new module, catalogued in infer/mod.rs)
pub(crate) struct SymbolAtCursor {
    pub name: String,
    pub role: SymbolRole,
    /// Receiver TYPE for member references (`user.save` → "User"), resolved via
    /// `CstQuery::expr_type` on the receiver subtree. `None` for bare names.
    pub receiver_type: Option<String>,
}

pub(crate) enum SymbolRole {
    /// The name token of a declaration; `kind` from the declaration node.
    Declaration { kind: DeclarationKind },   // Class | Function | Property | LambdaParam | …
    /// An identifier in expression/type position.
    Reference,
    /// A segment of an import path (navigation targets the imported symbol).
    ImportSegment,
}

pub(crate) enum NavigationSource<T> {
    /// Identity established from the CST + index: precise, ranked first.
    CstResolved(T),
    /// Name-based scan (string engine / rg): today's behavior, visibly labeled.
    NameScan(T),
}

pub(crate) fn classify_symbol_at(
    indexer: &Indexer, uri: &Url, pos: CursorPos,
) -> Option<SymbolAtCursor>
```

Classification is one CST pass at the cursor node:
- **Declaration**: the identifier is the name child of a declaration node
  (`class_declaration`, `object_declaration`, `function_declaration`, `property_declaration`,
  parameter, lambda parameter).
- **Reference**: identifier in expression or type position; if its parent is a
  `navigation_suffix`, extract the receiver subtree and type it via `CstQuery::expr_type`
  (the #222/#224 machinery — uri-threaded member lookup included).
- **ImportSegment**: identifier inside an `import_header` path.
- **`None` (not a symbol)**: cursor in a string literal (non-interpolated part), comment, or
  whitespace — navigation features return no result instead of name-scanning strings.
- Acquisition through `lambda_doc_at`, so mid-typing states classify against the repaired
  tree; classification never triggers the marker-insertion transform (the cursor sits on an
  existing token, not a completion gap).

The classifier produces IDENTITY, not locations — each feature keeps its own lookup, keyed
by that identity, and wraps results in `NavigationSource`.

### 6a — go-def, goto-impl, document-highlight

- **go-def**: `Reference` with `receiver_type` → receiver-typed member lookup (the
  `method_return_type`/`resolve_member` family) → `CstResolved` jump. Local/lambda-param
  references → the declaration node found by walking enclosing scopes in the CST.
  `Declaration` role → the symbol IS the definition (return self-location, matching LSP
  convention). Anything unresolvable → today's path (index by name + rg), wrapped `NameScan`.
  Ranking: `CstResolved` results first when both exist.
- **goto-impl**: same identity feeds the existing subtype lookup; receiver-typed identity
  filters same-named interfaces.
- **highlight** (54 lines): for locals/params, highlight only occurrences within the
  declaration's scope subtree (pure CST walk — no index needed); everything else keeps
  today's behavior via `NameScan`.

### 6b — find-references

Recall engine unchanged: the name scan (index + rg) still FINDS candidate sites. The
classifier then VERIFIES each candidate: run `classify_symbol_at` at the candidate position;
a member reference matches only if its `receiver_type` agrees with the query identity (via
the existing supertype walk for inherited members); declarations match only their own scope.
Cost is bounded: one transient parse per candidate FILE (`live_doc_or_parse` reuses
live/indexed content), candidates only. Unverifiable candidates (parse failed, receiver
untypeable) are KEPT and labeled `NameScan` — recall never drops below today's. The response
concatenates `CstResolved` first, then surviving `NameScan` entries.

### 6c — rename

1. Classify the cursor symbol: must be `CstResolved` identity (a `Declaration`, or a
   `Reference` whose definition resolves uniquely in the workspace). Jar/library-defined
   symbols refuse ("defined in a library").
2. Collect 6b's verified reference set. If ANY candidate in the set is `NameScan`
   (unverifiable), refuse: the edit would gamble.
3. Override ambiguity: if the symbol participates in an override relationship (existing
   supertype machinery detects it), refuse with that reason (deferred semantics, see
   non-goals).
4. Refusal = LSP request error with a human-readable reason string (Helix shows it in the
   status line). Success = `WorkspaceEdit` over exactly the verified set.

### Error handling

- Classifier returns `None` → features fall back exactly as today (never an error).
- Repair-bounded acquisition inherited from `lambda_doc_at`; classification on a broken tree
  that repair can't fix degrades to `NameScan`, never wrong-identity.
- Rename refusals are errors BY DESIGN and carry reasons; all other features never error.

### Testing

House decoy for every sub-slice — two classes with identically-named members
(`User.save()` / `File.save()`):
- 6a: go-def from `user.save()` hits `User.save` only (decoy: `assert` the `File.save`
  location is absent); local highlight does not light the same name in another function.
- 6b: find-refs on `User.save` excludes `File.save` call sites; unverifiable site stays in
  the result as `NameScan` (recall pin).
- 6c: rename refuses on an untypeable receiver (assert the error reason), renames exactly
  the verified set in the clean fixture, refuses on jar symbols and overrides.
- Cursor in string/comment → no navigation result (noise-kill pin).
- Existing navigation suites are the floor — `NameScan` parity means they pass unchanged.
- Live probe per sub-slice on the real project before its merge (go-def on a Compose member
  chain; find-refs on a same-named workspace member; rename refusal on a jar symbol).
