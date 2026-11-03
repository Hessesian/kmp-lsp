# Structured `ResolvedType` — Design (finishing "step 5: Sweep")

**Status:** draft for maintainer review, 2026-11-03. Docs only; nothing here is implemented.
**Finishes:** step 5 of `docs/superpowers/specs/2026-06-30-cst-resolution-unification-design.md`
(the `RawTypeName`/`TypeName` split, construction-sealed outputs, exhaustive `CstExpr` dispatch), which
`docs/superpowers/plans/2026-08-02-cst-catalogue-gap-closure.md` deferred and confirmed unstarted.
**Read against:** the open stack `stack/1-determinism-binding-outcome` →
`stack/2-binding-type-coverage` → `stack/3-receiver-and-iterable-typing` (PRs #337/#338/#339). The stack
is being amended, so this doc cites files and symbols, not line numbers. Claims marked *(unverified)*
were not checked in code.

## Problem

Inside the CST domain a Kotlin type is a `String`. `ResolvedType` (`src/indexer/infer/mod.rs`) wraps
`type_name: String`, a `nullable` flag derived from that string, and a `declaring_uri: Url`; internally
`expr_type.rs` and `chain.rs` pass `(String, Url)` pairs (36 occurrences on the stack/3 tip). Every
type operation (nullability, type-argument reading, substitution, identity, function-type recognition)
is re-derived from spelling at each use site.

### Evidence: 38 Copilot review comments on #337/#338/#339

All 38 comments (two review rounds, 2026-11-02 and 2026-11-03) were fetched read-only and classified
against the hypothesis below. Comment IDs are abbreviated to their last four digits (`#338·5912` is
comment 4445365912 on PR #338).

| Class (hypothesis) | Count | Comments |
|---|---|---|
| 1. Type identity compared by spelling, not declaration | 10 | `#337·1982` resolver/infer.rs; `#338·5912`, `#338·5946`, `#338·6084`, `#338·3087`, `#338·3128` expr_type.rs; `#338·2949` indexer.rs; `#339·9202`, `#339·8120` cst_lambda.rs; `#339·9577` parser.rs |
| 2. Nullability by string surgery | 6 | `#338·6223`, `#338·3007`, `#338·3157` expr_type.rs; `#339·9377`, `#339·8218`, `#339·8245` expr_type.rs |
| 3. Type-parameter binding/erasure on strings | 7 | `#338·6037` expr_type.rs; `#339·9156`, `#339·8034` chain.rs; `#339·9261`, `#339·9320`, `#339·8082` cst_lambda.rs; `#339·8335` parser.rs |
| 4. Declaration selected separately from its signature/metadata | 6 | `#337·1816` indexer.rs; `#337·7007` sig.rs; `#338·6127`, `#338·6179` expr_type.rs; `#339·9091` chain.rs; `#339·8293` sig.rs |
| 5. Other | 9 | `#337·1711` fill_when.rs; `#337·1913`, `#337·7063` sig.rs; `#338·5829` indexer.rs; `#338·5997`, `#338·3056` expr_type.rs; `#339·9449` scope.rs; `#339·9526` parser.rs; `#339·8173` expr_type.rs |

**Where the hypothesis is wrong or incomplete:**

- **Class 1 is two problems.** Only 3 of 10 compare spellings (`5912`, `2949`, `8120`). Five *discard
  or replace* the origin a type was written in (`1982` drops `super_uri`; `6084`, `3128`, `9202`
  re-anchor to the caller URI; `9577` truncates `actual.Widget` to `Widget`). Two are *callable*
  identity, not type identity: `error`/`TODO` (`5946`) and `mutableStateOf`/`lazy` (`3087`) are
  recognised by callee name. A type that carries its identity in its head fixes the first eight; the
  callee pair needs class 4's fix.
- **Class 4 is not a type-representation problem.** Structured types alone do not fix it. It needs one
  lookup returning the selected declaration *with* its signature (`SelectedCallable`, below). It is in
  scope because every class-3 fix depends on knowing which declaration owns a type parameter.
- **Class 5 hides a missing class: shape recognised by spelling.** `7063` (`is_lambda_shaped` rejects
  `Child.() -> Unit`), `8173` (iterable recognition rejects `kotlin.collections.List<Apple>` and
  `Array<out Apple>`) and `5829` (builtin names looked up in a hand-sorted list) all disappear with a
  structured head. `3056` and `1711` are the same bug twice: `binding_states_its_own_type` matches only
  `Annotated | Initialized`, so new `LocalBinding` variants silently fall to lambda inference. That is
  exactly the non-exhaustive dispatch step 5 targets. The remaining four are syntax-extraction or
  call-selection policy, which this design does not fix: `1913` (overload fall-through to inherited
  extensions), `5997` (named `value =` argument), `9449` (`this@Outer` label), `9526` (receiver walk past
  locals).
- **Class 3 includes missing rules, not just string erasure.** `6037` (explicit `lazy<Any>` type
  argument ignored) and `9261` (no supertype projection) are absent rules. The structured version makes
  the conservative answer (decline → Unknown) the default rather than a per-site remembering.

Net: 34 of 38 comments are fixed by the mechanism in this design (per-slice mapping in
[Regression suite](#regression-suite)); 4 are not.

### Code observations (verified by reading the stack/3 tip, not by running tests)

- `StrExt::strip_nullable` is `trim_end_matches('?')`, and `is_nullable` is `ends_with('?')`. On the
  function type `(Int) -> String?` (a non-null function returning a nullable `String`) both read the
  *return type's* `?` as the value's nullability. So `holder?.value` with that member type is not made
  nullable by `infer_navigation_expr_type`, and stripping removes the wrong `?`.
- `infer_as_expr_type` parenthesises a function type before appending `?` (fixing an earlier Copilot
  finding). Its test is `starts_with('(')`, so `suspend (A) -> B` and `T.() -> Unit` are still
  misrendered. `infer_navigation_expr_type` was not given the same fix (`#339·8218`): the same finding,
  fixed at one site, recurred at a sibling.
- `type_subst::is_generic_param` classifies "short all-uppercase identifier" as a type parameter. It is
  still the fallback in `cst_lambda::classify_extracted_type` and `substitute_from_receiver`, and in
  `chain.rs`. This is the `is_generic(&str)` sniff AGENTS.md names as the anti-pattern.
- `ExtensionReceiver::is_type_parameter` compares the receiver's *leaf* `base` against the declaration's
  type parameters (`#339·8335`, possibly being amended).
- `TypeTextForm { BareName, AsWritten }` exists only because an identifier's type must be reported
  either as a lookup key or as written. It is one string serving two roles.
- `chain::StrategyOutcome::finalize` looks up the callee's type parameters *again* by name
  (`unbound_type_parameters` → `find_fun_callable_info` / `find_member_type_params` /
  `find_class_type_params(leaf)`) after a strategy already chose the declaration that supplied
  `raw_return`. This is class 4 in its structural form.

## Goals / non-goals

**Goals**
1. One structured type value for the CST domain: identity, type arguments, nullability, function types
   and type parameters are fields, and every operation on them is a method in one module.
2. Type identity is the *declaration*, fixed when the type is parsed in its declaring scope. A consumer
   cannot re-anchor it to the caller.
3. Pure typing rules (elvis, for-element, substitution, agreement) take resolved sub-types and return a
   type with no index access. Name resolution happens once, at parse-in.
4. Unknown is a value that propagates conservatively. "Cannot tell" never becomes "same" or "the first
   candidate".
5. Finish step 5: exhaustive `CstExpr` dispatch, construction-sealed `ResolvedType`, deletion of the
   string helpers it replaces, and a ratchet that keeps them deleted.

**Non-goals**
- The string-domain engine (`resolver/infer_lines.rs`, `rg.rs`, `ReceiverType` users). This follows the
  2026-06-30 design's non-goal; those files stay string-based and are only frozen by the ratchet.
- Common-supertype (least upper bound) computation, supertype projection of generic arguments, and
  bounds checking. Each is explicitly unsupported and yields Unknown.
- Changing the on-disk index or cache format (see [Open questions](#open-questions)).
- The four comments named above as syntax or selection policy.

## The structured type

Lives in `src/indexer/infer/kotlin_type.rs` and is re-exported from `mod.rs` (the catalogue rule).

```rust
/// A Kotlin type as the CST engine knows it. Fields are private: a value is
/// only built by parsing a type in the scope that wrote it, or by the algebra
/// below — never from a caller-supplied string.
pub(crate) struct ResolvedType {
    head: TypeHead,
    arguments: Vec<TypeArgument>,
    nullability: Nullability,
}

pub(crate) enum TypeHead {
    /// A class, interface, object or typealias target, by declaration identity.
    Class(ClassIdentity),
    /// A written name the index could not resolve. Member lookup may still fall
    /// back by name (today's recall), but it never compares equal to anything.
    Unresolved { written: TypePath, scope: Url },
    /// A type parameter, owned by exactly one declaration.
    TypeParameter(TypeParameter),
    /// `(A, B) -> R`, `suspend R.(A) -> B`.
    Function(Box<FunctionType>),
    /// Inference gave up: no common supertype, conflicting bindings, an `in`/`*`
    /// argument read as a value, an unbound type parameter after a call.
    Unknown,
}

pub(crate) struct ClassIdentity(QualifiedName);       // `kotlin.collections.List`, `app.ui.Screen.State`

pub(crate) struct TypeParameter { name: Name, owner: TypeParameterOwner }
pub(crate) enum TypeParameterOwner { Class(ClassIdentity), Callable(CallableIdentity) }
pub(crate) struct CallableIdentity(Location);         // overloads share a name; the declaration site does not

pub(crate) struct FunctionType {
    receiver: Option<ResolvedType>,
    parameters: Vec<ResolvedType>,
    returns: ResolvedType,
    suspension: Suspension,                             // Suspend | Regular
}

pub(crate) enum TypeArgument { Invariant(ResolvedType), Out(ResolvedType), In(ResolvedType), Star }
pub(crate) enum Nullability { NonNull, Nullable }
pub(crate) enum TypeEquality { Same, Different, Undetermined }
```

### Decisions

**Identity is the qualified name, resolved at parse-in.** Alternatives considered:

| Option | For | Against |
|---|---|---|
| **`QualifiedName` (chosen)** | It is Kotlin's own type identity. `expect`/`actual` pairs and the same class in two JARs are one type. Comparable without the index, so the algebra stays pure. | Needs package + nesting path from a resolved `Location` *(unverified: whether the index exposes a file's package cheaply at every resolution site)*. |
| Declaration `Location` | Already what `resolve_type_path_declarations` returns. | `expect`/`actual` and duplicate JARs compare unequal, which is wrong in a KMP server. |
| Written path + scope, resolved lazily (today's `declaring_uri`, sealed) | Smallest step from today; no up-front resolution cost. | Equality needs the index, so typing rules cannot be pure. Nested arguments each need their own scope anyway. |

Callables use `Location` because overloads share a qualified name. Type-parameter ownership must tell
`Holder<Item>`'s `Item` from a top-level extension's (`#339·8034`) and a function's `A` from
`model.A` (`#339·9156`).

**`Unresolved` vs `Unknown` are different facts.** `Unresolved` means "the user wrote `Foo` and we could
not find it". It is kept so hover can show it and member lookup can fall back by name as it does today,
which protects benchmark recall. `Unknown` means "inference has no answer". Both are `Undetermined`
under equality.

**`Star` is not `Unknown`.** `List<*>` is a real, known type and `List<*>` equals `List<*>`. Today
`star_unbound_type_arguments` writes `*` for "we did not bind this parameter", which conflates a
projection with ignorance. Unbound parameters become `Unknown` arguments instead.

**Function types are a dedicated head, not `kotlin.FunctionN`.** Lambda inference (`cst_lambda.rs`)
needs the receiver/parameters/return split directly, and the nullable-function display (`((A) -> B)?`)
falls out of the structure: `nullability` belongs to the outer value, `returns.nullability` to the
return. Parse-in should normalise `FunctionN<…>` spellings to this head if the sidecar emits them
*(unverified: what spelling JAR signatures use)*.

**Nullability is two-valued.** Java platform types and definitely-non-null `T & Any` are not modelled.
`T & Any` parses to `Unknown`; platform types are an open question.

### Where strings legitimately remain

| Boundary | Direction | Mechanism |
|---|---|---|
| CST type nodes (`user_type`, `nullable_type`, `function_type`, type arguments/projections) | in | `ResolvedType::from_syntax(node, bytes, &TypeScope, &impl NameResolver)`: structural lowering, no text parsing (honours the 2026-06-30 "CST is the source of structure" principle) |
| Declaration text from the index or sidecar (`SymbolEntry` detail/return text, JAR signatures) | in | `ResolvedType::from_declaration_text(text, &TypeScope, &impl NameResolver)`: the **only** type-string parser in the crate, private to `kotlin_type.rs` |
| Hover, inlay, completion detail | out | `impl Display` (short names; parenthesises a nullable function type), plus `display_qualified()` |
| Index-key lookups not yet migrated (`find_field_type(name, …)` etc.) | out | `TypePath` accessors on `ClassIdentity` / `Unresolved`, shrinking per slice |

`TypeScope { file: &Url, type_parameters: &[TypeParameter] }` is "where was this type written": the
declaring file's imports and package, plus the enclosing class's and function's type parameters. A name
in `type_parameters` lowers to `TypeHead::TypeParameter`; a qualified path never does.

## The type algebra

Methods on `ResolvedType`. All are pure: no `InferDeps`, no `Url` arguments.

| Method | Semantics | Replaces |
|---|---|---|
| `without_null()` / `nullable()` | Set the outer nullability. A function type gets parentheses only at display. | `strip_nullable`, `format!("{..}?")`, `trim_matches('?')`, the `starts_with('(')` paren hack |
| `is_nullable()` | Outer nullability only, never a nested return or argument. | `StrExt::is_nullable` on type text |
| `same_type(&other) -> TypeEquality` | `Same` iff heads are equal `Class`/`TypeParameter`/`Function` identities and arguments are pairwise `Same` (`Star` = `Star`). Any `Unresolved`/`Unknown` → `Undetermined`. | `same_type_in_scopes`, `names_type_in_scope`, `==` on text, `KOTLIN_BUILTIN_TYPES` membership |
| `join(&other)` | Least upper bound *only* where it is trivial: `Nothing` is bottom; `Same` types yield that type with the wider nullability. Otherwise `Unknown`. **Common supertype is explicitly unsupported.** | Elvis, `if`, and argument-agreement logic in `expr_type.rs` / `cst_lambda.rs` |
| `substitute(&Substitution)` | Replace `TypeParameter` heads. `T?` with `T := Apple?` → `Apple?`. `T?` with `T := (A) -> B` → nullable function. Qualified paths are never touched because they are not parameters. | `apply_type_subst`, `apply_simple_subst`, `star_unbound_type_arguments` |
| `Bindings::bind(declared, actual)` | Unify by structure. Different class heads → `Declined` (no supertype projection). A second, different binding for one parameter → `Conflicting`. Both read as `Unknown` when closed. | `build_ext_fn_type_subst`, `match_type_args_recursive`, `same_type_constructors`, `build_type_arg_subst`, `build_fn_subst` |
| `close_unbound(owner)` | Each parameter of `owner` still unbound → `Unknown` argument. | `unbound_type_parameters` + `star_unbound_type_arguments` |
| `read_argument(index)` | Value read through an argument: `Invariant`/`Out` → the type; `In`/`Star` → `Unknown`. | `readable_type_argument`, `first_type_arg_raw`, `type_args_inner` |
| `function()` / `head()` | Accessors for shape tests. | `is_lambda_shaped`, `lambda_type_receiver`, `is_generic_param` |

`Substitution` is keyed by `TypeParameter` (name **and** owner), so a class's `T` and a function's `T`
cannot be confused.

## Layering

```
1. Syntax extraction   CST node → shape enum (CstExpr, LocalBinding, call arguments).  No index, no types.
2. Typing rules        pure fns over ResolvedType → ResolvedType.                      No index, no nodes.
3. Resolution          CstQuery: lower (1), resolve sub-expressions and names → (2).   The only layer with InferDeps.
```

Layer 1 already exists in parts: `LocalBinding` is syntax-only today, which is the model to follow.
Layer 3 is `CstQuery` plus the `NameResolver` used at parse-in. Feature code still only asks `CstQuery`
(AGENTS "features ask, they don't dig"). It receives `ResolvedType` values and may display them or call
algebra methods, but cannot construct one from a string.

### Worked example: `left ?: right`

**Today** (`infer_elvis_type`): infer `left` as written text, `strip_nullable()` it, special-case
`error`/`TODO` by callee name (`is_nothing_typed`), compare the right side's text with `==`, then ask
`same_type_in_scopes(text, left_uri, right_uri)`. A parenthesised right falls into the `BareName` form
and loses `?` (`#338·6223`). A call right loses its return `?` inside `extract_return_type_from_detail`
(`#338·3007`).

**Layer 1: syntax.** Lowering produces the shape; no index access:

```rust
pub(crate) enum CstExpr<'tree> {
    Literal(LiteralKind),
    Identifier(Node<'tree>),
    This { label: Option<Node<'tree>> },
    Navigation { receiver: Node<'tree>, member: Node<'tree>, access: MemberAccess }, // Dot | SafeCall
    Call(Node<'tree>),
    Elvis { left: Node<'tree>, right: Node<'tree> },
    Parenthesized(Node<'tree>),
    Cast { operand: Node<'tree>, target: Node<'tree>, kind: CastKind },             // As | SafeAs
    Jump,                                                                            // return/throw/break/continue
    Conditional(Node<'tree>), Range(Node<'tree>), Arithmetic(Node<'tree>),
    Prefix(Node<'tree>), BooleanOperator,
    NotModelled,
}
pub(crate) fn lower(node: Node<'_>) -> CstExpr<'_>;
```

**Layer 3: resolution.** `CstQuery` resolves the operands. The right side's call type comes from the
selected callable's declared return, parsed in *that declaration's* scope. A user's
`fun error(): Banana` therefore yields `app.Banana`, and the stdlib's yields `kotlin.Nothing`. No
callee-name check exists anywhere.

```rust
fn operand_type(&self, node: Node<'_>) -> ResolvedType {
    match lower(node) {
        CstExpr::Elvis { left, right } => typing::elvis(self.operand_type(left), self.operand_type(right)),
        CstExpr::Parenthesized(inner) => self.operand_type(inner),     // type passes through unchanged
        CstExpr::Jump => ResolvedType::nothing(),
        CstExpr::Call(call) => self.selected_callable(call).return_type_at(call),
        // … every variant handled; NotModelled => ResolvedType::unknown()
    }
}
```

**Layer 2: rule.** Kotlin types `a ?: b` as the least upper bound of `a`'s non-null type and `b`'s type:

```rust
/// `left ?: right`. `join` treats `Nothing` as bottom, so `x ?: return` is
/// `x`'s non-null type and `x ?: null` (`Nothing?`) is `x`'s type made nullable.
pub(crate) fn elvis(left: ResolvedType, right: ResolvedType) -> ResolvedType {
    left.without_null().join(&right)
}
```

Comments this one rule settles: `#338·5912` and `#338·2949` (different `ClassIdentity` → `Unknown`),
`#338·5946` (shadowing `error` returns `Banana`; join with `Apple` → `Unknown`), `#338·3007` (nullable
right → nullable join), `#338·6223` (parenthesised operand keeps its type), `#338·3157` (`State<Apple?>`
`getValue` substitutes `T := Apple?` with its `?` intact).

## Interaction with existing types

| Type (stack/3) | After |
|---|---|
| `LocalBinding<'tree>` (`cst_symbol.rs`) | Unchanged: it is already layer 1 (nodes only). Its consumers match it exhaustively, with no `matches!` subset (`#338·3056`, `#337·1711`). |
| `BindingType` (`expr_type.rs`) | `Typed(ResolvedType) \| LocalUnknown \| NotLocal`. `type_text` + `declaring_uri` collapse into the type. A top-level `Unknown` is normalised to `LocalUnknown` by its constructor, so there is only one way to say "local, untyped". |
| `Resolution<T>` (`mod.rs`) | Unchanged. `CstQuery::expr_type` returns `Resolution<ResolvedType>`. A top-level `Unknown` head becomes `Unresolved` at that boundary; `Unknown` survives only nested (`List<Unknown>` still serves `.size`). `Ambiguous` stays reserved for callable selection. |
| `ResolvedType::{from_inferred, as_type_str, type_path, resolution_scope, declaring_uri}` | Deleted. The scope lives in each head. |
| `TypeTextForm` | Deleted: callers ask `head()` or display. |
| `ExtensionReceiver { base, written, is_type_parameter }` (`parser.rs`) | `receiver: ResolvedType` parsed from the `receiver_type` node with the declaration's type parameters in `TypeScope`; "is a type parameter" becomes a head match. |
| `CallableInfo`, `ExtensionCallable`, `find_fun_callable_info`, `find_member_type_params`, `find_extension_callable` | Replaced by one `InferDeps::select_callable(name, CallSite) -> Resolution<SelectedCallable>` returning `{ identity, type_parameters, receiver, parameters, return_type }` from **one** declaration. Member precedence and call-shape choice happen once. |
| `find_class_type_params(class_name: &str)` | `class_type_parameters(&ClassIdentity)`; no leaf-name lookup. |
| `ReceiverType` (`resolver/infer.rs`) | Untouched (string engine). No bridge is built until a consumer needs one. |

### Unknown propagation

| Operation | Rule |
|---|---|
| `same_type` with `Unknown` or `Unresolved` anywhere | `Undetermined`, never `Same` |
| `join` with `Unknown` | `Unknown` |
| `substitute` where the binding is `Unknown` | that position becomes `Unknown`; the outer head is kept |
| `Bindings` conflict or `Declined` | the parameter closes to `Unknown` |
| member lookup on `Unknown` | `Unresolved` |
| member lookup on `Unresolved { written, scope }` | today's by-name fallback; the result is parsed in the member declaration's own scope |
| `read_argument` through `In`/`Star` | `Unknown` |
| top level at `CstQuery` / `BindingType` | `Resolution::Unresolved` / `BindingType::LocalUnknown` |
| display | top-level: no hint; nested: see open question 7 |

## Departures from the 2026-06-30 design

1. **`ResolvedType` is not "normalized `TypeName` + nullable flag", and there is no
   `RawTypeName`/`TypeName` pair.** Two string newtypes would stop un-normalized text being passed where
   normalized is required. That is the only guarantee they give, and it fixes none of the 38 comments
   except arguably part of class 2. "Raw" survives only as syntax (a CST node or declaration text) before
   parse-in; the step-5 intent ("can't pass un-normalized where normalized is required") holds more
   strongly because there is no normalized *string* at all.
2. **One string parser is kept** (`from_declaration_text`), against the principle that string parsing is
   a mistake. JAR signatures and cached declaration text have no CST at query time. It is one function in
   one module, behind the ratchet. Where a tree is available (open files, and the unopened-file parse
   memo from commit 059badee), `from_syntax` is preferred; see open question 10.
3. **This is not the rejected cross-domain IR.** `ResolvedType` is a *type value*, not an expression
   representation. The string engine does not lower into it, and nothing outside `indexer/infer`
   produces one.
4. **Sealing proves a different invariant.** The 2026-06-30 doc sealed "reachability-filtered +
   substitution-applied". Here, holding a `ResolvedType` proves "parsed in the scope that wrote it".
   That is the invariant whose absence produced class 1.
5. **`CstExpr` has more variants** than the 2026-06-30 sketch (`Ident, Nav, Call, Literal, Cond,
   Range`), because `infer_expr_type_at_depth` already handles elvis, casts, prefix, parenthesised,
   `this`, arithmetic and boolean operators. Tree-sitter kinds are strings, so `lower` itself ends in
   `_ => NotModelled`. Exhaustiveness is enforced on every *consumer* of `CstExpr`.
6. **`Resolution::Ambiguous` carries no `Vec<Fqn>`** on the stack (the doc proposed one). Not changed here.

## Migration plan

Stacked slices, each one PR of roughly 1000 changed lines *(estimates, not measured)*. Each must be
green (`cargo test`, `cargo clippy -- -D warnings`) and benchmarked with `kmp-lsp resolution-accuracy`
on the Moneta corpus before the next starts. The slices **start after #337–#339 merge**: their point
fixes and tests are the baseline this design replaces mechanically.

The order follows dependency first, then comments killed per slice. Identity (the largest class) needs
the representation to exist; substitution needs owner identities; selection needs both.

| Slice | Scope | Kills (regression tests) | Main files |
|---|---|---|---|
| S1 Representation + nullability + shape | `kotlin_type.rs` (type, parse-in from CST and text, display, `without_null`/`nullable`/`read_argument`/`head`). Heads are `Unresolved`/`TypeParameter`/`Function`/`Unknown` only; no `Class` resolution yet. `expr_type.rs` internals move from `(String, Url)` to `ResolvedType`; `ExtensionReceiver` → `ResolvedType`. Bridge `Display` for unmigrated `chain.rs`. | `#338·6223`, `#339·9377`, `#339·8218`, `#337·7063`, `#339·8173`, `#339·8335`, `#339·9577` (7) | new `kotlin_type.rs`, `expr_type.rs`, `mod.rs`, `parser.rs`, `sig.rs` (`is_lambda_shaped`) |
| S2 Identity | `NameResolver` + `ClassIdentity` resolution at parse-in, `same_type`, `join`. Elvis/`if`/agreement on identity. Delete the `(String, Url)` origin threading. | `#338·5912`, `#338·2949`, `#337·1982`, `#338·6084`, `#338·3128`, `#339·9202`, `#339·8120`, `#338·5829` (8) | `kotlin_type.rs`, `expr_type.rs`, `chain.rs`, `cst_lambda.rs`, `indexer.rs`, `deps.rs` |
| S3 Substitution | `TypeParameter` owners, `Substitution`, `Bindings` (conflict-aware, `Declined` on constructor mismatch), explicit call-site type arguments, `close_unbound`. | `#338·6037`, `#339·9156`, `#339·9261`, `#339·9320`, `#339·8034`, `#339·8082`, `#338·3157`, `#339·8245` (8) | `type_subst.rs` (mostly deleted), `chain.rs`, `cst_lambda.rs`, `lookup.rs` |
| S4 Selected callable | `select_callable → Resolution<SelectedCallable>`. `StrategyOutcome` carries it; call return types come from it; delegate shortcuts gated on `kotlin`/`androidx.compose.runtime` identity; no `error`/`TODO` name check. | `#337·1816`, `#337·7007`, `#338·6127`, `#338·6179`, `#339·9091`, `#339·8293`, `#338·3007`, `#338·5946`, `#338·3087` (9) | `deps.rs`, `indexer.rs`, `sig.rs`, `chain.rs`, `expr_type.rs`, `cst_lambda.rs` |
| S5 Sweep | `CstExpr` exhaustive dispatch; exhaustive `LocalBinding` consumers; private constructors; delete bridges and `TypeTextForm`; ratchet lowered to its floor; `StrExt` type helpers made private to `kotlin_type.rs` or deleted. | `#338·3056`, `#337·1711` (2) | `expr_type.rs`, `mod.rs`, `str_ext.rs`, `tests/architecture_ratchet.rs` |
| Not covered | syntax extraction / call-selection policy | `#337·1913`, `#338·5997`, `#339·9449`, `#339·9526` (4) | out of scope |

**What each slice deletes** (string helpers; counts from the stack/3 tip, excluding tests):

- **S1:** `format!("{..}?")` in `expr_type.rs` (2 sites); the `starts_with('(')` paren hack; `is_lambda_shaped`
  (4 call sites); `ExtensionReceiver::{base, written, is_type_parameter}`; CST-path calls of
  `extract_collection_element_type` (3); `TypeTextForm::BareName` callers.
- **S2:** `same_type_in_scopes` (5), `names_type_in_scope` (5), `ResolvedType::resolution_scope` (4),
  `origin_after_substitution`, `SpelledIn`, `callee_receiver_origin`, `KOTLIN_BUILTIN_TYPES` membership
  on the CST path, and the `(String, Url)` pairs in `infer/` (36).
- **S3:** `apply_type_subst` (13) and `apply_simple_subst`, `build_type_arg_subst`, `build_ext_fn_type_subst`,
  `match_type_args_recursive`, `same_type_constructors`, `mentions_identifier`, `is_generic_param` (11),
  `first_type_arg_raw`, `readable_type_argument`, `star_unbound_type_arguments`, `type_args_inner` (8),
  `split_top_level_commas` (9).
- **S4:** `find_fun_callable_info` (8), `find_member_type_params`, `find_extension_callable`,
  `unbound_type_parameters`, `is_nothing_typed`, CST-path `extract_return_type_from_detail` (15 total,
  part string engine), `find_class_type_params` by name (8).
- **S5:** `ResolvedType::{as_type_str, type_path}` bridges, `TypeTextForm`; `StrExt::{strip_nullable,
  is_nullable, strip_variance}` leave the public trait if no string-engine caller remains *(unverified:
  string-engine reliance)*.

**Risk per slice**

| Slice | Main risk | Mitigation |
|---|---|---|
| S1 | Display round-trip changes hover/inlay text (spacing, `out` projections). | Snapshot hover/inlay strings before/after on the fixture suite; benchmark should show zero recall drift. |
| S2 | Eager name resolution on hot paths (semantic tokens, inlay); recall loss where `Unresolved` used to compare "equal". | Per-request memo keyed by `(TypePath, file)` that lives only for one `CstQuery` request (snapshot-safe, AGENTS "no unsound lazy caching"); latency measured on the largest Moneta file; audit lost resolutions (open question 6). |
| S3 | `*` → `Unknown` for unbound parameters changes inlay text; `Declined` loses some currently-correct-by-luck bindings. | Benchmark diff by symbol; sampled audit. |
| S4 | Largest blast radius: the `InferDeps` surface, `TestDeps`, and `sig.rs` (1368 lines). | Land the trait method beside the old ones, migrate callers, delete in the same PR; `find_referencing_symbols` zero-check before each deletion. |
| S5 | Low. | Compiler-driven. |

**Benchmark caveat.** `resolution-accuracy` measures *recall*: a reference that resolves to the wrong
type still counts as resolved. Turning wrong answers into `Unknown` (most of S2/S3) can therefore
*lower* the reported number while improving correctness. Each slice reports the resolved-count delta
plus a sample of lost resolutions classified as "was wrong" or "was right"; only "was right" losses are
regressions.

## Regression suite

The 34 covered comments become tests in a new `src/indexer/infer/kotlin_type_regression_tests.rs`. Each
test is named after its behaviour and cites its comment in a doc comment (for example
`elvis_of_same_named_types_from_different_packages_is_unknown` cites `#338·5912`). Each slice adds its
rows from the migration table, verified **red on the parent commit before the slice** (the
depth-test lesson: an unverified-red test proves nothing). Tests the stack already added for the same
comments stay where they are; the new file adds the competing-declaration variants Copilot named:
cross-package same names, shadowing `error`, `Reordered<X, Y> : Base<Y>`, `fun <A> values(): List<model.A>`,
`Holder<(Int) -> String>?`, `this@Outer`. Pure algebra methods get table-driven unit tests with no
`TestDeps` at all; that they need none is itself the layer-2 check.

## Enforcement

### Ratchet: string-level type operations

Add a second table to `tests/architecture_ratchet.rs`, reusing its literal-count walker, with a
different root set and exclusion list:

- **Roots:** all of `src/`. The string engine is included so its counts freeze, not shrink.
- **Excluded:** `src/indexer/infer/kotlin_type.rs` (the one place these operations belong),
  `src/str_ext.rs` (definitions), and files ending in `tests.rs`.
- **Patterns** (literal substrings):

| Pattern | Catches |
|---|---|
| `trim_matches('?')`, `trim_end_matches('?')`, `strip_suffix('?')`, `ends_with('?')`, `push('?')`, `}?")` | nullability surgery |
| `.strip_nullable()`, `.is_nullable()`, `.strip_variance()` | `StrExt` type helpers |
| `split('<')`, `find('<')`, `rfind('>')`, `type_args_inner(`, `split_top_level_commas(` | argument parsing |
| `apply_type_subst(`, `is_generic_param(`, `build_ext_fn_type_subst(`, `build_type_arg_subst(` | substitution on text |
| `same_type_in_scopes(`, `names_type_in_scope(` | identity by spelling |
| `contains("->")` | function-type sniffing |

Same contract as the existing ratchet: counts only go down, a file is never added, and the fix is to
ask `ResolvedType`. The scan is literal and over-counts: `find('<')` also matches non-type code. A
frozen over-count is harmless; the ratchet only forbids growth.

**Baseline:** TBD, produced on the merge commit of S1 with:

```sh
for pattern in "trim_matches('?')" "trim_end_matches('?')" "strip_suffix('?')" "ends_with('?')" \
  "push('?')" '}?")' ".strip_nullable()" ".is_nullable()" ".strip_variance()" "split('<')" \
  "find('<')" "rfind('>')" "type_args_inner(" "split_top_level_commas(" "apply_type_subst(" \
  "is_generic_param(" "build_ext_fn_type_subst(" "build_type_arg_subst(" "same_type_in_scopes(" \
  "names_type_in_scope(" 'contains("->")'; do
  echo "== $pattern"
  git grep -F -c "$pattern" -- 'src/*.rs' ':!*tests.rs' ':!src/str_ext.rs' ':!src/indexer/infer/kotlin_type.rs'
done
```

(Without `:(glob)`, git pathspec `*` also matches `/`, so `src/*.rs` and `*tests.rs` cover nested files.)

### Proposed AGENTS.md line (Code Quality section; not applied)

> - **A type is a `ResolvedType`, never a string.** In `src/indexer/infer` and its callers, nullability, type arguments, substitution and identity are methods on `ResolvedType` (`src/indexer/infer/kotlin_type.rs`); don't strip or append `?`, split on `<`, or compare type text — `tests/architecture_ratchet.rs` counts it, lower a baseline, never raise it.

## Open questions

1. **Eager or lazy identity resolution?** *Recommend eager at parse-in with a per-request memo*, so the
   algebra stays pure. Fall back to sealed lazy `(path, scope)` only if S2's latency measurement on
   semantic tokens regresses past an agreed bound (proposal: p95 +10%).
2. **`ClassIdentity` = qualified name or declaration `Location`?** *Recommend qualified name* (KMP
   `expect`/`actual`, duplicate JARs). Needs confirmation that the package is cheap to obtain at every
   resolution site *(unverified)*.
3. **Java platform types.** *Recommend not modelling them now.* Java members parse as written. Add
   `Nullability::Platform` only if the nullable-call diagnostic needs it.
4. **Function types: dedicated head or `FunctionN` class?** *Recommend dedicated head*, normalising
   `FunctionN` spellings at parse-in. Someone must check the sidecar's spelling first.
5. **Store structured types in the index cache?** *Recommend no*: parse on read. A cache-format change
   means a version bump and memory cost (see the memory-reduction effort). Revisit if parse-in shows in
   profiles.
6. **Recall-drop acceptance for S2/S3.** *Recommend:* accept any drop where a 50-reference sample of
   lost resolutions is at least 80% "was wrong"; otherwise the slice is not done.
7. **Display of nested `Unknown`.** *Recommend:* inlay hints suppress any type containing `Unknown`
   (visible text must be right); hover shows the declared type text when one exists, else the head with
   `<…>`.
8. **Ratchet scope: all of `src/` or the CST domain only?** *Recommend all of `src/`* (frozen string
   engine counts cost nothing, and a new string op anywhere fails).
9. **Start before or after #337–#339 merge?** *Recommend after.* Their point fixes become the baseline
   behaviour and their tests the red/green anchors; designing against a moving stack is what forced
   symbol-only citations here.
10. **Text parser for indexed source declarations, or always CST via the unopened-parse memo?**
    *Recommend CST where the memo already holds a tree, text only for JAR/sidecar*, so the string parser
    has one input class. Needs a memo hit-rate measurement in S1.
