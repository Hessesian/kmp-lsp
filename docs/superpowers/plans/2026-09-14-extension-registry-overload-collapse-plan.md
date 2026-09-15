# Extension-registry Gap cluster — verified diagnosis, design, and implementation plan (draft 2)

> **Draft 2, 2026-09-14.** Supersedes `extension-registry-gap-plan-draft1.md`, which is kept for the
> record. This revision incorporates an independent adversarial critique
> (`extension-registry-gap-plan-critique1.md`): 2 blocking issues resolved with explicit decisions,
> 4 should-fix items incorporated, 5 minors applied. Everything the critique **confirmed** (C1–C11)
> is retained unchanged; a "what changed" log is at the very end.
>
> Written against `/home/ocel/Work/lsp/.worktrees/extension-supertype-variable-receiver`
> (tip `934e6fc1`, main after PRs #314/#317/#318/#319/#320). Measured against the real Moneta corpus
> at `/home/ocel/Work/Moneta/android`.
>
> **Every claim marked VERIFIED was reproduced by running real code** — a throwaway `#[test]` in
> `src/resolver/tests.rs`, or a throwaway probe block in `src/cli/hover.rs` driving the exact
> `classify_cursor` → `resolve_identity_with_io(index_only = true)` path the `resolution-accuracy`
> benchmark uses, with the benchmark's own index setup (`build_index(root, /* no_stdlib */ true)` +
> `index_jars` + `JarPhase::Ready`). All scratch code has been deleted.

---

## Part 0 — Phase 1 findings: what the pre-scout hypothesis got wrong

The context file's hypothesis was: *`extension_by_receiver` cannot match a concrete receiver (`List`)
against an extension keyed on a stdlib interface it implements (`Iterable`), because
`kotlin.collections.List` has no indexed declaration to walk from.*

**That hypothesis is false in every one of its three links.** Each link was tested directly, and the
critique independently re-derived all three.

### Q1 — Is `KOTLIN_BUILTIN_TYPE_PLATFORM_EQUIVALENTS` consulted on the `resolve_symbol` path?

**VERIFIED: yes, on every IO policy the benchmark uses.**
`resolve_kotlin_builtin_type_platform_equivalent` is the tail fallback of `resolve_chain` itself
(`src/resolver/resolve.rs:421` Full/rg, `:459` NoRg, `:470` IndexOnly, `:481`
HierarchyAmbiguitySafe; `ScopedOnly` deliberately excluded and documented as such).
`type_path_anchors` (`src/resolver/qualified.rs:186`) resolves its root through
`resolve_symbol_index_only` / `resolve_symbol`, so it inherits that fallback. It is **not**
hover/completion-only.

Measured on the real corpus:

```
resolve_symbol_index_only("String")       -> …/android-36.1/java/lang/String.java:142
resolve_symbol_index_only("CharSequence") -> …/android-36.1/java/lang/CharSequence.java:58
resolve_symbol_index_only("Iterable")     -> …/android-36.1/java/lang/Iterable.java:41
resolve_symbol_index_only("List")         -> jar:…/kotlin-stdlib-2.4.10.jar:2787
```

### Q2 — Does the `List` → `Iterable` supertype walk actually find an `"Iterable"`-keyed extension?

**VERIFIED: yes, end to end, through the benchmark's own path.** A throwaway test indexed
`java/util/List.java` → `java/util/Collection.java` → `java/lang/Iterable.java` as real fixture files,
registered `fun <T> Iterable<T>.firstOrNull(): T?`, and drove `classify_cursor` +
`resolve_identity_with_io(index_only = true)` on
`fun use(countries: List<CodeItem>) { countries.firstOrNull() }`:

```
classify_cursor  = Reference { receiver_type: Some("List"), is_call: true, shape: arg_count 0 }
resolve_identity = CstResolved([…/Extensions.kt:1])
```

Registry matching, the platform-type fallback, generic-arg stripping (`List<CodeItem>` → `"List"`)
and the supertype walk **all already work**. There is no missing infrastructure to build.

### Q3 — What does `super_names_for_class` return, and does it match the registry's bare-name keys?

**VERIFIED: the walk hands the collect-fn a bare simple name — exactly the registry's key shape.**
`super_names_for_class` (`src/resolver/hierarchy.rs:290`) returns the raw source spelling, which may
be dotted; `supertype_targets` normalizes to `super_leaf = super_name.last_segment()`
(`src/resolver/hierarchy.rs:209`, yielded at `:260`/`:284`), and the collect closure at
`src/resolver/qualified.rs:732` receives that leaf. Dumped from the real corpus:

```
supers java.lang.String: class_line=Some(142)
  [(142,"java.io.Serializable"),(142,"Comparable"),(142,"CharSequence"),
   (142,"Constable"),(142,"ConstantDesc"),(1933,"Comparator"),(1933,"java.io.Serializable")]
supers java.util.List:   class_line=Some(137)
  [(137,"SequencedCollection"),(137,"Collection")]
```

The `class_line` filter matches, the names are right. No key-shape mismatch exists.

### So why does `firstOrNull` still Gap on Moneta? — the real root cause

Probing the actual failing site
(`core/common/src/main/java/cz/moneta/smartbanka/common/extensions/PersonalAddress.kt:20`,
`countries.firstOrNull { it.code == country }`):

```
classify_cursor       = Reference { receiver_type: Some("List"), shape: arg_count 0, trailing_lambda: true }
qualified_unfiltered  = [jar:…/kotlin-stdlib-2.4.10.jar:2887]      <- exactly ONE location
ext["List"]["firstOrNull"]     = ["fun <T> List<T>.firstOrNull(): T?"]
ext["Iterable"]["firstOrNull"] = ["fun <T> Iterable<T>.firstOrNull(): T?",
                                  "fun <T> Iterable<T>.firstOrNull(predicate: (T) -> Boolean): T?"]
resolve_identity      = NameScan([])                                <- Gap
```

**VERIFIED ROOT CAUSE (Bug 1): the extension tiers collapse an overload set to one arbitrary
candidate.** The call has a trailing lambda, so its `CallShape` arity is 1. The qualified lookup
returns only the **0-arg** `List<T>.firstOrNull()` — the first entry under the own-type key. The
call-shape filter in `resolve_identity_with_io` then correctly rejects it, the result empties, and the
reference lands in Gap. The applicable 1-arg `Iterable<T>.firstOrNull(predicate)` is in the registry
and is independently reachable
(`find_definition_qualified_index_only("firstOrNull", Some("Iterable"))` resolves it), but no code
path can surface it, because **five** places take the first candidate and throw the rest away:

| # | Site | What it drops | Task |
|---|---|---|---|
| 1 | `src/resolver/extension.rs:69` — `resolve_extension_in_scope`'s `return vec![Location{…}]` inside the entry loop | every overload after the first in-scope match, despite the `-> Vec<Location>` signature | 1 |
| 2 | `src/resolver/qualified.rs:402` — `own_type_extension … .into_iter().next()` | the own-type tier narrowed to `Option<Location>` | 1 |
| 3 | `src/resolver/qualified.rs:433` and `:739` — `supertype_extension … .into_iter().next()`, plus `resolve_extension_via_supertype_hierarchy`'s own `matches.into_iter().next().into_iter().collect()` | every supertype-tier overload after the first | 1 |
| 4 | `src/resolver/qualified.rs:539` — `jar_extension_for_type_root`'s identical `return vec![Location{…}]` | every overload on the **last-resort type-root path**, i.e. exactly the built-in/uncompiled receivers this cluster is about | 1 (**added in draft 2 — critique S4.1**) |
| 5 | `src/resolver/extension.rs:151-163` — `implicit_receiver_extension_match` picks the **first** symbol matching `extension_declaration_matches`, then shape-checks only that one | an implicit-receiver call of the right arity, when the declaring file lists a different-arity overload first | **deferred, Task 5 (named, critique S4.2)** |

A sixth, same-family defect rides along at sites 1 and 4: even when the right *entry* is selected, its
`Location` **range** is chosen by `.find(|symbol| extension_declaration_matches(symbol, name,
receiver_base, entry.container))`. `extension_declaration_matches` (`src/resolver/infer.rs:1857-1866`)
compares only `(name, extension_receiver, container)` — identical for every overload — so all
overloads of one receiver resolve to the **same** range, the first one in the declaring file.

This is precisely the bug class PR #304 fixed for the **member** tier
(`find_all_names_scoped_to_container` returns every same-name candidate rather than `.find()`'s first).
The extension tiers were never given the same treatment. `docs/architecture/unified-resolution-strategy.md`
already names this as "root cause #2"; this cluster is its recurrence in a second registry.

### Second verified bug (Bug 2): JAR extension keys are not normalized the way source keys are

```
nullable-suffixed ext keys: count=54
  sample: ["Long?","Double?","A?","JavaType?","Any?","String?","T?","File?", …]
ext["String?"]["orEmpty"]  = ["fun String?.orEmpty(): String"]
ext["String"]["orEmpty"]   = []
find_definition_qualified_index_only("orEmpty", Some("String")) = []
```

The source-side key derivation (`parser::extension_receiver_from_decl`, `src/parser.rs:1362-1396`)
performs **three** normalizations: strip generics (`split('<')`), unwrap `nullable_type → user_type`,
and take the dotted leaf (`rsplit('.')`). The **JAR**-side derivation performs only the first, at both
`src/indexer/jar.rs:932-937` (Tier-2 `build_jar_file_data`) and `src/indexer/jar.rs:1499-1506`
(Tier-1 manifest). A JAR extension whose receiver is nullable, or is a nested type, is therefore filed
under a key **no lookup can ever produce** — every read site anchors on a bare, `strip_nullable()`d
`class_name`. 54 keys on the Moneta corpus are unreachable for the nullability reason alone.

Two normalizations are missing, not one:

| rendered receiver | key today | key required |
|---|---|---|
| `String?` | `"String?"` ❌ | `"String"` |
| `Outer.Inner` (kotlinx-metadata renders nested types dot-separated, `KotlinClassIndexer.kt`'s `substringAfterLast('/')`) | `"Outer.Inner"` ❌ | `"Inner"` |
| `List<String>?` | `"List"` ✅ already correct — the `?` sits after the `>` and the split discards it | `"List"` |
| `Map<String, Int>` | `"Map"` ✅ | `"Map"` |

This fully explains `orEmpty` (63 Gap occurrences).

### Third observation, verified-real but NOT root-caused (Bug 3, diagnosis task only)

```
find_definition_qualified_index_only("isNotEmpty", Some("CharSequence")) -> jar:…kotlin-stdlib…:6750
find_definition_qualified_index_only("isNotEmpty", Some("String"))       -> []
find_definition_qualified_index_only("forEach",    Some("Iterable"))     -> resolves
find_definition_qualified_index_only("forEach",    Some("List"))         -> []
```

`String` resolves to a real `java/lang/String.java` declaration whose `supers` genuinely contain
`CharSequence` on the matching `class_line`, and the `"CharSequence"`-keyed extension genuinely
resolves when asked for directly — yet the walk from `String` does not reach it. Same shape for
`List` → `Iterable`. **The hop that dead-ends has not been isolated**, so no fix is designed for it
here. (This is *not* the original hypothesis: the walk works in a controlled fixture, so the cause is
corpus-specific.) The critique agrees with deferring it and adds — correctly — that Bug 3 probably
subsumes more Gap names than draft 1 credited it with, making it likely the **largest** of the three,
not a leftover.

### Q4 / Scout-4 reclassification — every row re-probed, including the two draft 1 could not classify

Draft 1 excluded `#9 filter` and `#6 toInt` as "citations do not resolve". The critique correctly
called that an enumeration trap: the corpus has **five** `StringExtensions.kt` (draft 1 checked the
26-line one; the 141-line one has line 88) and **thirteen** `SetupInteractor.kt`. Both have now been
located across all same-named files and probed. **The conclusion survives, but on real evidence
rather than on a bad citation.**

| # | Name | Site | `receiver_type` the CST actually produced | Verified classification |
|---|---|---|---|---|
| 3 | `firstOrNull` | PersonalAddress.kt:20 | `List` ✅ correct | **(a) IN SCOPE — Bug 1** (overload collapse) |
| 12 | `orEmpty` | Account.kt:208 | `String` ✅ correct | **(a) IN SCOPE — Bug 2** (key is `"String?"`) |
| 7 | `isNotEmpty` | GitVersionValueSource.kt:21 | `String` ✅ correct | **(a′) Bug 3** — needs the `String`→`CharSequence` walk; diagnosis only |
| 13 | `contains` | CardExtensions.kt:8 | **`CardRight`** ❌ | **(c) OUT — receiver-inference name collision.** `Card.rights` is a **Java** field `@Nullable public List<ContextRight> rights` (`Card.java:18`); the CST returned `CardRight`, the element type of the *sibling* function's `vararg rights: CardRight` parameter three lines below. **Corrected in draft 2 (critique S6): this is NOT #8's vararg bug**, it is a scope-leak of a sibling function's parameter over a receiver's field. Evidence moves to Task 3's list. |
| 8 | `forEach` | CardExtensions.kt:12 | **`CardRight`** ❌ (receiver *is* `vararg rights: CardRight`, i.e. `Array<out CardRight>`) | **(c) OUT — vararg receiver-type inference drops the `Array<out …>` wrapper.** Confirmed by the critique. |
| 9 | `filter` | **core/common/**…/extensions/StringExtensions.kt:88 (the 141-line file) — `split.getOrNull(1)?.filter { it.isDigit() }` | **`T`** ❌ | **(c) OUT — un-substituted generic return type.** `getOrNull` is `fun <T> List<T>.getOrNull(index: Int): T?`; its return type reached the anchor as the literal type parameter `T`. Same family as PR #318's gap, one hop later in the chain. `ext["T"]["filter"]` is empty; `ext["CharSequence"]["filter"]` holds the real declaration, unreachable from key `"T"`. **Re-classified in draft 2 (critique S5).** |
| 6 | `toInt` | **core/common_screen/**…/variant/setup/SetupInteractor.kt:16 — `termMax?.toInt()` | **`Int`** ✅ plausibly correct | **(c) OUT — Kotlin primitive synthetic member.** `Int.toInt()` is a *member* of `kotlin.Int`, not an extension; `ext["Int"]["toInt"]` is empty and the platform table maps `Int → java.lang.Integer`, which declares `intValue()`, not `toInt()`. A third, distinct bucket: primitive-type members. **Re-classified in draft 2 (critique S5).** |
| 5 | `toString` | VersionCatalog.kt:7 | **`TestLoggedComponent`** ❌ (should be `VersionConstraint`) | **(c) OUT — chain return-type inference picked an unrelated type.** |
| 10 | `add` | ObjectMapper.kt:28 | **`OperationResponseBody`** ❌ (should be `Moshi.Builder`) | **(c) OUT — builder-chain receiver inference.** |
| 11 | `map` | IncomeSourcesStaticDtoMapper.kt:23 | **`TransformedText`** ❌ (should be `List<IIncomeSourceType>`) | **(c) OUT — implicit-extension-receiver chain inference.** |
| 1 | `text` | RiskSurveyDtoMapper.kt:87 | **`String`** ❌ (should be `CodeItem?`) | **(c) OUT — downstream of #2's `.required()` failing.** |
| 4 | `run` | IterableExtensions.kt:18 | `CoroutineScope` ✅ correct, but `ext["CoroutineScope"]["run"]` is empty | **(b) OUT — `kotlin.run` is `fun <T,R> T.run(…)`, keyed `"T"`. PR #318's bare-generic-receiver gap.** |
| 2 | `title` | InitUseCase.kt:56 | not re-probed (ambiguous filename; classification taken from the signature) | **(b) OUT — `fun <T> T?.required()`, keyed `"T"`. PR #318's gap.** |
| 14 | `finish` | DirectDebitVictoryScreen.kt:46 | not re-probed (same) | **(b) OUT — `val <T> CompositionLocal<T>.current` keyed `"T"`. PR #318's gap.** |

**Net, now fully enumerated: of the 14 names in the hypothesised cluster, exactly 2 are in this
plan's fix scope (`firstOrNull`, `orEmpty`), 1 is in its diagnosis scope (`isNotEmpty`), 3 belong to
PR #318, and 8 are receiver-type-inference or primitive-member bugs with nothing to do with the
extension registry.** The "exactly 2" headline held under re-enumeration; it is no longer resting on
an unresolved citation.

> One negative result worth recording: the critique suggested
> `StringExtensions.kt:89`'s trailing `integerPart.orEmpty()` as independent Bug-2 corroboration.
> Probed: `classify_cursor` returns `receiver_type: None` there, so it never enters the member bucket
> at all. `Account.kt:208` remains the single verified Bug-2 instance. Not claimed.

---

## Part 1 — Design

### Scope, stated plainly

**This is "fix two narrow defects in existing lookups", not "build supertype-synthesis
infrastructure".** No new table, no synthetic hierarchy edges, no `platform_types.rs` change, no
`hierarchy.rs` change, no sidecar change. Expected production diff: under 120 lines across four files.

The suggestion that a synthetic Kotlin-stdlib type-hierarchy edge table might be needed is
**rejected**: the real edges already exist, are already indexed from real Android SDK `.java`
sources, and the walk over them already works (Q2).

### Fix 1 — Extension lookups return every applicable overload

Mirror what PR #304 did for members. Four coordinated changes:

1. **`resolve_extension_in_scope` (`src/resolver/extension.rs:20-74`)** — accumulate instead of
   returning on the first in-scope entry. The signature is already `Vec<Location>`; only the body
   lies about it.
2. **`jar_extension_for_type_root` (`src/resolver/qualified.rs:505-542`)** — the identical fix on the
   last-resort type-root path. *(Added in draft 2 per critique S4.1.)* Not optional: this is the path
   taken when the qualifier's leaf has **no indexed declaration** — precisely the built-in receivers
   this cluster is about — and leaving it collapsed would violate AGENTS.md's "understand root
   causes, not just surface fixes" while also making Task 1's own test 1 fail for the wrong reason
   (see M11 below).
3. **Range disambiguation, at both of the above** — when several symbols in the declaring file satisfy
   `extension_declaration_matches`, prefer the one whose `detail` equals the `ExtensionEntry`'s own
   `detail`, falling back to today's first-match when no exact match exists.
   The critique verified this discriminator is **exact on both derivation paths, not just JAR**:
   `SymbolEntry.detail` and `ExtensionEntry.detail` are the same string by construction — JAR
   (`src/indexer/jar.rs:949` and `:1142`) and source (`src/indexer/apply.rs:219` and `:439`) alike —
   and it empirically produced two locations with two distinct ranges for two source-declared
   overloads. The `.or_else(first)` fallback covers the one tie case: JAR-derived **Java** methods
   can carry a `"(...)"` placeholder detail (the PR #311 shape), inert here because Java declares no
   Kotlin extensions.
   **Do not** widen `extension_declaration_matches` itself — `src/resolver/infer.rs` has other callers
   that do not want overload discrimination.
4. **`QualifiedCandidates` (`src/resolver/qualified.rs:320-349`)** — change
   `own_type_extension: Option<Location>` and `supertype_extension: Option<Location>` to
   `Vec<Location>`, drop the `.into_iter().next()` at `:402` and `:433`, and drop
   `resolve_extension_via_supertype_hierarchy`'s own narrowing at `:739`. `into_precedence_ordered`
   (`:342-348`) already uses `extend`, which accepts `Option` and `Vec` identically, so tier ORDER is
   preserved untouched — verified by the critique, which applied the change and saw every
   ordering/precedence test stay green.

**Blast radius, verified:** `candidates_on` has exactly one call site
(`src/resolver/qualified.rs:111`); `QualifiedCandidates` and `into_precedence_ordered` appear nowhere
else; `resolve_extension_in_scope` has exactly two call sites, both in `qualified.rs`;
`resolve_extension_via_supertype_hierarchy` has one. Completion reaches the registry through
`jar::extension_entries_for` directly and is unaffected. **Hover is affected — see the next section.**

**Why returning several locations is safe for the metric.** `ResolutionAccuracyAggregator::add`
(`src/features/unresolved_symbol_diagnostics.rs:288-300`) counts
`Success { tier: CstResolved, locations }` as recall **regardless of `locations.len()`**, and
`shape_filter_locations` runs *before* that, narrowing to arity-compatible candidates. Giving that
filter something to choose from is the entire point of the fix.

### Decision B1 — `ambiguous_member_extension_name_collision_first_match_wins` is rewritten, deliberately

*(Blocking issue raised by the critique; decided here rather than discovered mid-implementation.)*

`src/resolver/tests.rs:7282` pins today's behaviour: two unrelated interfaces each declaring
`fun Modifier.weight(weight: Float)`, asserting `locs.len() == 1`, with a doc comment naming
*"first match wins, no overload-set semantics"* as a **deliberate, documented limitation**. The
critique applied Task 1's production change and measured the result: **1945 passed, 1 failed, 3
ignored** — this test, and only this test, goes red with `["file:///compose/ColumnScope.kt",
"file:///lib/UnrelatedScope.kt"]`.

**Decision: rewrite the test to assert the overload set, and rename it
`ambiguous_member_extension_name_collision_returns_the_whole_candidate_set`.**

Rationale, stated so it can be argued with:

- Two same-arity `Modifier.weight` from unrelated scopes is a *genuine* ambiguity — the receiver-scope
  information needed to tell them apart (is `ColumnScope` or `UnrelatedScope` the enclosing receiver?)
  does not exist at this tier. Silently picking the first is how goto-definition lands in the wrong
  library.
- It is the same call PR #304 already made for members, and
  `docs/architecture/unified-resolution-strategy.md` names first-match collapse as a root cause to
  remove, not a property to preserve.
- The pinned comment describes the limitation as inherited from `resolve_extension_in_scope`. Once
  that limitation is gone, a test asserting it is asserting the bug.

The rewritten test keeps its decoy value: it must assert **both** locations come back **and** that
they are in declaration order, so it still fails if a future change reintroduces arbitrary narrowing
*or* reorders the tiers.

**The B1 / S4.1 interaction, checked explicitly** (the critique warned that fixing S4.1 "will break
the B1 test harder"): traced through `resolve_qualified` (`src/resolver/qualified.rs:103-131`). In
this fixture `Modifier` has no indexed declaration, so `type_path_anchors` hands back the
declaration-less anchor `{ declaration: None, class_name: "Modifier" }`; `candidates_on` then fills
`own_type_extension` from `resolve_extension_in_scope`, `candidates.is_empty()` is false, and the
function **returns before reaching** `jar_extension_for_type_root` at `:128`. So fix 1 and fix 2 reach
this test through the *same* single path, and the rewritten expectation (2 candidates, declaration
order) is correct under both fixes applied together. **No second version of the conflict is opened.**
Task 1 Step 4 nevertheless re-runs the full suite after *both* sub-fixes, not after each, so the
combined state is what gets measured.

### Decision S3 — the hover regression is real, predicted, and owned rather than silently shipped

*(Should-fix raised by the critique; accepted.)*

`pick_unambiguous_location` (`src/features/hover.rs:134-145`) returns `None` whenever more than one
candidate survives shape filtering. Today the extension tiers hand it exactly one location, so hover
always renders. After Task 1, a receiver with two in-scope **same-arity** same-named extensions loses
hover entirely — including the exact `Modifier.weight` shape from the B1 test, a hot Compose pattern.

This is invisible to `resolution-accuracy` and must not be discovered by a user.

**Decision: ship it, verify it, and name the reversal in advance.**

- It is philosophically consistent with PR #304's own comment in that same function: showing one
  arbitrary signature for a genuinely ambiguous name is worse than showing none.
- Task 1 Step 6 adds an explicit hover check on a colliding-extension shape, so the change is
  *observed*, not assumed.
- The PR description must state the trade in one sentence.
- **Pre-designed reversal, if it proves unacceptable:** loosen `pick_unambiguous_location` to accept a
  candidate set whose members all share the same name *and* the same arity, returning the first — a
  ~3-line change confined to `src/features/hover.rs` that restores hover **without** re-collapsing
  resolution. This is deliberately *not* bundled into Task 1: it is an unmeasured hover-behaviour
  change, and mixing it into a resolver PR would make the measurement ambiguous. It is Task 4.

### Fix 2 — Align JAR extension-receiver keys with the parser's normalization

One helper in `src/indexer/jar.rs`, used at both derivation sites so the two can never drift.
Per critique M7 it reuses the existing `StrExt::strip_nullable` (`src/str_ext.rs:86-88`) rather than
reinventing `trim_end_matches('?')`, and the doc comment states the *whole* normalization instead of
the false "mirrors the parser" claim draft 1 made while omitting the dotted-leaf strip:

```rust
use crate::str_ext::StrExt;   // both call sites need this import

/// The `extension_by_receiver` key for a sidecar-rendered receiver type.
///
/// Applies the same three normalizations `parser::extension_receiver_from_decl`
/// applies on the source side, because both feed the SAME map and every read
/// site anchors on a bare, `strip_nullable()`d leaf name: generic arguments,
/// nullability, and the dotted leaf are all view-level decoration on the
/// receiver's base type. `fun String?.orEmpty()` is callable on a `String`
/// receiver; kotlinx-metadata renders a nested receiver as `Outer.Inner`
/// (`KotlinClassIndexer.kt`'s `substringAfterLast('/')`) while the lookup only
/// ever produces `Inner`. A key carrying either decoration is unreachable by
/// construction.
fn extension_receiver_key(rendered_receiver_type: &str) -> &str {
    rendered_receiver_type
        .split('<')
        .next()
        .unwrap_or("")
        .strip_nullable()
        .last_segment()
}
```

*(Both helpers are existing `StrExt` methods — `strip_nullable` at `src/str_ext.rs:86-88`
(`trim_end_matches('?')`) and `last_segment` at `:76-78`
(`self.rsplit('.').next().unwrap_or(self)`, i.e. a dotless string passes through unchanged).
Verified, not assumed.)*

Call it at `src/indexer/jar.rs:932-937` (Tier 2) and `src/indexer/jar.rs:1499-1506` (Tier 1).

**Why a single derivation point makes this safe** (critique C6, which draft 1 never stated): the
`extension_receiver` local at `:932-937` feeds **both** `pack_cold_fields(...)` →
`SymbolEntry.extension_receiver()` (`:968`) **and** the registry key at `:1136`. Normalizing once
keeps `extension_declaration_matches`'s `symbol.extension_receiver() == receiver_base` comparison
(`src/resolver/infer.rs:1866`) matching, so the range lookup does not silently degrade to
`unwrap_or_default()` (0:0).

**Cache-version bump is mandatory.** `JarManifestName.extension_receiver`
(`src/indexer/jar_manifest_cache.rs:77`) persists the old, un-normalized key, so
`JAR_MANIFEST_CACHE_VERSION` (`:33`) goes `3 → 4`. The main `index.bin` (`CACHE_VERSION = 32`) does
**not** persist JAR `FileData` and needs no bump. This project has been burned once by a stale cache
masking a correct fix (PR #298–300 memo).

**Which tier the benchmark exercises.** Measured: on the eager CLI pipeline `jar_extension_receivers`
has **0 keys** while `extension_by_receiver` has 2528 — the eager `index_jars` materializes every JAR,
so Tier 1's promotion gate is bypassed and the benchmark only proves the Tier-2 site. The Tier-1 site
must still be fixed in the same change or the LSP server's lazy path keeps the bug; it simply cannot
be measured by `resolution-accuracy`.

### Explicitly excluded, with reasons

- **`forEach`** — receiver of a `vararg` parameter is inferred as the element type. Receiver-inference
  bug in `CstQuery::expr_type`; the registry is never consulted with a usable key.
- **`contains`** — receiver-inference **name collision**: a sibling function's `vararg rights:
  CardRight` shadowed the receiver's own Java field `List<ContextRight> rights`. Distinct from
  `forEach`'s vararg bug; evidence listed under Task 3.
- **`toString`, `add`, `map`, `text`** — four separate chain-inference bugs producing demonstrably
  unrelated receiver types.
- **`filter`** — the receiver type is the literal, un-substituted type parameter `T` returned by
  `List<T>.getOrNull`. Generic-substitution gap, PR #318's family.
- **`toInt`** — `Int.toInt()` is a primitive *member*, not an extension; a distinct "Kotlin primitive
  synthetic member" bucket.
- **`run`, `title`, `finish`** — the extension's declared receiver **is** the function's own generic
  type parameter, so the registry key is the literal `"T"`. PR #318's documented gap.
  **Decision on the context file's open question 4: leave it out.** It needs its own plan, built on
  `type_subst::is_generic_param` and the declaration's type-parameter list, and is larger than both
  fixes here combined. Folding it in would mix two unrelated mechanisms in one PR.
- **`implicit_receiver_extension_match` (`src/resolver/extension.rs:144-168`)** — same first-match
  bug, on the implicit-receiver path (`resolve_implicit_receiver_callee`), compounded with an arity
  filter applied to the single picked symbol. **Named, deferred to Task 5**: it is a genuinely
  separate entry point with its own callers, no measured Gap name currently points at it, and
  bundling it would widen Task 1's blast radius past what one measurement can attribute.
- **`weight`, `scenes`, `launch`, `await`, `detail`, `finishAffinity`** — already flagged as separate
  issues by the context file; not re-examined.

### Expected effect, stated as a falsifiable prediction

- Task 1: `firstOrNull` (125 occurrences) leaves the member-ref Gap top-20.
- Task 2: `orEmpty` (63 occurrences) leaves the member-ref Gap top-20.
- Aggregate recall movement will be ~+0.5–1.0pp at most — **within, or barely outside, the documented
  ±0.3–0.6pp run-to-run noise.** Judge each task by whether its named Gap entry disappears, not by the
  percentage.

---

## Part 2 — Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: use `superpowers:test-driven-development` for each task
> and `superpowers:executing-plans` to run them in order. Each task is independently testable,
> independently revertable, and ships as its own PR. Steps use checkbox (`- [ ]`) syntax.
>
> House rules for every task (AGENTS.md): never commit to `main`; `cargo test` and
> `cargo clippy -- -D warnings` clean after every change; `cargo fmt` before committing; no
> abbreviated names; use Serena symbolic tools, calling `mcp__serena__activate_project` with **this
> worktree's** path first.
>
> **Line numbers below are against tip `934e6fc1`** and were re-verified by the critique. Navigate by
> symbol name, not by line, if they have drifted.

---

### Task 1: Extension lookups return every overload, not the first

**Why this is first:** it is the only Gap entry in the cluster whose failure was reproduced end-to-end
through the benchmark's own code path, the diff is confined to two production files, and it needs no
cache bump.

**Files:**
- Modify: `src/resolver/extension.rs` (`resolve_extension_in_scope`, `:20-74`)
- Modify: `src/resolver/qualified.rs` (`QualifiedCandidates` `:320-349`; `candidates_on` `:402`/`:433`;
  `resolve_extension_via_supertype_hierarchy` `:739`; `jar_extension_for_type_root` `:505-542`)
- Modify: `src/resolver/tests.rs` (4 new tests, **1 existing test rewritten**)

**Interfaces:** `resolve_extension_in_scope` and `jar_extension_for_type_root` keep their signatures
(both already return `Vec<Location>`). `QualifiedCandidates`' two extension fields change from
`Option<Location>` to `Vec<Location>`; the struct is `pub(super)` with private fields, so nothing
outside `qualified.rs` sees it.

- [ ] **Step 1: Write the failing tests**

Add to `src/resolver/tests.rs`.

> **Fixture requirements — read before copying any template (critique M11).** The nearest template
> (`resolve_kotlin_builtin_type_platform_equivalent_surfaces_iterable_extensions_for_a_list_receiver`,
> `src/resolver/tests.rs:9523+`) hand-pushes an `ExtensionEntry` whose `file_uri` is
> `file:///app/Extensions.kt` — **a URI that is never indexed**. With that shape the range lookup finds
> no `FileData`, falls to `unwrap_or_default()`, and *every* candidate gets range `0:0`, so
> "two distinct ranges" fails even after a correct fix. Tests 1 and 2 below **must** index a real
> declaring file that literally declares both overloads (`idx.index_content(&ext_uri, …)`), not a
> synthetic entry pointing at a phantom URI. Reuse only the template's
> `write_fake_android_sdk_source` scaffolding for the `java/util/List.java` →
> `java/util/Collection.java` → `java/lang/Iterable.java` chain.
>
> Second trap: test 1 calls `find_definition_qualified_index_only("firstOrNull", Some("List"), …)`.
> If the fixture does not give `List` an indexed declaration, `anchors_for` still yields a
> declaration-less anchor and the ladder still runs — but if it yielded nothing, the call would fall
> through to `jar_extension_for_type_root`. Since that path is **also** fixed in this task (fix 2),
> the test is correct either way; the `java/util/List.java` fixture is still required for test 2's
> supertype walk.

1. `own_type_extension_tier_returns_every_overload_not_just_the_first` — a real indexed file
   declaring both `fun <T> List<T>.firstOrNull(): T?` and
   `fun <T> List<T>.firstOrNull(predicate: (T) -> Boolean): T?`, in that order. Call
   `find_definition_qualified_index_only("firstOrNull", Some("List"), caller)` and assert **two
   locations with two distinct ranges**. Red today on both counts.
2. `supertype_extension_tier_returns_every_overload_not_just_the_first` — the same two overloads
   declared on `Iterable`, reached from a `List` receiver through the real
   `List → Collection → Iterable` chain. Asserts the walk's narrowing is lifted too, not only the
   own-type tier's.
3. `trailing_lambda_call_resolves_the_predicate_overload_of_a_stdlib_extension` — the **real Moneta
   shape**, driven through `classify_cursor` + `resolve_identity_with_io(…, index_only = true)` on
   `fun use(countries: List<CodeItem>) { countries.firstOrNull { it.code == "x" } }`, with the 0-arg
   overload keyed `"List"` and the 1-arg keyed `"Iterable"` — exactly the corpus layout measured in
   Part 0. Assert `CstResolved` **and** that the resolved location is the **1-arg** declaration. The
   critique independently verified this test cannot pass trivially: `candidates_on` computes both
   tiers unconditionally, `into_precedence_ordered` yields `[0-arg, 1-arg]`, and
   `shape_filter_locations` narrows to the 1-arg one — impossible today because `supertype_extension`
   is an `Option`.
4. `a_real_member_still_outranks_an_extension_overload_set` — decoy guard. The receiver class declares
   a real, arity-compatible member of the same name **and** the registry holds two extension
   overloads. Assert the member comes back **first**. (Confirmed by the critique to pass both before
   and after — it is a guard, not a red test.)
5. `jar_extension_for_type_root_returns_every_overload` — the last-resort path in isolation: a type
   root with **no** indexed declaration and two same-named JAR-keyed overloads. Assert both come back
   with distinct ranges. *(New in draft 2, covers fix 2.)*

- [ ] **Step 2: Rewrite the pinned first-match test — decision B1**

Rewrite `ambiguous_member_extension_name_collision_first_match_wins`
(`src/resolver/tests.rs:7282`, assertion at `:7317`) as
`ambiguous_member_extension_name_collision_returns_the_whole_candidate_set`:

- assert `locs.len() == 2`;
- assert declaration order (`ColumnScope.kt` before `UnrelatedScope.kt`), so the test still fails if a
  future change reorders tiers or reintroduces arbitrary narrowing;
- replace the doc comment at `:7276-7280`. It currently documents the limitation as intended; it must
  now document that colliding same-arity member extensions are surfaced **as a candidate set**, that
  this is the PR #304 precedent applied to extensions, and that
  `pick_unambiguous_location` consequently declines hover for this shape (cross-reference Task 4).

Do not delete the test. Its fixture is a genuine Compose-shaped decoy and is worth keeping.

- [ ] **Step 3: Verify the tests fail (and only the expected ones)**

```
cargo test --bin kmp-lsp own_type_extension_tier_returns_every_overload        # FAIL: 1 location
cargo test --bin kmp-lsp supertype_extension_tier_returns_every_overload       # FAIL: 1 location
cargo test --bin kmp-lsp trailing_lambda_call_resolves_the_predicate_overload  # FAIL: NameScan([])
cargo test --bin kmp-lsp jar_extension_for_type_root_returns_every_overload    # FAIL: 1 location
cargo test --bin kmp-lsp a_real_member_still_outranks_an_extension_overload    # PASS (guard)
cargo test --bin kmp-lsp ambiguous_member_extension_name_collision             # FAIL: expects 2, gets 1
```

> **Red-before-green discipline (this session has been burned twice).** Each red test must fail for
> the *right* reason — an empty/`NameScan`/one-location result, not a panic or fixture error. Read the
> failure text, don't just read "FAILED". If test 3 passes before the fix, the fixture is leaking
> through a global bare-name fallback; add a decoy same-named declaration in an unrelated file until
> it fails. **Do not** apply the draft-1 version of this rule to a test the critique already proved is
> green-before-fix — see Task 2's note.

- [ ] **Step 4: Implement — `resolve_extension_in_scope` and `jar_extension_for_type_root`**

In `src/resolver/extension.rs`, replace the early `return vec![Location { uri, range }]` inside the
entry loop with accumulation into a `Vec<Location>` returned after the loop. Apply the identical
change to `jar_extension_for_type_root` (`src/resolver/qualified.rs:539`).

For the range, replace the single `.find(…)` with a two-step selection in named locals
(AGENTS.md: explicit over clever — no chained-combinator soup), extracted into one shared helper used
by both sites rather than duplicated:

```rust
let declaring_symbols = /* file_data.symbols matching extension_declaration_matches(...) */;
let exact_signature_match = declaring_symbols
    .iter()
    .find(|symbol| symbol.detail == entry.detail);
let selected = exact_signature_match.or_else(|| declaring_symbols.first());
```

Keep `extension_declaration_matches` itself untouched.

- [ ] **Step 5: Implement — `qualified.rs` tiers**

Change `QualifiedCandidates::own_type_extension` and `::supertype_extension` to `Vec<Location>`;
update `is_empty()` to use `.is_empty()` on both; `into_precedence_ordered` keeps its two `extend`
calls unchanged. Delete the `.into_iter().next()` at `:402` and at `:433`. In
`resolve_extension_via_supertype_hierarchy` (`:739`) return `matches` directly and update its doc
comment: the breadth-first **nearest-level** stop
(`src/resolver/hierarchy.rs:104-118`, `if !found.is_empty() { return found; }`) is retained and is
what still prevents a farther ancestor outranking a nearer one; only the one-of-many narrowing is
gone, because a same-name ancestor extension set is an **overload set** the caller's shape filter must
see in full. Cite PR #304's member-tier precedent.

- [ ] **Step 6: Verify**

```
cargo test --bin kmp-lsp   # expect: 1945 pre-existing green + 5 new + 1 rewritten = all green
cargo clippy -- -D warnings
cargo fmt --check
```

Run the suite **after both Step 4 and Step 5 are applied**, not after each — the combined state is
what B1's rewritten expectation was reasoned about.

- [ ] **Step 7: Verify the predicted hover change — decision S3**

Explicitly observe, don't assume:

```
cargo build
cd /home/ocel/Work/Moneta/android
# 1. a colliding same-arity shape (the B1 pattern) — expect hover to DECLINE after this change
kmp-lsp hover <a file with a Modifier.weight call> <line> <col>
# 2. the fixed shape — expect hover to RENDER the 1-arg overload
kmp-lsp hover core/common/src/main/java/cz/moneta/smartbanka/common/extensions/PersonalAddress.kt 20 27
```

Record both outcomes in the PR description. If case 1's loss is judged unacceptable, do **not** patch
it here — open Task 4.

- [ ] **Step 8: Measure on the real corpus**

```
cargo build --release
./target/release/kmp-lsp resolution-accuracy /home/ocel/Work/Moneta/android
```

Record: member recall before/after, **whether `firstOrNull` left the member-ref Gap top-20**, and
whether `filtered_candidate_total` moved. The success criterion is `firstOrNull` leaving the list.

If `filtered_candidate_total` rises materially, the sibling-ancestor tie-break matters after all.
**Contingency, and it is not a one-liner (critique M10):** narrowing
`resolve_extension_via_supertype_hierarchy` to the overloads of the *first matching ancestor name*
requires `walk_hierarchy_breadth_first` (`src/resolver/hierarchy.rs:104-115`) to stop flattening —
today it does `found.extend(collect(idx, &super_name, &super_uri, caller))` and no ancestor label
survives. The collect closure's element type must become e.g. `(String, Location)` and the one other
`walk_hierarchy_breadth_first` caller's generics adjusted. **Budget half a day for this contingency;
do not attempt it under measurement pressure.**

- [ ] **Step 9: Commit and open a PR**

The PR description must state, in its own words: (a) the overload-set change, (b) that
`ambiguous_member_extension_name_collision_*` was rewritten because it pinned the bug, (c) the hover
trade from Step 7 and that Task 4 is the reversal if needed.

```bash
git commit -m "fix(resolve): surface every extension overload, not just the first match

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01L7ZonwYUh94VsuQphiHykF"
```

---

### Task 2: Align JAR extension-receiver keys with the parser's normalization

**Independent of Task 1** — different files, different mechanism. Ordered second only because Task 1's
measurement is the larger one.

**Files:**
- Modify: `src/indexer/jar.rs` (new `extension_receiver_key` helper; call sites `:932-937`, `:1499-1506`)
- Modify: `src/indexer/jar_manifest_cache.rs` (`JAR_MANIFEST_CACHE_VERSION`, `:33`)
- Modify: `src/indexer/jar_tests.rs` (new tests)

**Interfaces:** internal to `src/indexer/`. No resolver change.

- [ ] **Step 1: Write the failing tests**

In `src/indexer/jar_tests.rs`, next to the existing `extension_entries_for` fixture at `:3023`.

1. `jar_nullable_receiver_extension_is_keyed_on_the_base_type` — feed a synthetic
   `SidecarSymbol { name: "orEmpty", extension_receiver_type: "String?",
   detail: "fun String?.orEmpty(): String", … }` through the Tier-2 materialization path; assert
   `extension_by_receiver` holds it under `"String"` and **not** `"String?"`. **Red today.**
2. `jar_nested_type_receiver_extension_is_keyed_on_the_leaf_type` — receiver `"Outer.Inner"` → key
   `"Inner"`. Red today (keys as `"Outer.Inner"`), and it is the second real normalization gap: the
   sidecar's `KmType.render` does `substringAfterLast('/')`, which strips the package but keeps
   dot-separated nesting, while every lookup site produces a bare leaf.
   *(Replaces draft 1's test #2 — see the note below.)*
3. `jar_manifest_tier1_receiver_key_matches_tier2` — the same two assertions against
   `jar_extension_receivers` via the Tier-1 manifest path, so the second call site has its own test
   rather than being covered by inspection. `write_versioned_manifest_cache_for_test`
   (`src/indexer/jar_manifest_cache.rs:96`) is the scaffolding.
4. `jar_manifest_cache_ignores_a_stale_v3_version` — write a v3 manifest with
   `write_versioned_manifest_cache_for_test(3, …)`, load, assert it is discarded. *(Critique M9: this
   near-duplicates `jar_manifest_cache_ignores_a_stale_pre_field_extraction_version` at
   `src/indexer/jar_manifest_cache_tests.rs:74`, which hardcodes `2`. The mechanism is already
   covered; this pins the new constant. That existing test's own doc comment argues for a hardcoded
   sibling per bump, so keep it — but know it is a constant-pin, not a mechanism test.)*

> **Draft 1's test #2 was deleted, not weakened (blocking issue B2).** It asserted that
> `"List<String>?"` keys as `"List?"` today. It does not — the trailing `?` sits *after* the `>`, in
> the second `split('<')` fragment, and is discarded by the split itself. Measured:
> `"String?" → "String?"` (broken), `"List<String>?" → "List"` (already correct),
> `"Map<String, Int>?" → "Map"` (already correct). The test was green before the fix and its stated
> rationale ("proves the two strips compose in the right order") was false — no ordering of the two
> strips gets it wrong in either direction. Under draft 1's own red-before-green rule an implementer
> would have burned real time hunting a phantom fixture leak. Test 2 above replaces it with a shape
> that is genuinely red.

- [ ] **Step 2: Verify they fail**

```
cargo test --bin kmp-lsp jar_nullable_receiver_extension_is_keyed        # FAIL: key is "String?"
cargo test --bin kmp-lsp jar_nested_type_receiver_extension_is_keyed     # FAIL: key is "Outer.Inner"
cargo test --bin kmp-lsp jar_manifest_tier1_receiver_key_matches_tier2   # FAIL
cargo test --bin kmp-lsp jar_manifest_cache_ignores_a_stale_v3_version   # FAIL until the bump
```

- [ ] **Step 3: Implement**

Add `extension_receiver_key` to `src/indexer/jar.rs` exactly as specified in the design (reusing
`StrExt::strip_nullable` and the dotted-leaf helper, with the `use crate::str_ext::StrExt;` import at
both call sites — the Tier-1 site is in a different module scope). Call it at `:932-937` and
`:1499-1506`. Bump `JAR_MANIFEST_CACHE_VERSION` to `4`.

Do **not** touch `src/parser.rs` — the source side already normalizes correctly, and duplicating the
helper there would create the drift this task exists to prevent.

- [ ] **Step 4: Verify**

```
cargo test --bin kmp-lsp
cargo clippy -- -D warnings
cargo fmt --check
```

- [ ] **Step 5: Measure on the real corpus**

Rebuild release and re-run `resolution-accuracy`.

**Verify the cache actually rolled over by file presence, not by a log line (critique M8).**
`cache_path()` (`src/indexer/jar_manifest_cache.rs:83`) embeds the version in the *filename*
(`jar-manifest-v{VERSION}.bin`), so after the bump the loader simply does not find
`jar-manifest-v4.bin` and takes the silent early-return arm at `:126-128`. The
`"jar_manifest_cache: version mismatch or corrupt"` log at `:139` can **never** fire for a version
bump. Check:

```
ls ~/.cache/kmp-lsp/jar-manifest-v*.bin     # before: only v3;  after the run: v4 exists
```

Success criterion: **`orEmpty` leaves the member-ref Gap top-20.** As a cheap independent check,
re-probe the count of nullable-suffixed `extension_by_receiver` keys — **54** before the fix, **0**
after.

- [ ] **Step 6: Commit and open a PR**

```bash
git commit -m "fix(jar): normalize JAR extension-receiver keys like the parser does

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01L7ZonwYUh94VsuQphiHykF"
```

---

### Task 3: Diagnose (do not fix) the `String` → `CharSequence` supertype-extension dead-end

**This task ships no production code.** The failure is verified real but its cause is not isolated,
and this project's own process lesson is that implementing against an un-isolated cause is how the
last two reverts happened. The critique independently agreed with deferring it — and noted it is
likely the **largest** of the three tasks, since it plausibly subsumes several Gap names this plan
excludes.

**Evidence to start from (all reproduced on the Moneta corpus, benchmark index setup):**

```
find_definition_qualified_index_only("isNotEmpty", Some("CharSequence")) -> resolves
find_definition_qualified_index_only("isNotEmpty", Some("String"))       -> []
find_definition_qualified_index_only("forEach",    Some("Iterable"))     -> resolves
find_definition_qualified_index_only("forEach",    Some("List"))         -> []
resolve_symbol_index_only("String") -> …/android-36.1/java/lang/String.java:142
supers(java/lang/String.java) = [(142,"java.io.Serializable"),(142,"Comparable"),
                                 (142,"CharSequence"),(142,"Constable"),(142,"ConstantDesc"), …]
ext["CharSequence"]["isNotEmpty"] = ["fun CharSequence.isNotEmpty(): Boolean"]
ext["CharSequence"]["contains"]   = 3 overloads   (relevant to the CardExtensions `contains` site)
```

Every input the walk needs is present and correct, and the same walk succeeds in an isolated fixture
(Q2). The dead-end is corpus-specific.

**Also in this task's evidence list, moved here in draft 2:** `CardExtensions.kt:8`'s `contains` —
receiver inferred as `CardRight` where `Card.rights` is really a Java `List<ContextRight>` field,
because a *sibling* function's `vararg rights: CardRight` parameter shadowed it. Whether that is a
scope bug or a `List`-receiver walk bug is exactly the kind of thing this diagnosis should separate.

- [ ] **Step 1: Instrument one hop at a time**

Add temporary `eprintln!` tracing (deleted before the task ends) inside `supertype_targets`
(`src/resolver/hierarchy.rs:176`) reporting, per hop: `class_name`, `class_uri`, the
`super_names_for_class` output, and for each super name whether
`resolve_symbol_hierarchy_ambiguity_safe` returned a location or declined. Drive it through the CLI
against `java.lang.String` / `"isNotEmpty"`.

> The probe harness this plan's Part 0 used is the cheapest way to do this: a `KMP_PROBE_LIST`
> env-var-driven block at the head of `hover_at` (`src/cli/hover.rs`) listing `path|line|col` targets,
> with `run_hover` temporarily switched to the benchmark's index setup (`build_index(root, true)` plus
> `index_jars` + `JarPhase::Ready`). Re-create it, then delete it.

- [ ] **Step 2: Confirm or refute the leading hypothesis**

Leading hypothesis: `resolve_symbol_hierarchy_ambiguity_safe` **declines** `"CharSequence"` at hop 1
because the corpus has both `java/lang/CharSequence.java` and kotlin-stdlib's own `CharSequence`, and
the ambiguity-safe tie-break cannot separate them from a `java/lang/String.java` origin. If confirmed,
the fix is a tie-break question in `ambiguity_safe_tail_with_denylist` — which is the shared tie-break
for **three** IO arms (`src/resolver/resolve.rs:411`, `:471`, `:483`) and moves far more than this
cluster. It belongs in its own plan with its own measurement, **not** here.

- [ ] **Step 3: Check the sidecar budget as the alternative cause**

`MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` is spent by `ensure_jar_definitions_for` once per super
name; `java.lang.String` has **five** direct supers and `CharSequence` is third. Confirm the budget is
not exhausted before the `CharSequence` hop is attempted. This is a known-shape bug already ticketed
as "Task 4: Close the `hierarchy.rs` budget leak" in
`docs/superpowers/plans/2026-09-09-jar-promotion-latency-budget-plan.md`.

- [ ] **Step 4: Write up, delete the tracing, stop**

Produce a short findings note naming the exact dead-end line and which hypothesis held. **Do not
implement a fix in this task.** Confirm `git status` is clean before finishing.

---

### Task 4 (conditional, opened only if Task 1 Step 7 says so): restore hover for same-arity candidate sets

Loosen `pick_unambiguous_location` (`src/features/hover.rs:134-145`) to accept a candidate set whose
members all share the same name **and** the same arity, returning the first. ~3 lines, confined to
`src/features/hover.rs`, restores hover for the `Modifier.weight` collision shape **without**
re-collapsing resolution.

Deliberately not bundled into Task 1: it is an unmeasured hover-behaviour change, and mixing it into a
resolver PR would make Task 1's measurement ambiguous. Needs its own test
(`hover_renders_for_a_same_arity_extension_collision`) and its own one-line PR.

---

### Task 5 (named, deferred): `implicit_receiver_extension_match` has the same first-match bug

`src/resolver/extension.rs:144-168` picks the **first** symbol matching
`extension_declaration_matches` (`:151-158`) and then shape-checks *only that one* (`:163`). An
implicit-receiver 1-arg call whose declaring file lists the 0-arg overload first is rejected outright,
even though the right overload is two lines below — the same bug as Task 1's, compounded with an arity
filter applied to a single pre-selected candidate.

Deferred, not dismissed: it is a separate entry point (`resolve_implicit_receiver_callee`) with its
own callers, no currently-measured Gap name points at it, and folding it in would widen Task 1's blast
radius past what one corpus measurement can attribute. The fix shape is known — collect every matching
symbol, shape-filter the set, return the survivors — so this is a one-session follow-up once Task 1 has
measured.

---

## Appendix A — how Phase 1 was verified

Two throwaway instruments, both since deleted:

1. A `#[test]` appended to `src/resolver/tests.rs` building the `List`/`Collection`/`Iterable`
   platform-source chain with `write_fake_android_sdk_source`, registering an `"Iterable"`-keyed
   extension, and printing `classify_cursor` / `find_definition_qualified_index_only` /
   `resolve_identity_with_io`. This proved the machinery works in isolation (Q2).
2. A probe block at the head of `hover_at` (`src/cli/hover.rs`) driven by a `KMP_PROBE_LIST` env var
   listing `path|line|col` targets, with `run_hover` temporarily switched to the benchmark's own index
   setup. This produced every real-corpus number quoted above. `resolve_symbol`,
   `resolve_symbol_index_only` and `resolve_kotlin_builtin_type_platform_equivalent` were temporarily
   un-`#[cfg(test)]`-gated in `src/resolver/mod.rs` to reach them from the CLI.

Both were reverted; `cargo build` and `cargo test --bin kmp-lsp` (1946 passed, 3 ignored) are green at
the state this document was written.

## Appendix B — what changed from draft 1

| Item | Change |
|---|---|
| **B1** (blocking) | Draft 1 promised the suite stays green. It does not: `ambiguous_member_extension_name_collision_first_match_wins` (`tests.rs:7282`) pins the bug and goes red. **Decided explicitly**: rewrite it to assert the overload set in declaration order, replace its doc comment, and say so in the PR. The B1 × S4.1 interaction was traced through `resolve_qualified` and shown not to open a second conflict. |
| **B2** (blocking) | Draft 1's Task 2 test #2 (`"List<String>?"`) is green before the fix; its rationale was false. **Deleted and replaced** with `"Outer.Inner"` → `"Inner"`, a genuinely red shape exposing the second missing normalization. |
| **S3** | New **Decision S3** section + Task 1 Step 7 (explicit hover verification) + conditional **Task 4** as the pre-designed reversal. |
| **S4** | `jar_extension_for_type_root` (`qualified.rs:505-542`) **folded into Task 1** with its own test; `implicit_receiver_extension_match` **named and deferred as Task 5** instead of going unmentioned. |
| **S5** | `filter` and `toInt` re-located across all same-named files and **re-probed**: `filter` → receiver `T` (generic-substitution gap), `toInt` → receiver `Int` (primitive synthetic member). Both still excluded, now on evidence. The "exactly 2 in fix scope" headline survives re-enumeration. The critique's suggested `StringExtensions.kt:89` Bug-2 corroboration was probed and **does not hold** (`receiver_type: None`) — recorded as a negative result. |
| **S6** | `contains` reclassified with real evidence (`Card.java:18` is `List<ContextRight>`; the CST returned the sibling function's `vararg` element type) and moved to Task 3's evidence list. |
| **M7** | Helper body specified as one line over the existing `StrExt::strip_nullable`; the import both sites need is called out; the false "mirrors the parser" doc claim replaced by the true three-normalization statement (and the dotted-leaf strip actually added, which is what makes it true). |
| **M8** | Task 2 Step 5 verifies the cache rollover by file presence, not by a log line that can never fire. |
| **M9** | Task 2 test #4's near-duplication of `jar_manifest_cache_tests.rs:74` noted; kept as a constant-pin. |
| **M10** | Task 1's contingency re-scoped from "one-liner" to a real change (`walk_hierarchy_breadth_first` must carry `super_name` through), with a time budget. |
| **M11** | Task 1 tests 1–2 gained an explicit fixture-requirements block: the declaring file must be really indexed (the cited template's `file:///app/Extensions.kt` is not), and the `List` declaration requirement is stated. |
| **C2** | Line numbers corrected: struct `:320-349` (was `:337-365`), `.next()` calls `:402`/`:433` (was `:401`/`:434`), plus a "navigate by symbol, not line" note. |
| **C5** | The `detail`-equality discriminator is now claimed for **both** JAR and source paths (the critique verified source too), with the Java `"(...)"`-placeholder tie called out as the one inert exception. |
| **C6** | Added: the single derivation point in `jar.rs` is what keeps the registry key and `SymbolEntry.extension_receiver()` consistent — the reason Task 2 does not degrade ranges to `0:0`. |
