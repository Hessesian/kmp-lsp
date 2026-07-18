# Kotlin 2.4 language coverage audit

## Result

The Kotlin language coverage matrix is complete for the stable Kotlin 2.4 / compiler `v2.4.10` boundary. The audit preserves the 2,105 Kotlin 1.9 baseline requirements, verifies every one through 2.4, and adds 40 source-backed `KL-*` requirements for behavior introduced or changed after the baseline.

No baseline `KS-*` requirement was retired. No production code, dependency, parser version, build script, or runtime configuration was changed.

The final current-requirement matrix contains 2,145 requirements:

| Classification / status | Kotlin 1.9 baseline | Kotlin 2.4 final | Change |
|---|---:|---:|---:|
| exact / active | 619 | 632 | +13 |
| exact / ignored | 358 | 385 | +27 |
| heuristic / active | 27 | 27 | 0 |
| heuristic / ignored | 57 | 57 | 0 |
| out-of-scope / excluded | 1,044 | 1,044 | 0 |
| **Total** | **2,105** | **2,145** | **+40** |

Status totals are 659 active, 442 ignored, and 1,044 excluded. The 442 ignored requirements have 448 unique, individually executable tests.

## Pinned source boundaries

| Purpose | Repository / release | Revision | Source boundary |
|---|---|---|---|
| Normative Kotlin 1.9 baseline | `Kotlin/kotlin-spec` / `1.9-rfc+0.1` | `2f7aa0524ec27e788dfacd550f144809f2e0254c` | `docs/src/md`, 20 Kotlin/Core documents |
| Compiler baseline | `JetBrains/kotlin` / `v1.9.0` | `bcf27812cd28041e0b9ffa3bfe52fc58c397d0eb` | compiler sources and test data |
| Stable target | `JetBrains/kotlin` / `v2.4.10` | `5687445832cd835b4509b9fbc264cdf1a8201093` | compiler sources, test data, stable changelogs through 2.4.10 |
| Kotlin 2.3 maintenance source | `JetBrains/kotlin` | `eb08be1d1e0114988f5c7388b5c14855cdf819e0` | `docs/changelogs/ChangeLog-2.3.X.md`; target semantics rechecked at the stable target |
| Preview boundary | `JetBrains/kotlin` / `v2.4.20-Beta1` | `adf58296cf6637999310c497834b3d97516abf5f` | `ChangeLog.md`, section 2.4.20-Beta1 only |
| Current Language Guide | `JetBrains/kotlin-web-site` | `7c270c2ac320fbee4884927f056b89d32f2a002e` | 49 local topics under the `Language guide` subtree in `docs/kr.tree` |

The Kotlin 1.9 specification citations remain historical baseline citations; they are not presented as a Kotlin 2.4 specification. Post-baseline changes are represented in the separate evolution and documentation layers.

## Stable changelog audit

All 9,084 stable changelog bullets in the required intervals have contiguous audit IDs and one disposition.

| Release line | Audit items | Existing | Changed | New | Duplicate | Excluded | Current `KL-*` requirements | Active / ignored |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1.9 | 1,025 | 17 | 8 | 0 | 0 | 1000 | 8 | 2 / 6 |
| 2.0 | 2,849 | 3 | 0 | 9 | 1 | 2836 | 9 | 3 / 6 |
| 2.1 | 1,504 | 3 | 0 | 8 | 23 | 1470 | 7 | 4 / 3 |
| 2.2 | 1,604 | 3 | 0 | 13 | 27 | 1561 | 5 | 1 / 4 |
| 2.3 | 1,371 | 3 | 0 | 9 | 149 | 1210 | 6 | 2 / 4 |
| 2.4 | 731 | 1 | 0 | 10 | 47 | 673 | 5 | 1 / 4 |
| **Total** | **9,084** | **30** | **8** | **49** | **247** | **8,750** | **40** | **13 / 27** |

The 8,750 stable changelog exclusions are individually reasoned and typed:

| Exclusion kind | Items |
|---|---:|
| analysis API | 999 |
| backend | 25 |
| build tool | 1,780 |
| code generation | 724 |
| compiler infrastructure | 405 |
| compiler plugin | 239 |
| compiler semantics not observable through the LSP contract | 2,345 |
| documentation-only | 34 |
| IDE-only | 332 |
| performance | 78 |
| platform-specific | 1,446 |
| runtime | 22 |
| standard library | 312 |
| test infrastructure | 9 |
| **Total** | **8,750** |

The separate `v2.4.20-Beta1` preview audit contains 497 bullets: 21 preview-deferred, 75 duplicates, and 401 typed exclusions. No preview-only behavior is counted as stable `v2.4.10` support.

## Current Language Guide audit

The pinned TOC contains exactly 49 local topics. The completed audit records 150 atomic claims: 131 linked language claims and 19 typed exclusions. Those linked claims reference 149 distinct current requirements.

| # | Topic | Claims | Linked requirements | Exclusions |
|---:|---|---:|---:|---:|
| 1 | `basic-syntax.md` | 5 | 8 | 0 |
| 2 | `keyword-reference.md` | 2 | 2 | 0 |
| 3 | `packages.md` | 2 | 3 | 0 |
| 4 | `annotations.md` | 3 | 3 | 0 |
| 5 | `visibility-modifiers.md` | 2 | 1 | 1 |
| 6 | `coding-conventions.md` | 3 | 2 | 1 |
| 7 | `idioms.md` | 4 | 4 | 1 |
| 8 | `types-overview.md` | 2 | 1 | 1 |
| 9 | `numbers.md` | 3 | 4 | 1 |
| 10 | `unsigned-integer-types.md` | 1 | 0 | 1 |
| 11 | `booleans.md` | 3 | 4 | 0 |
| 12 | `characters.md` | 3 | 2 | 1 |
| 13 | `strings.md` | 4 | 4 | 1 |
| 14 | `arrays.md` | 4 | 4 | 1 |
| 15 | `typecasts.md` | 3 | 3 | 0 |
| 16 | `type-aliases.md` | 2 | 3 | 0 |
| 17 | `control-flow.md` | 5 | 6 | 0 |
| 18 | `returns.md` | 2 | 3 | 0 |
| 19 | `exceptions.md` | 4 | 4 | 1 |
| 20 | `functions.md` | 6 | 8 | 0 |
| 21 | `lambdas.md` | 5 | 5 | 0 |
| 22 | `this-expressions.md` | 2 | 2 | 0 |
| 23 | `type-safe-builders.md` | 2 | 2 | 0 |
| 24 | `using-builders-with-builder-inference.md` | 2 | 1 | 0 |
| 25 | `context-parameters.md` | 3 | 2 | 0 |
| 26 | `inline-functions.md` | 3 | 3 | 0 |
| 27 | `operator-overloading.md` | 3 | 4 | 0 |
| 28 | `unused-return-value-checker.md` | 1 | 0 | 1 |
| 29 | `classes.md` | 3 | 3 | 0 |
| 30 | `data-classes.md` | 3 | 3 | 0 |
| 31 | `extensions.md` | 3 | 5 | 0 |
| 32 | `interfaces.md` | 3 | 5 | 0 |
| 33 | `delegation.md` | 2 | 2 | 0 |
| 34 | `inheritance.md` | 3 | 3 | 0 |
| 35 | `object-declarations.md` | 4 | 6 | 0 |
| 36 | `sealed-classes.md` | 3 | 4 | 1 |
| 37 | `enum-classes.md` | 2 | 2 | 0 |
| 38 | `inline-classes.md` | 3 | 4 | 1 |
| 39 | `nested-classes.md` | 2 | 2 | 0 |
| 40 | `fun-interfaces.md` | 2 | 3 | 0 |
| 41 | `properties.md` | 5 | 9 | 0 |
| 42 | `delegated-properties.md` | 3 | 4 | 1 |
| 43 | `null-safety.md` | 5 | 8 | 0 |
| 44 | `equality.md` | 3 | 2 | 1 |
| 45 | `generics.md` | 5 | 7 | 1 |
| 46 | `async-programming.md` | 2 | 1 | 1 |
| 47 | `coroutines-overview.md` | 3 | 2 | 1 |
| 48 | `reflection.md` | 3 | 3 | 1 |
| 49 | `destructuring-declarations.md` | 4 | 4 | 0 |
| **Total** | **49 topics** | **150** | **per-topic unique counts** | **19** |

Documentation exclusions:

| Exclusion kind | Claims |
|---|---:|
| build tool | 1 |
| compiler infrastructure | 2 |
| non-behavioral explanation | 1 |
| platform-specific | 6 |
| runtime | 2 |
| standard library | 6 |
| style | 1 |
| **Total** | **19** |

`coding-conventions.md` and `idioms.md` were audited claim by claim. Their style and standard-library material is excluded separately, while their language examples remain linked to requirements.

## Baseline migration result

All 2,105 current `KS-*` requirements carry `verified_through = "2.4"`.

- Migrated `KS-*`: none.
- Retired `KS-*`: none.
- Lost or duplicated current/historical IDs: none.
- Added `KL-*`: 40.
- Post-baseline changed behaviors modeled as `KL-*`: 19.
- New behaviors modeled as `KL-*`: 21.

The lack of retired `KS-*` entries is an audit result, not an assumption: the stable evolution sources did not establish that a baseline requirement's atomic statement ceased to be current. Corrections and new versioned behavior are represented by the evolution requirements below.

## Added evolution requirements

| Requirement | Kind | Maturity / compiler flag | Classification / status | Statement |
|---|---|---|---|---|
| `KL-1-9-0001` | changed | stable | exact / ignored | A class literal must have an explicit type or value to the left of ::class; the empty ::class form is rejected. |
| `KL-1-9-0002` | changed | stable | exact / active | An unambiguous callable reference retains its declaration target when its surrounding expected type is incompatible, allowing the mismatch to be reported on the containing expression. |
| `KL-1-9-0003` | changed | stable | exact / ignored | An enum entry cannot be selected as the right-hand side of a callable reference. |
| `KL-1-9-0004` | changed | stable | exact / ignored | A reference to an enum entry annotated Deprecated is marked deprecated, while an unannotated competing entry is not. |
| `KL-1-9-0005` | changed | stable | exact / ignored | Calls through a function-type value cannot use named arguments, even when the function type declares parameter names. |
| `KL-1-9-0006` | changed | stable | exact / active | Without the FunctionalTypeWithExtensionAsSupertype language feature, an extension function type is forbidden as a class supertype. |
| `KL-1-9-0007` | changed | stable | exact / ignored | A type-parameter name is not a value expression; a same-named companion property remains the value-resolution target inside the generic inner class. |
| `KL-1-9-0008` | changed | stable | exact / ignored | The synthetic enum entries property has priority over a same-named companion property for an Enum.entries access. |
| `KL-2-0-0001` | changed | stable | exact / active | A declaration in the root package is not implicitly visible from a named package; an explicit import makes it resolvable. |
| `KL-2-0-0002` | new | stable | exact / active | In a true branch of a Boolean Elvis condition whose left side is a safe call and whose fallback is false, the safe-call receiver is smart-cast to non-null. |
| `KL-2-0-0003` | new | stable | exact / active | After a successful disjunction of type checks, the checked value is smart-cast to the common supertype of the alternatives. |
| `KL-2-0-0004` | new | stable | exact / ignored | A Boolean disjunction whose false path exits the function propagates the surviving non-null fact after the expression. |
| `KL-2-0-0005` | changed | stable | exact / ignored | The type of a prefix increment expression is the getter type of the assigned property, not the return type of the inc operator. |
| `KL-2-0-0006` | changed | stable | exact / ignored | An annotation on a companion object is resolved without the companion object's own member scope. |
| `KL-2-0-0007` | changed | stable | exact / ignored | A when expression over an empty sealed or enum type remains non-exhaustive, including when the subject type is nullable. |
| `KL-2-0-0008` | new | stable | exact / ignored | A multi-dollar string interpolation prefix selects how many consecutive dollar signs begin interpolation. |
| `KL-2-0-0009` | new | stable | exact / ignored | A subject-based when branch may add a Boolean guard after one primary condition, and the guarded branch retains the primary condition's smart cast. |
| `KL-2-1-0001` | changed | stable | exact / active | A root-package object used as a value is not implicitly visible from a named package; an explicit import makes it resolvable. |
| `KL-2-1-0002` | new | stable | exact / ignored | A named context parameter may precede a function or property declaration, and its name is in scope in that declaration body. |
| `KL-2-1-0003` | new | stable | exact / ignored | A when expression over a type parameter with a sealed upper bound is exhaustive when its branches cover every direct non-sealed subtype of that bound. |
| `KL-2-1-0004` | changed | stable | exact / active | The legacy soft keywords header and impl are valid enum-entry names and resolve like ordinary enum entries. |
| `KL-2-1-0005` | changed | stable | exact / active | A package declaration cannot carry declaration modifiers such as public. |
| `KL-2-1-0006` | new | stable | exact / ignored | The all annotation use-site target applies an eligible annotation to every applicable property-related target. |
| `KL-2-1-0007` | new | stable | exact / active | A type alias may be nested in a classifier, and an inherited nested type alias resolves in a derived class. |
| `KL-2-2-0001` | new | experimental; `+UnnamedLocalVariables` | exact / active | An underscore may declare an unnamed local variable whose initializer is evaluated without binding a usable name. |
| `KL-2-2-0002` | new | experimental; `+ContextSensitiveResolutionUsingExpectedType` | exact / ignored | An expected enum, sealed, or companion-bearing type supplies the implicit qualifier for an unqualified member in type, expression, call-argument, and annotation-argument positions. |
| `KL-2-2-0003` | new | stable | exact / ignored | Data-flow facts from a preceding equality guard or definite assignment narrow a when subject's remaining cases for exhaustiveness. |
| `KL-2-2-0004` | changed | stable | exact / ignored | After an Elvis expression whose right side invokes an inline lambda that exits the enclosing function, a non-null left operand is smart-cast on the surviving path. |
| `KL-2-2-0005` | new | stable | exact / ignored | A local function may declare a named context parameter, whose name is in scope in the local function body. |
| `KL-2-3-0001` | new | experimental; `-Xlocal-type-aliases` | exact / active | A type alias declared inside a function is in scope for later type references in that function. |
| `KL-2-3-0002` | new | experimental; `-Xname-based-destructuring=complete` | exact / ignored | Full-form name-based destructuring may bind data-class properties by name while renaming the introduced local variables. |
| `KL-2-3-0003` | new | stable | exact / active | A property may declare an explicit backing field initializer beneath its property type, while references continue to target the property declaration. |
| `KL-2-3-0004` | changed | stable | exact / ignored | A return expression is permitted directly in an expression body only when the function declares an explicit return type. |
| `KL-2-3-0005` | changed | stable | exact / ignored | A sealed triangle hierarchy is exhaustive when every reachable non-sealed leaf is covered once, including a leaf shared by two sealed paths. |
| `KL-2-3-0006` | new | experimental; `-Xname-based-destructuring=only-syntax` | exact / ignored | Square-bracket positional destructuring introduces one local variable for each selected component. |
| `KL-2-4-0001` | new | experimental; `-Xcollection-literals` | exact / ignored | A collection literal is valid only when its expected type supplies a companion operator factory named of that accepts the literal elements. |
| `KL-2-4-0002` | new | experimental; `+CompanionBlocksAndExtensions` | exact / ignored | A member declared in a class companion block is callable through that class's classifier. |
| `KL-2-4-0003` | new | experimental; `+CompanionBlocksAndExtensions` | exact / ignored | A top-level companion extension declared for a classifier is callable through that classifier and resolves to the matching receiver's declaration. |
| `KL-2-4-0004` | changed | stable | exact / active | A preceding null guard smart-cast transfers to a when subject variable initialized from the guarded value, so the non-null sealed cases are exhaustive. |
| `KL-2-4-0005` | new | experimental; `-Xexplicit-context-arguments` | exact / ignored | A named explicit context argument selects the overload whose context parameter has that name and compatible type. |

The current requirement exclusion total remains the baseline 1,044 entries:

| Exclusion kind | Requirements |
|---|---:|
| compiler semantics | 733 |
| runtime | 183 |
| platform-defined | 61 |
| unspecified/TODO boundary | 52 |
| standard library | 15 |
| **Total** | **1,044** |

## Key unsupported Kotlin 2.4 gaps

The exact ignored tests expose unsupported behavior rather than masking it:

- Named context parameters and explicit context arguments do not parse cleanly; context-parameter scope and explicit-argument overload selection are unavailable.
- Collection literals parse without the required expected-type companion `operator fun of` validation.
- Companion blocks, companion extensions, `@all` use-site targets, multi-dollar interpolation, and `when` guards are not recognized by the production grammar.
- Context-sensitive resolution does not use expected enum, sealed, companion-bearing, call-argument, or annotation-argument types.
- Exhaustiveness diagnostics do not fully model generic sealed upper bounds, empty bounded types, data-flow narrowing, shared triangle leaves, or inline-lambda control-flow facts.
- Several K2 diagnostic rules remain absent, including empty-left-hand-side class literals, enum-entry callable references, deprecated enum-entry diagnostics, invalid named lambda arguments, and expression-body return restrictions.
- Experimental name-based and square-bracket destructuring forms remain unsupported.

Supported evolution behavior is kept active, including local type aliases, inherited nested type aliases, explicit backing-field navigation, underscore local declarations, root-package/import corrections, package modifier rejection, and Kotlin 2.4 smart-casted `when` subject exhaustiveness.

## Verification

The following checks pass on the final tracked audit state:

- `cargo test coverage_matrix_has_valid_traceability_entries --quiet`
- `cargo test coverage_matrix_matches_pinned_source_checkout -- --ignored`
- `cargo test evolution_matrix_matches_pinned_kotlin_checkout -- --ignored`
- `cargo test documentation_matrix_matches_pinned_kotlin_web_site_checkout -- --ignored`
- all 448 coverage-managed ignored tests, individually filtered with `cargo test --bin kmp-lsp <test> -- --ignored`; all failed for their recorded gap, with no unexpected pass, zero-test filter, multi-test match, timeout, or harness failure (40.78 seconds)
- `cargo fmt --check`
- `cargo test --no-run --quiet`
- `cargo test --quiet`
- `cargo clippy -- -D warnings`
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
- final allowlist audit against the Kotlin/Core migration baseline

Ordinary `cargo test` remains self-contained and does not require the local Kotlin source checkouts. Checkout identity, source paths, headings/anchors, line ranges, changelog bullets, and the Language Guide TOC are verified only by the explicit ignored audit tests.

## Change boundary

The migration changes only:

- `src/kotlin_spec_tests/**`;
- `tests/kotlin_spec/**`;
- this report.

The untracked local source checkouts, historical `kotlin-spec-audit-pr.md`, `GOAL.md`, and other user-owned paths are not part of the migration commits. There are no changes to production behavior, `Cargo.toml`, the lockfile, parser dependencies, build scripts, or runtime configuration.
