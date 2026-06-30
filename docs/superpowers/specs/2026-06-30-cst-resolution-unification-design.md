# CST Resolution Unification — Design

Status: **approved design** (brainstormed with the user 2026-06-30). Implementation plan to follow
(writing-plans). Branch: `refactor/cst-resolution` off `refactor/unified-resolution`.

## Context (why)

Symbol/type resolution in the LSP is **fragmented**: the same question — "what's the type of this
receiver / expression / lambda variable?" — is answered by several engines, each with its own walk,
reachability filter, generics handling, and `it`/`this` logic. Divergent copies are the bug factory:
most resolution fixes have been per-consumer patches over the same gaps.

There are **two deliberately separate domains**, and they should stay separate:

- **String path** (`resolver/resolve.rs`, `resolver/complete.rs`, the string `infer_*`) — powers
  go-to-def / find-refs **without a synced project**, plus optimized agent find. Intentionally
  heuristic; "good enough" is the bar. **Out of scope** here. Whether anything is shareable among its
  ad-hoc heuristics is a separate later investigation.
- **CST path** (`indexer/infer/*` + a bespoke walk in `semantic_tokens/resolve.rs`) — the
  **authoritative** engine for LSP features + diagnostics. **This is what we unify.**

### The CST fragmentation, concretely

Within the CST domain there are **four parallel mechanisms** answering overlapping questions:

| Mechanism | ~lines | Strategy |
|---|---|---|
| `indexer/infer/cst_lambda.rs` | ~892 | CST-node based (`ThisLambdaCtx`) |
| `indexer/infer/receiver.rs` | ~518 | text-context heuristics (~12 `*_lambda_type` variants) |
| `indexer/infer/it_this.rs` | ~523 | line-string scans (`find_it_element_type*`) |
| `indexer/infer/chain.rs` | ~553 | nav-chain segment walk (chain logic also echoed in `receiver.rs`) |
| `semantic_tokens/resolve.rs` `expression_type` family | ~150 | **its own** CST walk, wired sideways into the *string* engine (`use crate::resolver::infer::…`) |

The three lambda/`it`/`this` engines (CST + text + line) all answer the *same* question with different
mechanisms — that triad is where the bulk of the ~2,000 lines and the divergence bugs live. Consumers
reach into three different entry points (`infer_lambda_param_type_at`, `lambda_receiver_type_from_context`,
`find_it_element_type*`), so each feature re-derives slightly differently → the next divergence.

The CST engine already has good bones: a documented one-responsibility submodule layout, the
**`InferDeps`** data-access trait (+ `TestDeps` stub for unit-testing), and pure-read infer functions.
What's missing is a **single catalogue** consumers request from instead of hand-rolling.

**Reference:** `docs/architecture/parse-to-lsp-paths.md` maps the full parse→LSP request pipeline and
classifies every feature's resolution engine (string / CST / both) — the redesign's blast-radius map.
Key caveat from it: `CursorContext::build` bridges nominally-*string* features (hover, goto-def) into
the CST engine via `infer_variable_type_from_cst`, so the CST blast radius is wider than the obviously-CST
feature set. Conversely, signature help is a *hard* string/CST split (CST = *where* the call/param is;
string = *what* the signature text says) that does not collapse into one engine.

## Goals / non-goals

**Goals**
1. One **CST-driven walk** (CST is authoritative) that does the proper traversal through long dotted
   chains *and* lambda bodies, absorbing the text/line lambda engines and the chain walk.
2. A **single catalogue facade trait** (`CstResolve`) that is the complete, intent-named map of CST
   resolution capabilities, returning self-documenting types. Built on `InferDeps`; pure reads.
3. **`mod.rs` is the catalogue** — every resolution module (and consuming feature) exposes its I/O
   structs + catalogue trait in `mod.rs` with zero logic, so agents read one file and route to the
   existing capability instead of reinventing.
4. **Model the domain in enums/structs** so the compiler catches logic errors (illegal states
   unrepresentable).
5. Delete the reinvented walks (`semantic_tokens` bespoke walk, the text/line lambda engines, echoed
   chain logic) — the substantial deletion the catalogue work so far did not deliver.
6. Make the **symbol-identity-at-cursor** navigation family **CST-aware** — go-to-def,
   goto-implementation, find-references, document-highlight, rename. They all answer "which symbol does
   this identifier refer to?" name-based today (hence noisy); the catalogue gives them the precise
   receiver-typed identity, with string + rg as the guaranteed fallback. Layered, not engine-merging.
   (Post-catalogue phase.) Pure listing/fuzzy features (document-symbol, workspace-symbols) are **not**
   in this family and stay as-is.

**Non-goals**
- Unifying the **string engine's internal heuristics** (heuristic by design; separate later analysis).
  Note: navigation features *gain a CST-first layer* (Goal 6), but the string engine itself is untouched
  and **remains the fallback** — go-def/find-refs must still work with no CST (cold start / unsynced /
  agent find).
- A shared cross-domain **IR** that both string and CST paths lower into — explicitly rejected as
  added complexity / a new domain.
- Changing observable behaviour beyond intended consolidation (existing CST suites are the net).

## Architecture

One shared CST resolution library inside `indexer/infer`, exposing a single facade catalogue trait
built on the existing `InferDeps` data seam. Every CST consumer requests through the facade.

```
features (hover, semantic_tokens, inlay, diagnostics, completion, sig-help, fill_when)
      │  each exposes its own thin *feature catalogue trait* in its mod.rs
      ▼
  CstResolve  ◄── THE catalogue facade (capabilities + self-documenting I/O types)
      │  built on
      ▼
  InferDeps (data seam, exists)  +  ONE unified CST walk
```

Design tracing (per house rule WHY→WHAT→HOW):
- **WHY** — infer functions are pure reads over a snapshot; consumers must not re-derive resolution; and
  the ~50 lambda/chain functions all thread the same `(node, doc, deps, uri)` bundle — a missing struct.
- **WHAT** — a `CstQuery` struct that bundles a node-in-its-document-and-index, carrying the catalogue
  methods. Self-documenting newtypes; one walk that lives on the struct.
- **HOW** — `struct CstQuery<'a, D: InferDeps>` (static dispatch, generics over `dyn`); the walk is
  `self.at(parent)`; `TestDeps` drives it because it holds `&D`, not `&Indexer`.

## The `CstQuery` catalogue (the resolution surface)

The repeated `(node, doc/bytes, idx, uri)` parameter bundle threaded through the whole CST engine *is*
the missing struct (the house "long parameter lists signal a missing struct" rule). Bundle it once; the
catalogue methods hang off it; the walk becomes `self.at(parent)`. Generic over `D: InferDeps` so
`TestDeps` still drives it without an `Indexer` (a `CstQuery` holding `&Indexer` would silently kill the
unit-test seam); `doc`/bytes are carried separately because `InferDeps` deliberately excludes the live doc.

```rust
/// A CST node positioned in its document + index — the surface for CST-driven
/// type resolution (LSP + diagnostics). Pure reads over `InferDeps`. Every
/// returned type is ALREADY reachability-filtered and generic-substituted.
pub(crate) struct CstQuery<'a, D: InferDeps> {
    node: Node<'a>,
    doc:  &'a LiveDoc,      // bytes + live tree
    deps: &'a D,
    uri:  &'a Url,
    io:   ResolveIo,        // bounds underlying name→def lookups (hot paths pass IndexOnly)
}

impl<'a, D: InferDeps> CstQuery<'a, D> {
    /// Re-anchor the query at another node (the walk step).
    fn at(&self, node: Node<'a>) -> Self { Self { node, ..*self } }

    /// Type of `self.node` as an expression — ident, nav-chain, call, literal, if/range.
    /// Walks long dotted chains and into lambda results in one traversal.
    fn expr_type(&self) -> Resolution<ResolvedType> { … }

    /// Receiver type for a member access: outer / leaf / nullable breakdown.
    fn receiver_type(&self) -> Resolution<ReceiverType> { … }

    /// Implicit lambda scope at `self.node`'s position: `this`, `it`, named params —
    /// in-scope only. Subsumes the 3 scattered it/this/receiver entries.
    fn lambda_scope(&self) -> LambdaScope { … }

    /// Return type of a call resolved from an (optional) receiver + name.
    fn call_return_type(&self, receiver: Option<&ReceiverType>, name: &str)
        -> Resolution<ReturnType> { … }
}
```

(A thin `NodeExt`-style `node.cst(doc, deps, uri, io)` constructor keeps call sites terse; the struct —
not a node-trait method — is the home so the multi-node walk and future memoization have somewhere to live.)

### Signatures maximize agent information (the governing rule)

Read every signature from the perspective of an agent that will only see the *declaration*: it must
yield the highest possible information context without reading the body.

- **Outcome enums over `Option`.** `-> Resolution<ReceiverType>` states the full contract (resolved /
  ambiguous / absent); `-> Option<ReceiverType>` hides *why* it's `None` and invites a guess or a
  body-read. Every resolving method returns `Resolution<T>` (below), not `Option`/empty-`Vec`.
- **Named newtypes over primitives** (`ReceiverType`/`ReturnType`/`ResolvedType`, never `String`/`bool`).
- **A struct over loose params** — `CstQuery` bundles node+doc+deps+uri+io once (the methods read `self`),
  vs threading positional `(node, doc, idx, uri)` through every function.

The signature plus one hop to its named types *is* the documentation — that is what stops an agent
re-deriving a capability that already exists.

### Capability mapping (the user's five)

| Capability | Home |
|---|---|
| 1. import/package/super-aware **filter** | *Invariant* — applied internally in every method; an unfiltered result never escapes the facade (compiler-enforced, see Type-driven correctness) |
| 2. generics **substitution** | *Invariant* — returned types are already subst-applied |
| 3. optional in-scope **`it`/`this`** | `lambda_scope` → `LambdaScope { this, it, named }` |
| 4. **chain + lambda walk** (return inference) | `expr_type` / `receiver_type` (the one traversal); `call_return_type` for the call layer |
| 5. **IO / no-IO** (rg/fd bound) | `CstCtx.io: ResolveIo` — threaded into underlying name→definition lookups; hot paths pass `IndexOnly`, hover/go-to-def pass `Full` |

### Supporting I/O types

- **`CstQuery<'a, D: InferDeps>` { node, doc, deps, uri, io }** *(new — see "The `CstQuery` catalogue")* —
  bundles the resolution input AND the node, carrying the catalogue methods; the walk is `self.at(node)`.
  **This supersedes the earlier `CstResolve`-trait-on-`Indexer` + bare `CstCtx` framing** throughout this
  doc (where older sections still say "`CstResolve` trait" / "`CstCtx`", read `CstQuery`; `CstCtx`'s
  uri/doc/io fields now live on `CstQuery`). Distinct from the existing **`CursorContext`**
  (backend/cursor.rs — the resolved cursor *token*); reconcile, do not duplicate.
- **`ResolvedType`** *(new)* — a normalized `TypeName` + nullability flag (the `expr_type` result);
  **`ReceiverType`** *(exists — resolver/infer.rs, raw/qualified/outer/leaf/nullable)* **/ `ReturnType`**
  *(exists — resolver/api.rs)*. Self-documenting; bare `Option<String>` banned from the surface. The
  `RawTypeName` vs `TypeName` distinction (Type-driven correctness §5) is what these wrap.
- **`LambdaScope`** *(exists — `features/completion_context.rs`: `{ it_type, named_params, label }`,
  bare `String`)* — **promote to the catalogue and extend**: add a `this` field carrying the existing
  tri-state **`ThisLambdaCtx`** *(exists — `cst_lambda.rs`: `Resolved/Receiver/NotReceiver`; keep the
  distinction, do NOT flatten to `Option` — `Receiver` vs `NotReceiver` controls fallback)*, and upgrade
  the bare `String`s to `ReceiverType`. It subsumes `infer_lambda_param_type_at` +
  `lambda_receiver_type_from_context` + `find_it_element_type*`.
- **`CursorPos`** *(exists — types.rs)* for `lambda_scope`'s position; **`CallableInfo`** *(exists —
  infer/deps.rs)* and **`LambdaParamResolution`** *(exists — infer/lambda_resolution.rs, the stage-2
  it/this record)* reused inside the walk; **`ResolvedSymbol`** *(exists — indexer/resolution.rs, widened)*
  and **`Definitions`** *(exists — resolver/api.rs)* for symbol/definition-returning paths.

## Reuse inventory (existing types — do not reinvent)

Audited with Serena before designing the surface. Most of the vocabulary already exists; the catalogue
**reuses** it and adds only a few small newtypes.

| Type the design needs | Status | Location | Decision |
|---|---|---|---|
| `Resolution<T>` outcome enum | **new** (pattern exists) | — | Generalizes `SignatureResult`; migrate it on later |
| `SignatureResult` | exists | `indexer/infer/sig.rs:41` | Becomes `Resolution<Signature>` post-catalogue |
| `ReceiverType` | exists | `resolver/infer.rs:100` | Reuse as-is |
| `ReceiverKind` | exists | `resolver/infer.rs:88` | Reuse |
| `ReturnType` | exists | `resolver/api.rs:51` | Reuse |
| `LambdaScope` | exists | `features/completion_context.rs:15` | **Promote to catalogue + extend** (add `this`, richer types) |
| `ThisLambdaCtx` (this tri-state) | exists | `indexer/infer/cst_lambda.rs:65` | Reuse as `LambdaScope.this`; **do not flatten** |
| `LambdaParamResolution` | exists | `indexer/infer/lambda_resolution.rs:44` | Reuse as the stage-2 it/this record |
| `CallableInfo` | exists | `indexer/infer/deps.rs:21` | Reuse |
| `CursorPos` | exists | `types.rs:114` | Reuse for `lambda_scope(pos)` |
| `CursorContext` | exists | `backend/cursor.rs:16` | Distinct role (resolved token); reconcile with `CstCtx` |
| `ResolvedSymbol` | exists | `indexer/resolution.rs:13` | Reuse for symbol-returning paths |
| `Definitions` | exists | `resolver/api.rs:35` | Reuse for definition lists |
| `InferDeps` / `TestDeps` | exists | `indexer/infer/deps.rs` | The data seam — build on |
| `ResolveIo` | exists | `resolver/resolve.rs` | Reuse for `CstCtx.io` |
| `CstCtx { uri, doc, io }` | **new** | — | Thin input bundle (≠ `CursorContext`) |
| `ResolvedType` | **new** | — | normalized `TypeName` + nullability |
| `TypeName` / `RawTypeName` | **new** | — | normalized vs as-written newtypes |
| `Fqn` | **new** | — | importable-FQN newtype for `Resolution::Ambiguous` |

Net: ~13 existing types reused, **6 genuinely new** (`Resolution<T>`, `CstCtx`, `ResolvedType`,
`TypeName`, `RawTypeName`, `Fqn`) — all small. The catalogue is mostly *assembly + promotion* of
existing pieces, not new construction.

## `mod.rs` is the catalogue (structural rule)

Each resolution module's `mod.rs` contains exactly three things, in order, with **zero logic**:

1. **Input structs** — `CstCtx`, `CursorPos` — doc-commented ("what you pass in").
2. **Output structs** — `Resolution<T>`, `ResolvedType`, `ReceiverType`, `ReturnType`, `LambdaScope`
   — doc-commented *with their invariants* ("what you get back, and what's guaranteed").
3. **The catalogue trait** — `CstResolve` — intent-named methods wired to those I/O types.

The walk and helpers live in submodules (`chain`, `lambda`, `expr_type`, …), never in `mod.rs`.
Shared output types living elsewhere (`ReceiverType`/`ReturnType` in `resolver/infer.rs`) are
**re-exported from the catalogue `mod.rs`** so it reads self-contained — open one file, see the whole
I/O vocabulary + every capability. The same rule applies to each consuming **feature** `mod.rs`
(its feature-catalogue trait + that feature's I/O types).

## Type-driven correctness (compiler catches logic errors)

Model the domain so illegal states are unrepresentable:

1. **One generic outcome enum, never empty-`Vec`/`None` overloading.** "Not found" and "ambiguous
   overload" must not both collapse to empty:
   ```rust
   /// Outcome of resolving something to `T`. Reused across the catalogue so an
   /// agent learns the three outcomes once and reads them off every signature.
   enum Resolution<T> { Resolved(T), Ambiguous(Vec<Fqn>), Unresolved }
   ```
   This **generalizes the existing `SignatureResult`** (sig.rs — `Unique{..} / Overloaded / NotFound /
   UnresolvableReceiver`), which already models found/ambiguous/absent for signatures; migrate
   `SignatureResult` onto `Resolution<Signature>` once the catalogue lands. The consumer *must* match
   `Ambiguous`.
2. **Construction-sealed outputs — invariants become types.** `ReceiverType`/`ResolvedType` have
   private fields, constructible only inside the catalogue. Holding one is *proof* it was
   reachability-filtered + subst-applied — a consumer cannot fabricate an unfiltered one and feed it
   back. The "invariant" stops being a doc promise.
3. **Exhaustive CST dispatch — no silent `_ => None`.** Lower a node *once* into a thin, CST-local
   shape enum and match exhaustively:
   ```rust
   enum CstExpr<'a> { Ident(..), Nav(..), Call(..), Literal(..), Cond(..), Range(..) }
   ```
   Adding a handled node kind becomes a compile error until handled. CST-local dispatch sugar — **not**
   the rejected cross-domain IR; it never leaves the walk.
4. **Lambda scope as a sum type.** `ThisLambdaCtx`/`ThisContext` are already enums; `LambdaScope` keeps
   "no implicit receiver" as a *variant*, not an empty struct.
5. **Raw vs normalized in the type.** `RawTypeName` (generics + `?`) vs `TypeName` (normalized);
   nullability a typed field — can't pass un-normalized where normalized is required. Enforced via
   distinct types, not a `_raw` naming convention.

## Principle: the CST is the source of structure — string *parsing* is a mistake

The CST holds the program's structure *exactly*. Recovering structure from text — line scans,
`rsplit('.')`, finding a lambda brace by text position, `before_brace` / `cst_before_open_text`
substring walks, counting unclosed parens — is error-prone re-derivation of what the CST already has.
Within the CST domain **every such string-parsing mechanism is a mistake to eliminate, not an
alternative strategy.** This is the real reason `receiver.rs` (text heuristics) and `it_this.rs` (line
scans) collapse into the CST walk: not dedup for its own sake, but removing a class of imprecision.

**What is *not* a mistake — genuine heuristics, which stay:** best-effort guesses where *no precise
answer exists even with the CST*:
- **Stdlib scope-function receiver inference** — `apply`/`run`/`with`/`also`/`let` and indexed
  receiver-lambda fns, where the receiver object's type is genuinely unresolvable. This is already
  modelled by `ThisLambdaCtx::Receiver` ("known receiver context, type not found") — keep the
  classification; it is semantic knowledge, not structure recovery.
- **The string-domain unsynced/agent fallback** for navigation (no index/CST available at all).

The discriminator for every line of the old engines: *is it recovering structure the CST already has
(→ delete, drive from the CST node) or guessing semantics the CST cannot give (→ keep as a heuristic,
re-homed onto CST-derived inputs, never fed by text scanning)?* Heuristic **content** is preserved; the
**text-scanning mechanism feeding it** is not.

## Internal consolidation plan (four walks → one)

The one walk lives in `indexer/infer`, CST-driven, dispatched by the `CstExpr` shape. Each former
mechanism becomes a *phase*, not a parallel path:

| Former mechanism | Folds into | Deleted |
|---|---|---|
| `cst_lambda.rs` (CST `ThisLambdaCtx`) | the **spine** — CST traversal + lambda-context classifier | standalone entry points |
| `chain.rs` (nav-segment walk) | the **chain-segment step** of `expr_type` | duplicate chain logic in `receiver.rs` (`method_chain_lambda_type`, `chain_with_type_subst`) |
| `receiver.rs` (text heuristics) | the **lambda-receiver step**, CST-driven | the text-scan variants (`receiver_dot_lambda_type`, `nested_receiver_lambda_type`, `has_unclosed_paren`, …) |
| `it_this.rs` (line scans) | the **`it`/`this`** resolution feeding `lambda_scope` | the line-scan variants + back-scan constants |
| `semantic_tokens::expression_type` family | calls `CstResolve::expr_type` | the bespoke walk + its string-engine `use` leakage |

### Sequencing (each step green via `cargo test --bin kmp-lsp` before the next; move-don't-rewrite)

1. **Stand up the catalogue** — `CstResolve` + I/O structs + `CstCtx` + `Resolution<T>` in
   `infer/mod.rs`; methods are thin delegations to *today's* functions. Behaviour-identical, no deletion.
2. **Route consumers** onto the catalogue (semantic_tokens, completion_context, inlay, diagnostics,
   hover, sig-help, fill_when); **delete `semantic_tokens`'s bespoke walk** + its string-engine leakage.
3. **Collapse the lambda triad** — `lambda_scope` becomes the one CST classifier; fold `receiver.rs`
   text variants + `it_this.rs` line variants in. Per the principle above: **re-home the genuine
   heuristics** (scope-function receiver classification → `ThisLambdaCtx::Receiver`) onto CST-derived
   inputs, and **delete the text/line-scanning mechanisms** that recovered structure.
4. **Collapse the chain walk** — `chain.rs` becomes the chain step of `expr_type`; delete the echoed
   chain logic in `receiver.rs`.
5. **Sweep** — remove dead helpers/constants; introduce the `CstExpr` exhaustive dispatch + the
   construction-sealed outcome/types; confirm `mod.rs` is the only public face.
6. **CST-aware navigation** (post-catalogue) — generalize the `CursorContext` → CST bridge so the
   symbol-identity family (go-to-def, goto-impl, find-refs, document-highlight, rename) resolves via
   `CstResolve`, string + rg as fallback; one slice per feature (see the dedicated section). Depends on 1–5.

Deletion lands mostly in steps 2–4 (the bespoke walk + text/line lambda engines + echoed chain logic):
on the order of a thousand-plus lines, with the three-mechanism divergence closed.

## Per-feature catalogues

Each consuming feature's `mod.rs` exposes its own small catalogue trait (e.g. `SemanticClassification`,
`InlayInference`, `HoverFacts`) listing what that feature *produces*, as a thin projection of
`CstResolve`, with that feature's own I/O types beside it. An agent opens the feature `mod.rs`, sees its
catalogue delegating to `CstResolve`, and has no reason to hand-roll inference.

## CST-aware navigation (layered; post-catalogue phase)

The **symbol-identity-at-cursor family** — go-to-def, goto-implementation, find-references,
document-highlight, rename — all answer "which symbol does this identifier refer to?" **name-based**
today (string + rg + line scans), which is why they're noisy. Only the narrow `it`/`this`/named-param
case touches CST, via the existing `CursorContext` bridge
(`infer_receiver_type(Contextual)` → `infer_variable_type_from_cst`). They miss the unified benefits —
chain/lambda walk, generics subst, receiver-typed member/overload accuracy — for general member access
(`someVar.field.method`, dotted chains, generics). These are the "forgotten orphans."

**Refinement:** generalize the `CursorContext` → CST bridge through the catalogue so the whole family is
**CST-first with string fallback**. The CST role differs by feature:

- **go-to-def / goto-impl** (resolve target): `CstResolve::receiver_type`/`expr_type` → map type →
  definition via `resolve_member` (`Definitions`), then string `find_definition_qualified` + rg fallback.
  ```
  1. CstResolve::receiver_type / expr_type (CST, when synced)  → resolve_member → Definitions
  2. fall back to string find_definition_qualified + rg        (unsynced / agent / cold start)
  ```
- **find-refs / document-highlight / rename** (identify + filter): CST resolves the *target's* declaring
  type / FQN at the cursor, then **filters** the rg/name candidate set to references whose receiver type
  matches. This is the principled replacement for the manual **findReferences noise-mitigation**
  heuristics (qualified-pattern rg / package scoping) — type resolution instead of string tricks.

Shared properties:
- **Seam:** `CursorContext.contextual` is already a `ReceiverType` from CST; widen it to resolve *any*
  qualifier via `CstResolve`, not just `it`/`this`. Reuses `resolve_member`/`Definitions` — minimal new
  code, large accuracy gain.
- **Invariant preserved:** the string + rg path stays as the fallback, so the family still works with no
  CST (the unsynced/agent capability the string domain exists for).
- **IO:** navigation is not a keystroke hot path — `CstCtx.io = Full` is fine.
- **Out of family:** document-symbol (lists `FileData.symbols`) and workspace-symbols (fuzzy name scan)
  are pure listing/fuzzy — no symbol-identity resolution, left as-is.

This phase **depends on the catalogue + consolidation landing first** (it consumes `CstResolve`), so it
sequences after step 5. Each feature is its own slice (uniform pattern, per-feature behaviour change
guarded by the existing nav/refs/rename suites).

## Migration & branch strategy

Per-slice PRs onto `refactor/cst-resolution` (off the merged `refactor/unified-resolution`), one per
sequencing step, each green before the next (bisectable; move-don't-rewrite). Steps 1–2 are low-risk and
land first; steps 3–4 carry the deletion and the risk. Open PRs only when asked.

## Testing & verification

- The existing CST suites are the behaviour net: hover / inlay / semantic-tokens / completion /
  `fill_when` / diagnostics must stay green through every deletion. `cargo test --bin kmp-lsp`
  (binary-only crate; `--lib` runs 0 tests). Focused loops: `-- semantic`, `-- inlay`, `-- completion`,
  `-- fill_when`, `-- nullable_call`.
- `TestDeps` drives `CstResolve` unit tests directly (no `Indexer`).
- Where a consolidation step changes a path, add a decoy regression test *first* (red→green); diff
  semantic-tokens / inlay output before vs after each deletion.
- `find_referencing_symbols` (Serena) to enumerate every consumer before routing/deleting — the
  anti-reinvention completeness check.

## Risks

- **`cst_lambda.rs` is 892 lines of the most correctness-sensitive code.** Mitigation: it becomes the
  spine (kept, not rewritten); the text/line engines fold *into* it incrementally, each step suite-gated.
- **Hot-path latency** (semantic tokens / diagnostics on every keystroke). Mitigation: `CstCtx.io =
  IndexOnly` on those paths bounds rg/fd; behaviour-preserving for `Full`.
- **Behaviour drift during consolidation.** Mitigation: move-don't-rewrite, per-step suite green, output
  diffs, decoy tests for changed paths.

## Out of scope

- The **string path** and any unification of its heuristics (separate later analysis).
- A shared cross-domain **IR**.
