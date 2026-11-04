# Structured `ResolvedType` — Design (finishing "step 5: Sweep")

**Status:** draft for maintainer review, 2026-11-03; docs only, nothing implemented. Drafted against
the stack #337/#338/#339, now merged (`main` at `f5329570`); symbols cited are valid on `main`, counts in
the Problem section may be off by a few. The open questions were answered on 2026-11-04, reviewed by an
independent critique the same day (which re-measured the key numbers on the Moneta caches), and
reconciled into the [Decisions](#decisions-formerly-open-questions), tagged *measured*, *read* or
*inferred*. Claims marked *(unverified)* were not checked in code.
**Finishes:** step 5 of `docs/superpowers/specs/2026-06-30-cst-resolution-unification-design.md`
(the `RawTypeName`/`TypeName` split, construction-sealed outputs, exhaustive `CstExpr` dispatch), which
`docs/superpowers/plans/2026-08-02-cst-catalogue-gap-closure.md` deferred and confirmed unstarted.

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

**A third round** left 15 more comments (53 in total): 8 fixed in the stack, 7 deferred to slices
([Deferred review comments](#deferred-review-comments)). The rounds gave 22, 16 and 15 comments: each
round's point fixes added string operations the next round found gaps in.

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
2. Type identity is the class's `ClassId`, fixed when the type is parsed in its declaring scope. A consumer
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
- Changing the on-disk index or cache format in S1–S5. A minimal sidecar change is decided later, from
  the Slice 0 counter (decision 5).
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
    /// Inference gave up: no common supertype, conflicting bindings, an unbound
    /// type parameter after a call, `T & Any`, an intersection or context receiver.
    Unknown,
}

pub(crate) enum ClassIdentity {                       // K2 `ClassId` (decision 2)
    /// `kotlin.collections` / `List`, `app.ui` / `Screen.State`. Package and nested path interned.
    Global { package: PackageName, path: RelativeClassName },
    /// Local and anonymous classes: not addressable by name, never equal to a `Global`.
    Local(DeclarationSite),
}

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

**Identity is the K2-style `ClassId`, resolved at parse-in.** Alternatives considered:

| Option | For | Against |
|---|---|---|
| **`ClassId` = package + nested path (chosen)** | Kotlin's own classifier identity. The same class in two JARs is one type (34% of JAR top-level classes appear in more than one JAR, *measured*). Package and path are separate, so `a.b`+`C` is not `a`+`b.C`. Comparable without the index, so the algebra stays pure. | Needs the nested path (decision 2). |
| Declaration `Location` | Already what `resolve_type_path_declarations` returns. | Duplicate JAR classes compare unequal. |
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
return. The sidecar already renders `kotlin/FunctionN` as lambda syntax (*read*); parse-in normalises
a written `kotlin.Function1<…>` to this head. `FunctionType::class_identity()` gives
`kotlin.FunctionN` / `kotlin.coroutines.SuspendFunctionN` for member lookup and supertypes (decision 4).

**Nullability is two-valued** (decision 3). On a `TypeParameter` head, `nullability` means "marked `?`
at this use"; a bare `T` takes its nullability from the binding.

### Where strings legitimately remain

| Boundary | Direction | Mechanism |
|---|---|---|
| CST type nodes (`user_type`, `nullable_type`, `function_type`, type arguments/projections) | in | `ResolvedType::from_syntax(node, bytes, &TypeScope, &impl NameResolver)`: structural lowering, no text parsing (honours the 2026-06-30 "CST is the source of structure" principle) |
| Declaration text from the index or sidecar (`SymbolEntry` detail/return text, JAR signatures) | in | `ResolvedType::from_declaration_text(text, &TypeScope, &impl NameResolver)`: the **only** type-string parser in the CST domain, private to `kotlin_type.rs`; used only where no tree exists (decision 5) |
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
| `read_argument(index)` | Value read through an argument, combining use-site with declaration-site variance: `Invariant`/`Out` → the type; `Star` → the parameter's upper bound (default `Any?`); `In` → `Any?`. Needs the class's declared parameters (variance, bounds) at lowering. | `readable_type_argument`, `first_type_arg_raw`, `type_args_inner` |
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
| `read_argument` through `In`/`Star` | `Any?` / upper bound (not `Unknown`: these are known types) |
| top level at `CstQuery` / `BindingType` | `Resolution::Unresolved` / `BindingType::LocalUnknown` |
| display | per surface, decision 7 |

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
   memo from #339), `from_syntax` is preferred; see decision 5. The string engine stays a string parser
   by the non-goal, so "one parser" holds inside the CST domain only.
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
on the Moneta corpus before the next starts. The slices start from `main` after #337–#339: their point
fixes and tests are the baseline this design replaces mechanically.

The order follows dependency first, then comments killed per slice. Identity (the largest class) needs
the representation to exist; substitution needs owner identities; selection needs both. There is no
index-time stub slice: whether JAR types need sidecar or index work is decided from Slice 0's counter.

| Slice | Scope | Kills (regression tests) | Main files |
|---|---|---|---|
| Slice 0 Residue counter (tiny) | A benchmark column counting `Unresolved` heads after lowering, split by declaration source (workspace / source JAR / compiled JAR); the JAR columns decide decision 5. Reported by every later slice. | none | `resolution-accuracy` CLI |
| S1 Representation + nullability + shape | `kotlin_type.rs` (type, parse-in from CST and text, display, `without_null`/`nullable`/`read_argument`/`head`). Heads are `Unresolved`/`TypeParameter`/`Function`/`Unknown` only; no `Class` resolution yet. `expr_type.rs` internals move from `(String, Url)` to `ResolvedType`; `ExtensionReceiver` → `ResolvedType`. Bridge `Display` for unmigrated `chain.rs`. | `#338·6223`, `#339·9377`, `#339·8218`, `#337·7063`, `#339·8173`, `#339·8335`, `#339·9577` (7) | new `kotlin_type.rs`, `expr_type.rs`, `mod.rs`, `parser.rs`, `sig.rs` (`is_lambda_shaped`) |
| S2 Identity | `NameResolver` + `ClassIdentity` resolution at parse-in (eager, per-request memo, latency gate: decision 1), builtin const table, `same_type`, `join`. Elvis/`if`/agreement on identity. Delete the `(String, Url)` origin threading. | `#338·5912`, `#338·2949`, `#337·1982`, `#338·6084`, `#338·3128`, `#339·9202`, `#339·8120`, `#338·5829` (8) | `kotlin_type.rs`, `expr_type.rs`, `chain.rs`, `cst_lambda.rs`, `indexer.rs`, `deps.rs` |
| S3 Substitution | `TypeParameter` owners, `Substitution`, `Bindings` (conflict-aware, `Declined` on constructor mismatch), explicit call-site type arguments, `close_unbound`, `*`/`in` reads via declaration-site variance and bounds, member lookup through type-parameter upper bounds. | `#338·6037`, `#339·9156`, `#339·9261`, `#339·9320`, `#339·8034`, `#339·8082`, `#338·3157`, `#339·8245` (8) | `type_subst.rs` (mostly deleted), `chain.rs`, `cst_lambda.rs`, `lookup.rs` |
| S4 Selected callable | `select_callable → Resolution<SelectedCallable>`. `StrategyOutcome` carries it; call return types come from it; delegate shortcuts gated on `kotlin`/`androidx.compose.runtime` identity; no `error`/`TODO` name check. | `#337·1816`, `#337·7007`, `#338·6127`, `#338·6179`, `#339·9091`, `#339·8293`, `#338·3007`, `#338·5946`, `#338·3087` (9) | `deps.rs`, `indexer.rs`, `sig.rs`, `chain.rs`, `expr_type.rs`, `cst_lambda.rs` |
| S5 Sweep | `CstExpr` exhaustive dispatch; exhaustive `LocalBinding` consumers; private constructors; delete bridges and `TypeTextForm`; ratchet lowered to its floor; `StrExt` type helpers split into `TypeTextExt` (decision 8). | `#338·3056`, `#337·1711` (2) | `expr_type.rs`, `mod.rs`, `str_ext.rs`, `tests/architecture_ratchet.rs` |
| Not covered | syntax extraction / call-selection policy | `#337·1913`, `#338·5997`, `#339·9449`, `#339·9526` (4) | out of scope |

**"34 of 38" stands; one is a dependency.** An earlier answer claimed four of the 34 depend on index-time
JAR stubs; the critique checked each against the code (*read*). Only `#338·5829` has a dependency, the
builtin identity table, and a const array suffices. `#338·3087` and `#338·5143` need the *callable's*
declaring package, which `SidecarSymbol.pkg` / `jar_symbol_packages` already carry (S4). `#338·3157` is
substitution with `?` kept (S3). `#339·8173` is head recognition of a written FQN (S1).

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
  is_nullable, strip_variance}` move to `TypeTextExt` for the string engine (decision 8).

**Risk per slice**

| Slice | Main risk | Mitigation |
|---|---|---|
| S1 | Display round-trip changes hover/inlay text (spacing, `out` projections). | Snapshot hover/inlay strings before/after on the fixture suite; zero stable benchmark transitions (decision 6). |
| S2 | Eager name resolution on hot paths (semantic tokens, inlay) and the JAR promotion it triggers; recall loss where `Unresolved` used to compare "equal". | Per-request memo only; latency gate (decision 1); acceptance rule (decision 6). |
| S3 | Inlay hints with unbound arguments disappear (intended: `*` meant ignorance); `Declined` loses some currently-correct-by-luck bindings. | Count the vanished hints in the S3 report; acceptance rule (decision 6). |
| S4 | Largest blast radius: the `InferDeps` surface, `TestDeps`, and `sig.rs` (1368 lines). | Land the trait method beside the old ones, migrate callers, delete in the same PR; `find_referencing_symbols` zero-check before each deletion. |
| S5 | Low. | Compiler-driven. |

**Benchmark caveat.** `resolution-accuracy` measures *recall*: a wrong resolution still counts. Turning
wrong answers into `Unknown` (most of S2/S3) can lower the number while improving correctness, so each
slice is judged by decision 6.

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

### Deferred review comments

Seven round-3 comments were valid, not fixed in #338/#339, and are acknowledged in their threads as
tracked here. IDs are abbreviated as above (`#338·4829` is comment 4453154829).

| Comment | Scenario | Slice | Why it waits |
|---|---|---|---|
| `#338·4829` indexer.rs | Built-in name shadowed in one scope only; the global fallback equates the declarations. | S2 | `same_type` on identities removes the built-in shortcut. |
| `#338·4897` expr_type.rs | Two imported `Holder.getValue` extensions chosen by receiver leaf name. | S4 | `select_callable` filters by resolved receiver identity. |
| `#338·4942` cst_symbol.rs | `is` narrowing survives a later reassignment of the subject. | not covered | Needs a stability analysis (flow), not a type representation. Tracked separately. |
| `#338·5040` expr_type.rs | Numeric-literal elvis default stays untyped (recall only). | S1 | Literals lower to `ResolvedType` directly. |
| `#338·5143` expr_type.rs | A custom `Lazy`/`MutableState` factory is mistaken for the stdlib one. | S4 | Shortcuts gated on resolved callable identity. |
| `#338·5266` expr_type.rs | A substituted type argument's origin is re-anchored to the caller. | S2, S3 | Origin lives in the head and survives substitution. |
| `#339·0957` expr_type.rs | Qualified `kotlin.collections.Map.Entry` is not recognised (recall only). | S1 | Shape recognised by head, not spelling. |

Six of the seven are covered by S1–S4; one (`#338·4942`) is a flow-analysis gap this design does not
address.

## Enforcement

Four tiers, strongest first. A weaker tier is added only where a stronger one cannot reach.

| Tier | Mechanism | Covers |
|---|---|---|
| 1. Delete | Each slice deletes the helpers it replaces (lists above). | the project helpers (`apply_type_subst(`, `is_generic_param(`, …) |
| 2. Narrow visibility | Move `is_nullable`/`strip_nullable`/`strip_variance` out of `StrExt` into a `TypeTextExt` trait. One ratchet pattern, `TypeTextExt`, with a per-file allowed list: no new file imports it, and `src/indexer/infer` reaches zero by S5. | the three `StrExt` type helpers |
| 3. clippy `disallowed-methods` | Only if tier 2 proves leaky. Verified in this bin-only repo by the critique (*measured*): `clippy.toml` listing `kmp_lsp::str_ext::StrExt::strip_nullable`, `allow` in `main.rs`, `deny` in `src/indexer/infer/mod.rs` gave 16 errors, all in `infer`, none elsewhere (also under `--tests`). It covers only the 3 trait methods, and a `crate::` path silently matches nothing, so it needs a canary (one banned call under `#[expect(clippy::disallowed_methods)]`). | the same three methods |
| 4. Literal ratchet | Standard-library char surgery, which no path-based tool can ban without banning `ends_with` itself. | the table below |

**Literal ratchet.** A second table in `tests/architecture_ratchet.rs` with its counting rule
(occurrences, `//` lines skipped) and contract (counts only go down, a file is never added). Roots: all
of `src/`, so string-engine counts freeze. Excluded: `kotlin_type.rs`, `src/str_ext.rs`, `*tests.rs`.
**Baseline on `main` `f5329570`**, as total / files / in `src/indexer/infer` (*measured* by the answers;
the critique reproduced all 21 rows and the total of 130 exactly). Counts include the helpers' own `fn`
definitions, inline `#[cfg(test)]` modules and one HTML `find('<')`; struck-out rows leave the table.

| Pattern | Count | Pattern | Count | Pattern | Count |
|---|---|---|---|---|---|
| `trim_matches('?')` | 1 / 1 / 0 | `trim_end_matches('?')` | 2 / 1 / 0 | `strip_suffix('?')` | 0 / 0 / 0 |
| `ends_with('?')` | 3 / 3 / 2 | `push('?')` | 0 / 0 / 0 | `}?")` | 4 / 3 / 2 |
| ~~`.strip_nullable()`~~ | 25 / 10 / 16 | ~~`.is_nullable()`~~ | 6 / 5 / 4 | ~~`.strip_variance()`~~ | 6 / 3 / 5 |
| `split('<')` | 6 / 5 / 1 | `find('<')` | 7 / 7 / 2 | `rfind('>')` | 2 / 2 / 1 |
| `type_args_inner(` | 10 / 2 / 10 | `split_top_level_commas(` | 9 / 3 / 9 | `apply_type_subst(` | 14 / 9 / 8 |
| `is_generic_param(` | 11 / 4 / 11 | `build_ext_fn_type_subst(` | 6 / 4 / 6 | `build_type_arg_subst(` | 4 / 3 / 3 |
| `same_type_in_scopes(` | 6 / 4 / 5 | `names_type_in_scope(` | 5 / 4 / 4 | ~~`contains("->")`~~ | 3 / 3 / 2 |

| Change | Pattern | Reason |
|---|---|---|
| Removed | `.is_nullable()`, `.strip_nullable()` | Collide with the API this design promotes (`.is_nullable()` already counts `ResolvedType::is_nullable`). Tier 2 covers the `StrExt` versions, and `.strip_variance()` with them. |
| Widened | `}?")` → `}?"`; `contains("->")` → `"->"` | `}?")` misses `format!("{}?", x)` (`}?"`: 4 hits, critique). `"->"`: 7 (4 in `infer`). |
| Added | `starts_with('(')` 7 (5), `strip_prefix("suspend` 2 (2), `starts_with('?')`, `'?' =>` | Paren hack, suspend sniffing (*measured*, answers); the last two were found by the critique in `resolver/infer_lines.rs`, baseline taken when added. |
| Not added | `last_segment()` 31, `rsplit('.')` 17, one `take_while(… '?')` | Many legitimate non-type uses; leaf-name identity goes with `same_type_in_scopes`. The `take_while` spelling is too specific to recur. |

### Proposed AGENTS.md line (Code Quality section; not applied)

> - **A type is a `ResolvedType`, never a string.** In `src/indexer/infer` and its callers, nullability, type arguments, substitution and identity are methods on `ResolvedType` (`src/indexer/infer/kotlin_type.rs`). Don't strip or append `?`, split on `<`, sniff `->` or compare type text, and don't import `TypeTextExt` in a new file. `tests/architecture_ratchet.rs` counts both; lower a baseline, never raise it.

## Decisions (formerly open questions)

Reconciled from the answers and the critique; where they disagreed on a number, the critique's
re-measurement is used and noted. Numbers without a tag are *measured*. Question 9 (start after
#337–#339) was already decided.

**1. Identity resolves eagerly at lowering, with a per-request memo only.** A lazy `(path, scope)` head
makes every equality an index query, so the pure algebra (goal 3) could not exist; the "lazy if p95
+10%" fallback is deleted. The memo belongs to one `CstQuery` request and is never cross-request: a
name's answer depends on the whole index, and there is no index revision counter to key it on (AGENTS
purity and snapshot rules). Costs:

| Evidence | Value |
|---|---|
| Name resolution share, best case (`tokens --resolve`, 3 largest Moneta files) | 5–6.5% (140 ms of 2.8 s; inference dominates) |
| `resolve_chain`, full corpus run | 41% of the run (164.5 s, 28.6 µs/call) |
| `resolve_type_path_declarations` | ~40 µs/call (1.38 s / 34,014); 0–5 calls per request today, multiplied by eager lowering (+5–17% per 2.8 s request, *inferred*) |
| JAR promotion (*read*) | resolution calls `ensure_jar_definitions_for`, promoting JAR symbols for every type name touched |

*Gate:* S2 measures eager-lowering latency (`tokens --resolve`, largest Moneta files) and JAR promotions
per request; no p95 regression beyond run-to-run noise. *If it regresses:* lower only the heads actually
inspected (arguments on first read, complete once lowered), never a lazy identity.

**2. `ClassIdentity` is a K2 `ClassId`** (`Global { package, path }` | `Local(site)`, parts interned).

| Point | Decision and evidence |
|---|---|
| Name, not `Location` | JAR duplication: 21,950 of 63,847 JAR top-level classes (34%) live in more than one JAR. Not expect/actual: Moneta has 0 `expect` declarations and the index does not model them (*read*); KMP identity needs a fixture built for S2. `Indexer::qualified` is not changed. |
| Package | Cheap (*read*): `Indexer::package_of(uri)`, `jar_symbol_packages`. |
| Workspace nested path | Derived from the stored `SymbolEntry.range`: 391 of 17,967 nested class-likes have an ambiguous container name, and all 391 resolve uniquely by range containment. No index change. |
| JAR nested path | Ambiguous for 5,973 of 43,715 (13.7%); the Java visitor keeps one `$` segment (*read*). Needs the sidecar's nested path (decision 5). |
| Typealiases | Expand at lowering (cycle guard); the alias name is kept for display only. The cache indexes them as kind `Class` (23 of 25), not `TYPE_PARAMETER` as the answers said. |
| Builtins | A const table of `kotlin.*` identities seeded from `KOTLIN_BUILTIN_TYPES`, reached through default imports. No index work. `resolver/platform_types.rs` stays a member/go-to-definition lookup map. |

**3. Nullability stays two-valued** (`NonNull | Nullable`). Platform types (`T!`) and definitely-non-null
`T & Any` are a stated non-goal because nothing can construct them: Java methods in the JAR index carry
no return type and no annotations are read (*read*, `KotlinClassIndexer.kt`); Moneta has 0 `& Any`. Adding
a variant later is compiler-driven (every exhaustive `match` stops compiling). A bare `T` is a
`TypeParameter` head whose `nullability` means "marked `?` here"; substitution keeps the binding's
nullability, widened by the mark. `T & Any` lowers to `Unknown`. *Revisit when* the first producer of
platform-type facts exists (e.g. Java nullability annotations from the sidecar) with a consumer.

**4. Dedicated function head, plus a class identity.** `FunctionType::class_identity()` gives
`kotlin.FunctionN` / `kotlin.coroutines.SuspendFunctionN` (arity counts the receiver) for `f.invoke`,
extensions on function types, `class Foo : (Int) -> Unit` and `same_type` against a written
`Function1<A, B>`; context receivers lower to `Unknown`. Sidecar renderer defects (*read*,
`KotlinClassIndexer.kt`), all inputs to the minimal sidecar change: (a) a nullable function type renders
as `"$body?"` without parentheses, so `((A) -> B)?` reads as `(A) -> B?` (518 JAR signatures by the
critique's strict count; the answers' 1,501 is a loose ceiling); (b) `FunctionN` is detected by a
simple-name regex, misreading a user class `Function3`; (c) an unknown type-parameter id renders as `T`;
(d) class names lose their package.

**5. Declaration types: declaring-scope text where there is no tree; no index-time stubs.** This
reverses the answers' "S0 stubs as a prerequisite of S2". The original design stands:
`from_declaration_text` with the declaring package and scope and an `Unresolved` fallback (today's
by-name behaviour), and `from_syntax` wherever a tree exists.

| Declaration source | Tree at query time | Lowering |
|---|---|---|
| Workspace source | open file, or the unopened-parse memo (97.4% hit rate over a corpus run; 0.47 ms median parse) | `from_syntax` |
| Source JAR (tree-sitter parsed, full signatures) | parsed at index time; served by the memo: unverified | `from_syntax` if available, else text |
| Compiled JAR (sidecar) | none | `from_declaration_text` |

- *The 75% "ambiguous package" figure is inflated:* 68% of ambiguous JAR heads are `String`, `Boolean`,
  `Int`, `List` and similar, which default imports resolve. Of 144,445 heads naming an indexed class,
  same-package and single-candidate rules leave ~22,090 (15%; ~4% of all non-function return heads;
  crude heuristics). 239,302 class-looking heads name no indexed class at all.
- *Workspace detail is truncated* at 120 characters in 10.73% of entries, mostly outside the type (44%
  are class-likes, 27% start with an annotation). About 1,100 functions lose a declared return type and
  up to 2,641 are cut mid-type, at most ~3% of typed declarations (sample-extrapolated);
  `declared_return_type` re-reads source then. Another reason to prefer CST.
- *Slice 0* adds the residue counter as a benchmark column (it has data once S2 lowers identities); the
  stub/sidecar decision is taken from it. If compiled-JAR `Unresolved` dominates, the **minimal sidecar
  change** adds optional fields: fully qualified names, parenthesised nullable function types, the nested
  path. Costs: a `JAR_CACHE_VERSION` bump re-runs the sidecar over 998 JARs; a `CACHE_VERSION` bump (avoid
  it) orphans the ~695 MB sources-JAR cache; `detail` has ~60 read sites, so structure is additive (no
  "memory nets to zero"); the sidecar ships separately, so an old one decodes (`serde(default)`) but
  silently loses the gain. Ship the sidecar change first and alone.

**6. Acceptance rule for slices that change resolutions.** (1) Run base and slice 3 times each and
compare only references *stable* across a build's runs (needs the determinism fixes merged in #337).
(2) Classify stable transitions as gains, losses and retargets. (3) Sample gains and losses uniformly,
seeded from the base commit hash and stratified by reference shape (bare, member on local, member on
call result, field chain, `it`/`this`); a reviewer who has not seen which side is new labels each answer
right or wrong. (4) Accept if, on sample-extrapolated counts, *right gained + w·wrong removed ≥ right
lost + w·wrong gained* (with no gains: *w·wrong removed ≥ right lost*), and no cluster of ≥ 10
right-answer losses shares one root cause; such a cluster is fixed before merge, as `mapCardItems` was.
(5) Behaviour-neutral slices (S1, S5) need zero stable transitions.

*w is a policy choice for the maintainer, not a measurement.* Recommended w = 2: a wrong answer costs
twice a missing one, so break-even is one wrong answer removed per two right answers lost (the earlier
"trade 1 right for 3 wrong" was inverted; that is w = 1/3). *The final #339 comparison* (111 losses,
1,172 gains, 19% of a 62-loss sample wrong): right lost ≈ 89.9, wrong removed ≈ 21.1, so losses give
2 × 21.1 − 89.9 = −47.7. Gains must cover it: 1,172 × (ĝ − 2(1 − ĝ)) ≥ 47.7, so **ĝ ≥ 0.68**. #339
passes only if at least 68% of its gains are right; they were not sampled, so that is plausible but
unproven, hence step 3 samples gains. *Sample size* for ±10 points at 95%: n = n₀/(1 + (n₀ − 1)/N) with
n₀ = 1.96² · 0.25 / 0.1² ≈ 96, giving 52 for N = 111 and 89 for N = 1,172. Point estimates only; close
calls go to the maintainer. *Optional, untried:* a gold set of ~1,200 references labelled with the
Kotlin Analysis API would measure precision, which the benchmark cannot (feasibility *inferred*).

**7. Display of `Unknown`, per surface.** Inlay hints: no hint for any type containing `Unknown`.
Hover and completion "Inferred type": the declared text when present, else the head with `…` per unknown
argument (`Map<String, …>`), no type line if the head itself is unknown. `Unresolved` names show as
written. Never `_` (`List<_>` is not valid Kotlin) or `*` (`List<*>` is a real type); today
`star_unbound_type_arguments` (`chain.rs`) writes `*` for ignorance, which S3 removes. Hover on locals
still goes through the string engine (`infer_variable_type_raw`, *read*), so most of this applies once
hover migrates.

**8. Ratchet:** all of `src/`, four tiers ([Enforcement](#enforcement)).

**Still open (maintainer):** the weight w and the strata (recommend w = 2 and the five shapes above);
whether to make the sidecar change (recommend: only if Slice 0 shows compiled-JAR residue dominating).

### Missing considerations

Three declaration sources, sidecar release skew, the residue counter and the stability gate's
dependency on determinism are covered in decisions 5 and 6.

| Topic | Position |
|---|---|
| No KMP code in Moneta | expect/actual needs a fixture; the benchmark cannot see it. `CallableIdentity(Location)` makes an `expect fun <T>` and its `actual` different owners; acceptable until the fixture shows a mismatch. |
| Typealiases | Workspace expansion needs the CST (detail is cut at `=`, *read*). The sidecar emits no JAR aliases (`KmPackage.typeAliases` ignored, *read*), so `kotlin.collections.ArrayList` stays `Unresolved` until the sidecar change. |
| Java ↔ Kotlin classes | `java.lang.String` and `kotlin.String` are one identity; the builtin table maps the Java names (as K2's `JavaToKotlinClassMap`, *inferred*). |
| Type-parameter upper bounds | Member lookup on a `T : CharSequence` receiver needs bounds by owner, or S3 turns by-name successes into `Unknown`. In S3. |
| Intersection, captured types, context receivers | Out of scope; lower to `Unknown`. Smart casts keep today's narrowed type. |
| Raw Java types, arrays | Raw `List` gets `Unknown` arguments; `int[]` → `kotlin.IntArray`; `T[]` → `kotlin.Array<out T>` (*inferred*). |
| Interning | Intern `ClassIdentity` parts (the `&self` interners in `types.rs` are the model); whole types only if a profile shows clone or hash cost. |
| Fallback reliance | The Slice 0 counter also counts member lookups served by the `Unresolved` by-name fallback. |
