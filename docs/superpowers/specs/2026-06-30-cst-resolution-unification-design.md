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

**Non-goals**
- Touching the **string path** (heuristic by design; separate later analysis).
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
- **WHY** — infer functions are pure reads over a snapshot; consumers must not re-derive resolution.
- **WHAT** — a catalogue trait over `InferDeps`; self-documenting newtypes; one walk.
- **HOW** — `trait CstResolve` implemented for `D: InferDeps` (static dispatch, generics over `dyn`).

## The `CstResolve` catalogue (the trait surface)

Implemented for any `D: InferDeps` (so `TestDeps` drives it without an `Indexer`). CST input
(node/pos/doc) is passed in; data access via `self`; cross-cutting context bundled into one newtype.

```rust
/// The single catalogue of CST-driven type resolution for LSP + diagnostics.
/// Pure reads over `InferDeps`. Every returned type is ALREADY reachability-
/// filtered and generic-substituted — a consumer never re-derives those.
pub(crate) trait CstResolve {
    /// Type of any expression node — ident, nav-chain, call, literal, if/range.
    /// Walks long dotted chains and into lambda results in one traversal.
    fn expr_type(&self, node: Node, ctx: &CstCtx) -> Resolution<ResolvedType>;

    /// Receiver type for a member access: outer / leaf / nullable breakdown.
    fn receiver_type(&self, node: Node, ctx: &CstCtx) -> Resolution<ReceiverType>;

    /// Implicit lambda scope at a cursor: `this`, `it`, named params — in-scope only.
    /// Subsumes the 3 scattered it/this/receiver entries.
    fn lambda_scope(&self, pos: CursorPos, ctx: &CstCtx) -> LambdaScope;

    /// Return type of a call resolved from an (optional) receiver + name.
    fn call_return_type(&self, receiver: Option<&ReceiverType>, name: &str, ctx: &CstCtx)
        -> Resolution<ReturnType>;
}
```

### Signatures maximize agent information (the governing rule)

Read every signature from the perspective of an agent that will only see the *declaration*: it must
yield the highest possible information context without reading the body.

- **Outcome enums over `Option`.** `-> Resolution<ReceiverType>` states the full contract (resolved /
  ambiguous / absent); `-> Option<ReceiverType>` hides *why* it's `None` and invites a guess or a
  body-read. Every resolving method returns `Resolution<T>` (below), not `Option`/empty-`Vec`.
- **Named newtypes over primitives** (`ReceiverType`/`ReturnType`/`ResolvedType`, never `String`/`bool`).
- **A named context struct over loose params** (`ctx: &CstCtx` documents uri+doc+IO and their
  invariants in one hop, vs positional `(uri, doc, io)`).

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

- **`CstCtx { uri, doc, io: ResolveIo }`** *(new)* — per-request resolution *input* (document + IO
  policy; keeps signatures short). Distinct from the existing **`CursorContext`** (backend/cursor.rs —
  the resolved cursor *token*: word/qualifier/contextual/lambda_decl); reconcile, do not duplicate.
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
   text variants + `it_this.rs` line variants in; delete both sets.
4. **Collapse the chain walk** — `chain.rs` becomes the chain step of `expr_type`; delete the echoed
   chain logic in `receiver.rs`.
5. **Sweep** — remove dead helpers/constants; introduce the `CstExpr` exhaustive dispatch + the
   construction-sealed outcome/types; confirm `mod.rs` is the only public face.

Deletion lands mostly in steps 2–4 (the bespoke walk + text/line lambda engines + echoed chain logic):
on the order of a thousand-plus lines, with the three-mechanism divergence closed.

## Per-feature catalogues

Each consuming feature's `mod.rs` exposes its own small catalogue trait (e.g. `SemanticClassification`,
`InlayInference`, `HoverFacts`) listing what that feature *produces*, as a thin projection of
`CstResolve`, with that feature's own I/O types beside it. An agent opens the feature `mod.rs`, sees its
catalogue delegating to `CstResolve`, and has no reason to hand-roll inference.

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
