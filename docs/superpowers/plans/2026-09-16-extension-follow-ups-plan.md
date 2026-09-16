# Extension-registry follow-up cluster (post-PR #321) — verified diagnosis, design, and implementation plan

> **Draft 2, 2026-09-16.** Revised after an independent review that re-derived every claim from
> scratch and *ran* several of the plan's own tests. One blocker (Task 4's tests were false-green),
> nine smaller findings, all addressed — see **Appendix D** for the change log and **Appendix C** for
> the red-then-green evidence that now backs Task 4. Everything the review confirmed is retained
> unchanged.
>
> Written against `/home/ocel/Work/lsp/.worktrees/extension-follow-ups`
> (branch `fix/extension-registry-follow-ups`, tip `878cc4ea`, i.e. `main` after PR #321 plus two
> unrelated `chore:` commits). Measured against the real Moneta corpus at
> `/home/ocel/Work/Moneta/android`.
>
> Source: the controller's context brief `follow-ups-context.md`, items 1–5. **Every claim in that
> brief was re-checked against this worktree's actual source before being used here.** Four of its
> claims were wrong or incomplete; those corrections are Part 0, and they change the design of items
> 1, 2 and 4 materially — read Part 0 before reading the tasks.
>
> **Every claim marked VERIFIED was reproduced by running real code.** The instrument was a
> throwaway `throwaway_probe(&index, root)` block injected into `run_resolution_accuracy`
> (`src/cli/resolution_accuracy_poc.rs`), gated on a `KMP_PROBE` env var and placed *after* the
> benchmark's own index setup (`build_index(root, /* no_stdlib */ true)` + `scan_gradle_jars` +
> `detect_android_r_class_jars` + `index_jars`), so it observed the exact index state the
> `resolution-accuracy` benchmark observes. `mod platform_types;` was temporarily widened to
> `pub(crate) mod platform_types;` to reach the fallback from the CLI. Appendix A has the full probe.
>
> **Task 4's tests, and the Task 3/Task 4 production diffs they depend on, were additionally written
> into this worktree, compiled, and run** — real red, then real green — rather than only designed.
> See Appendix C. Every scratch edit has been reverted: `git status` shows only this plan file
> staged, and `cargo test --bin kmp-lsp` is back to the **1956 passed, 0 failed, 3 ignored**
> baseline.

---

## Part 0 — What the context brief got wrong

### 0.1 — Item 1 is **not** a "false-unique-win". It is `default_kotlin_import_tie_break` picking a decoy.

The brief states: *"`indexer.lookup_definitions("List")` returns exactly **one** candidate
corpus-wide — a decompiled/synthetic `List` symbol inside `kotlin-stdlib-2.4.10.jar`. Because
`lookup_definitions` returns exactly one entry, `ambiguity_safe_tail_with_denylist` treats it as an
unambiguous win."*

**VERIFIED FALSE.** On the real corpus `lookup_definitions("List")` returns **32** candidates,
including the correct `file:///…/android-36.1/java/util/List.java:138`. There is no unique-match
shortcut. What actually happens is one hop deeper inside
`ambiguity_safe_tail_with_denylist` (`src/resolver/tie_break.rs:75-107`): the 32 candidates survive
the denylist and module-scope stages, and then **`default_kotlin_import_tie_break`
(`src/resolver/tie_break.rs:135-156`) narrows them to the single `kotlin.collections`-packaged
candidate** — the body-less kotlin-stdlib decoy — because `kotlin.collections` is in
`KOTLIN_DEFAULT_IMPORT_PACKAGES`. That leaves exactly one location, the tail returns non-empty, and
`resolve_kotlin_builtin_type_platform_equivalent` is never reached.

The irony is on the record: that tie-break's own doc comment (`tie_break.rs:129-134`) names
*"an ambiguous `java.util.List` vs. `kotlin.collections.List`"* as the shape it must not get wrong
from a **Java** origin — and from a **Kotlin** origin it gets exactly that pair wrong, in the
opposite direction, for the one name family where the Kotlin candidate is a decoy with no `.class`
body.

This matters for the design: a fix aimed at "unique-match shortcuts" would have missed `List`
entirely.

### 0.2 — Only **7** of the 22 table names actually resolve wrong, and `String` is not among them.

The brief worries the reorder *"changes resolution for every name in that table (10+ names)"* and
asks whether `String` still resolves correctly. **VERIFIED, all 22 names, one probe run.** Comparing
`resolve_symbol_index_only(name, None, <a real Kotlin corpus file>)` against
`resolve_kotlin_builtin_type_platform_equivalent(name)`:

| name | `lookup_definitions` count | `resolve_symbol_index_only` today | platform equivalent | agree? |
|---|---|---|---|---|
| `String` | 38 | `…/java/lang/String.java:143` | `…/java/lang/String.java:143` | ✅ |
| `CharSequence` | 3 | `…/java/lang/CharSequence.java:59` | same | ✅ |
| **`List`** | **32** | **`jar:…/kotlin-stdlib-2.4.10.jar:2788`** | `…/java/util/List.java:138` | ❌ |
| **`MutableList`** | **2** | **`jar:…/kotlin-stdlib-2.4.10.jar:2789`** | `…/java/util/List.java:138` | ❌ |
| `Set` | 4 | `…/java/util/Set.java:115` | same | ✅ |
| **`MutableSet`** | **1** | **`jar:…/kotlin-gradle-plugin-2.4.10-gradle813.jar:27426`** | `…/java/util/Set.java:115` | ❌ |
| `Map` | 6 | `…/java/util/Map.java:163` | same | ✅ |
| **`MutableMap`** | **1** | **`jar:…/kotlin-gradle-plugin…:27427`** | `…/java/util/Map.java:163` | ❌ |
| `Collection` | 14 | `…/java/util/Collection.java:258` | same | ✅ |
| **`MutableCollection`** | **1** | **`jar:…/kotlin-gradle-plugin…:27423`** | `…/java/util/Collection.java:258` | ❌ |
| `Iterable` | 8 | `…/java/lang/Iterable.java:42` | same | ✅ |
| **`MutableIterable`** | **1** | **`jar:…/kotlin-gradle-plugin…:27422`** | `…/java/lang/Iterable.java:42` | ❌ |
| `Iterator` | 19 | `…/java/util/Iterator.java:58` | same | ✅ |
| **`MutableIterator`** | **1** | **`jar:…/kotlin-gradle-plugin…:27420`** | `…/java/util/Iterator.java:58` | ❌ |
| `Int` | 6 | `…/java/lang/Integer.java:1244` | same | ✅ |
| `Long` | 10 | `…/java/lang/Long.java:1375` | same | ✅ |
| `Double` | 7 | `…/java/lang/Double.java:745` | same | ✅ |
| `Float` | 8 | `…/java/lang/Float.java:673` | same | ✅ |
| `Boolean` | 10 | `…/java/lang/Boolean.java:133` | same | ✅ |
| `Byte` | 5 | `…/java/lang/Byte.java:380` | same | ✅ |
| `Short` | 8 | `…/java/lang/Short.java:383` | same | ✅ |
| `Char` | 4 | `…/java/lang/Character.java:9014` | same | ✅ |

**15 of 22 already agree with the platform equivalent, so reordering is provably a no-op for them on
this corpus. 7 change, and each change is a decoy → real-declaration improvement.** `String` is safe,
confirmed on current `main`, not assumed. Note also that the two failure shapes are *different*:
`List`/`MutableList` lose the tie-break to a `kotlin-stdlib` decoy, while the other five
`Mutable*` names genuinely are unique-match wins over a `kotlin-gradle-plugin` decoy — so both
halves of 0.1 matter and a single reorder covers both.

### 0.3 — `extension_is_in_scope` has **six** callers, not two, and most are not extension-specific.

The brief says *"its only two callers, `resolve_extension_in_scope` in `extension.rs` and
`implicit_receiver_extension_match`"*. **VERIFIED FALSE.** Current callers:

| # | Call site | Is it about extensions? |
|---|---|---|
| 1 | `src/resolver/extension.rs:91` — `resolve_extension_in_scope` | yes |
| 2 | `src/resolver/extension.rs:178` — `implicit_receiver_extension_match` | yes |
| 3 | `src/resolver/infer.rs:1936` — `find_extension_fn_return_type_scoped` | yes |
| 4 | `src/features/nullable_call_diagnostics.rs:275` — `extension_in_scope_here` | yes, but with its own stricter rule |
| 5 | `src/resolver/infer.rs:1626` — `candidate_declaration_is_reachable` | **no** — a receiver-less by-name reachability check over *all* workspace definitions |
| 6 | `src/indexer.rs:1099` — `Indexer::jar_candidate_is_reachable` | **no** — the same, over *all* JAR definitions |

The function's own doc comment at `infer.rs:1604-1607` admits it: *"its body is not actually
extension-specific"*. **Consequence: the brief's "add a default-import-package check to
`extension_is_in_scope`" would silently change bare-name candidate preference for every JAR symbol in
`kotlin.*`/`java.lang` corpus-wide** (call site 6 gates which JAR candidates are "reachable" for
*any* name). That is a much larger, differently-shaped blast radius than the brief credits, and it is
not what item 2 is trying to fix. The design below therefore adds a *new* entry-shaped wrapper used
by the three extension-registry call sites only, and leaves the shared predicate alone.

### 0.4 — `find_extension_fn_return_type_scoped` and `_global` do **not** have the same bug shape.

The brief asks to *"verify whether the same bug exists in both"*. Read in full
(`src/resolver/infer.rs:1894-1977` and `:1979-2009`):

- **`_scoped`** iterates `ExtensionEntry`s from `extension_by_receiver`, so it *does* have the
  PR #321 family bug — but only on its truncated-`detail` fallback path (`:1959-1970`), where a
  single `.find(|s| extension_declaration_matches(…))` picks the first declaration in the file
  regardless of which `entry` the loop is on. On the primary path (`:1947-1949`) it reads
  `entry.detail` directly, which *is* per-overload correct; what it lacks there is any way to choose
  *between* overloads.
- **`_global`** has **no `ExtensionEntry` at all**. It walks `find_in_workspace_defs(method_name, …)`
  and scans each file's own `symbols` by `(name, kind, extension_receiver)`. There is no `detail` to
  match against, no registry entry, and no scope check. **`select_extension_symbol_range`'s two-step
  selection cannot be applied to it** — there is no second step to apply. It is also effectively
  dead in production: its only production caller chain
  (`Resolver::method_return_type` → `find_method_return_type` → `find_extension_fn_return_type`) is
  reached from `Indexer::find_method_return_type_for_type` (`src/indexer.rs:507`), which always
  passes `Some(uri)`.

Two further findings the brief did not have, both from reading the full body:

- **Real early-abort bug, not just a first-match bug.** In `_scoped`'s truncated-detail fallback,
  both `indexer.files.get(&entry.file_uri)…?` (`:1955-1958`) and `.find(…)?` (`:1962-1969`) return
  `None` from the **whole function**, not from the current loop iteration. One entry whose declaring
  file is not loaded, or whose declaration cannot be located, aborts the search over every remaining
  entry. This is a `continue`-vs-`?` bug and it is cheap to fix.
- **Neither variant has a `CallShape`, and neither does any caller up the chain.** `find_method_return_type`,
  `Resolver::method_return_type` (`src/resolver/api.rs:162-167`) and
  `Indexer::find_method_return_type_for_type` are all arity-blind. "Add arity awareness" is therefore
  not a local fix — it is an API change through the `Resolver` trait. Decision D4 below scopes that
  out with evidence.

### 0.5 — Two corrections that strengthen, rather than contradict, the brief

- The brief calls `jar_extension_for_type_root` a "compounding factor" that makes the
  `CharSequence` case "work" by coincidence. **VERIFIED and sharper than stated:** that scope-blind
  fallback is reachable **only** for a `QualifierRoot::TypePath` root
  (`src/resolver/qualified.rs:126-129` — `if let QualifierRoot::TypePath { root, nested } = parsed`).
  A real call site like `text.isNotEmpty()` has a **value** root and never reaches it. So the scope
  rejection in item 2 is *fatal* for exactly the shape users write, and only invisible when a test or
  probe spells the type name out. This is why item 2 is worth doing and why a naive end-to-end test
  of it will be green before the fix (see Task 2 Step 1's trap note).
- Every other line number and code excerpt in the brief re-verified as correct:
  `resolve.rs:411/421/459/462-470/473-481`, `platform_types.rs:50-86/88-93/110`,
  `infer.rs:1791-1850/1857-1866`, `extension.rs:28-54/162-219`, `qualified.rs:513`,
  `rename.rs:100-110`.

---

## Part 1 — Design

### Global constraints

- **Repo house rules (`AGENTS.md`):** never commit to `main`; `cargo test` and
  `cargo clippy --all-targets -- -D warnings` clean after every change; `cargo fmt` before committing;
  **no abbreviated names** (`symbol`, `location`, `indexer`, `package` — never `sym`, `loc`, `idx`,
  `pkg`) in any new code, tests included; **no `and` in a function name**; no `unwrap()`/`expect()` in
  production code; no hardcoded tree-sitter node-kind strings. `.git/hooks/pre-push` gates five
  checks (those four plus "no direct `index_workspace_full`/`index_workspace_prioritized` calls
  outside the actor"). **Do not treat the hook as the definition of the rules:** its abbreviation
  check is a fixed token list (`\b([sc])\b` plus a specific suffix list) and misses `fd`, `pkg` and
  friends entirely, so AGENTS.md is the authority and the hook is a partial backstop. Every new name
  in this plan (`extension_entry_is_in_scope`, `select_extension_symbol`,
  `android_sdk_source_roots`) was checked against all five rules by hand.
- **Serena:** call `mcp__serena__activate_project` with **this worktree's** path
  (`/home/ocel/Work/lsp/.worktrees/extension-follow-ups`) before any symbolic edit. One active project
  at a time — parallel tasks in this worktree must not both drive Serena.
- **Attribution:** every commit ends with
  `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` and **nothing else** — no
  `Claude-Session:` line, in any commit or PR description.
- **No interactive rebase.** `git rebase -i` is unavailable in this session's tooling. Use
  `git cherry-pick` / `git commit-tree` plumbing for any history edit.
- **Baseline:** `cargo test --bin kmp-lsp` → **1956 passed, 0 failed, 3 ignored** at `878cc4ea`.
  Each task states its own expected post-count.
- **No cache-version bump is needed by any task here.** Nothing in this cluster changes a persisted
  key or field — `JAR_MANIFEST_CACHE_VERSION` and `CACHE_VERSION` both stay put. (Stated explicitly
  because the PR #298–300 memo's stale-cache trap applies only to derivation changes, and this
  cluster has none.)
- **Measurement discipline:** `resolution-accuracy` recall is noisy at ±0.3–0.6pp on an unchanged
  binary. Judge a task by whether its named Gap entries move, not by the aggregate percentage.
- **Line numbers below are against `878cc4ea`.** Navigate by symbol name if they drift.

### Fix 1 — Try the Kotlin built-in platform equivalent *before* the global-definitions tail

`resolve_chain` (`src/resolver/resolve.rs:248-484`) has four IO-policy tails that all end
`<global definitions lookup>; if non-empty return it; else resolve_kotlin_builtin_type_platform_equivalent`.
Swap the two, in all four:

| policy | today | after |
|---|---|---|
| `Full` (step 5.4 / 5.5) | `:403-414` then `:421` | platform equivalent at `:415`, then the existing `:403-414` block |
| `NoRg` | `:450-458` then `:459` | platform equivalent first |
| `IndexOnly` | `:462-469` then `:470` | platform equivalent first |
| `HierarchyAmbiguitySafe` | `:473-480` then `:481` | platform equivalent first |
| `ScopedOnly` | no tail at all | **unchanged** — "no tail" is its documented contract |

This sits **below** steps 1–4.5 (local declaration, local line scan, explicit imports, Swift fast
path, same package, star imports, class hierarchy), all of which still win first. A workspace file
that really declares its own `class Double` is still found by its own callers through steps 1/3 —
verified present on the corpus (`…/InitUseCase.kt:73` declares a `Double`) and unaffected, because
that path never reaches the tail for an in-package caller. (The corpus does contain a workspace
declaration literally named `Double` — `…/InitUseCase.kt:73` — though it is a `data class Double`
**nested** inside `sealed class IntRates`, not a top-level declaration, so it is weaker corroboration
than the first draft of this plan claimed. The guard test below uses its own top-level fixture and
does not depend on this citation.)

`resolve_kotlin_builtin_type_platform_equivalent`'s doc comment (`platform_types.rs:88-93`) states a
founding assumption that Part 0.1 disproves — *"every normal resolution step already failed by the
time any tail fallback calls this … so this can only ever turn an existing decline into a correct
resolve"*. It must be rewritten to state the real rule: **for the 22 compiler-intrinsic mapped types
in `KOTLIN_BUILTIN_TYPE_PLATFORM_EQUIVALENTS`, the real platform declaration outranks any same-named
index/JAR candidate, because those names have no compiled `.class` anywhere and every same-named
candidate is therefore a decoy.** Also update the test-module banner at
`src/resolver/tests.rs:9272-9280`, which repeats the "last-resort" framing.

#### Decision D1 — memoize the SDK-source-root probe in the same task, keyed on the root path

*(Revised after review. The first draft deferred this behind a wall-clock measurement; that was
over-built for the problem and left a log-spam regression shipping in the meantime. Adopting the
reviewer's simpler design.)*

`resolve_kotlin_builtin_type_platform_equivalent` calls
`crate::workspace_json::detect_android_sdk_source_paths(&workspace_root)`
(`platform_types.rs:132`), which is **not memoized** — every call does a `local.properties` read (or
env lookup), an `is_dir`, and a `read_dir` of `sdk/sources/` (`src/workspace_json.rs:666-686` →
`resolve_android_sdk_root`/`sdk_dir_from_local_properties`, `:849-869`). Today that only runs when
the whole chain already declined, which for `String`/`Int` never happens. After the reorder it runs
on every tail resolution of a built-in name — the hottest name class in any Kotlin corpus, on the
hover/inlay path. This project has been burned by exactly this class of regression before (the
hover/inlay multi-second stall from bare-name global scans).

**And the cost is not only wall clock.** `detect_android_sdk_source_paths` does
`log::info!("android-sdk: auto-detected sources at …")` on **every successful detection**
(`workspace_json.rs:681`). Unmemoized after the reorder, that line fires once per built-in-name tail
resolution — it was observable as dozens of repeated lines per second in this plan's own probe run.
That is a user-visible log regression regardless of how the timing comes out, so "ship it and see"
is not available as an option here.

**Decision: memoize in Task 1, keyed on the workspace root path itself.** Path-keyed rather than
`OnceLock`, because `WorkspaceRoot::set` (`src/indexer/workspace_root.rs:35-42`) can change the root
mid-session — it bumps a generation counter for exactly that reason. Comparing the key is simpler
than threading that generation through and is self-invalidating:

```rust
use std::sync::Mutex;

/// Memoized [`crate::workspace_json::detect_android_sdk_source_paths`], keyed
/// on the workspace root it was computed for.
///
/// That function does real filesystem work on every call — a
/// `local.properties` read, an `is_dir`, and a `read_dir` of `sdk/sources/` —
/// plus a `log::info!` on every success. Harmless while the built-in-type
/// fallback only ran after the whole resolution chain had already declined;
/// not harmless now that it runs ahead of the tail for every name in
/// [`KOTLIN_BUILTIN_TYPE_PLATFORM_EQUIVALENTS`], which is the hottest name
/// class in a Kotlin corpus and sits on the hover/inlay path.
///
/// Keyed on the root path rather than cached once, because
/// `WorkspaceRoot::set` can change the root mid-session. An SDK installed
/// *after* the first probe is not picked up until the root changes — the same
/// staleness the workspace scan already has, since it reads these paths once
/// at startup (`src/workspace/mod.rs:133`, `src/cli/run.rs:284`).
fn android_sdk_source_roots(workspace_root: &Path) -> Vec<PathBuf> {
    static DETECTED_ROOTS: Mutex<Option<(PathBuf, Vec<PathBuf>)>> = Mutex::new(None);
    let Ok(mut detected) = DETECTED_ROOTS.lock() else {
        return crate::workspace_json::detect_android_sdk_source_paths(workspace_root);
    };
    if let Some((cached_root, cached_paths)) = detected.as_ref() {
        if cached_root == workspace_root {
            return cached_paths.clone();
        }
    }
    let paths = crate::workspace_json::detect_android_sdk_source_paths(workspace_root);
    *detected = Some((workspace_root.to_path_buf(), paths.clone()));
    paths
}
```

Note the poisoned-lock arm falls through to the uncached call rather than `unwrap()`ing — the
pre-push hook forbids `unwrap`/`expect` in production code, and "slower but correct" is the right
behaviour for a cache anyway.

**Placement: `src/resolver/platform_types.rs`, not `workspace_json.rs`.** The two other callers of
`detect_android_sdk_source_paths` run once each at scan time; memoizing inside the shared function
would change their behaviour for no benefit. Only the hot caller needs it.

**Why this needs no test of its own:** the suite already contains eight tests that call
`resolve_kotlin_builtin_type_platform_equivalent` against *different* `tempfile::tempdir()` roots
(`src/resolver/tests.rs:9303`, `:9337`, `:9359`, `:9384`, `:9427`, `:9490`, `:9518`, `:10168`,
`:10190`). A memo that ignored its key would serve one test's SDK layout to another and they would go
red. That is real coverage of the only thing that can go wrong here; adding a ninth test that asserts
the cache-key comparison directly would be testing the implementation, not the behaviour.

Wall clock is still recorded in Task 1 Step 5 — not as a gate, just so the effect of the memo is on
the record.

#### Expected effect of Fix 1, as a falsifiable prediction

- `List`, `MutableList`, `MutableSet`, `MutableMap`, `MutableCollection`, `MutableIterable`,
  `MutableIterator` stop anchoring on a decoy and start anchoring on a real `java.util.*`/`java.lang.*`
  declaration **with a walkable supertype chain**.
- The other 15 table names are unchanged — measured, not hoped.
- Downstream: `type_path_anchors` (`src/resolver/qualified.rs`) now anchors `List` on
  `java/util/List.java`, so the supertype walk `List → Collection → Iterable` becomes reachable for
  the first time on this corpus.
- **`firstOrNull` will probably NOT leave the Gap top-20 on Fix 1 alone.** VERIFIED why: even with a
  correct `List` anchor, the `Iterable`-keyed `firstOrNull` overloads are rejected by
  `extension_is_in_scope` (probe: `Iterable.firstOrNull` → `in_scope=false`, twice). Fix 1 and Fix 2
  are **jointly** necessary for the trailing-lambda `firstOrNull` shape. Fix 2 alone should recover
  the 0-arg `List`-keyed overload. Do not read "`firstOrNull` still present" after Task 1 as Task 1
  failing.

### Fix 2 — Kotlin's default-import packages count as in-scope for an extension entry

**Do not touch `extension_is_in_scope`** (Part 0.3). Add one entry-shaped wrapper next to it in
`src/resolver/infer.rs`:

```rust
/// Whether extension registry `entry` is callable from the file at `from_uri`.
///
/// [`extension_is_in_scope`]'s package/import rules, plus the one rule they
/// cannot express: a Kotlin file implicitly imports every name declared
/// directly in Kotlin's own default-import packages (see
/// [`crate::resolver::imports::KOTLIN_DEFAULT_IMPORT_PACKAGES`]), so
/// `kotlin.text`'s `isNotEmpty` or `kotlin.collections`'s `firstOrNull` is in
/// scope at every call site with no `import` line anywhere — and no real file
/// ever writes one.
///
/// Gated on the CALLING file's language, for the same reason
/// [`crate::resolver::tie_break`]'s `default_kotlin_import_tie_break` gates
/// its own use of the same set: Kotlin's default imports are a fact about
/// Kotlin source files, and `resolve_qualified` runs over indexed `.java` and
/// `.swift` files too.
///
/// Measured on the Moneta corpus: the registry holds 24897 entries across 2360
/// receiver buckets; 3641 of those entries live in a default-import package,
/// 3638 of them top-level, and every one was rejected here for every caller.
///
/// Deliberately NOT folded into [`extension_is_in_scope`] itself: four of that
/// function's six callers are not about extensions at all
/// (`candidate_declaration_is_reachable`, `Indexer::jar_candidate_is_reachable`
/// and `nullable_call_diagnostics`' stricter own rule), and widening the shared
/// predicate would change bare-name JAR candidate preference corpus-wide.
pub(crate) fn extension_entry_is_in_scope(
    entry: &crate::types::ExtensionEntry,
    from_uri: &Url,
    caller_file_data: Option<&FileData>,
) -> bool {
    if extension_is_in_scope(
        entry.package.as_ref(),
        &entry.name,
        entry.container.as_ref(),
        entry.visibility,
        entry.file_uri == from_uri.as_str(),
        caller_file_data,
    ) {
        return true;
    }
    let caller_is_kotlin =
        crate::Language::from_path(from_uri.as_str()) == crate::Language::Kotlin;
    let entry_is_default_imported = entry
        .package
        .as_ref()
        .is_some_and(|package| crate::resolver::imports::is_default_import_package(package));
    caller_is_kotlin && entry_is_default_imported
}
```

Reuse, not reinvention: `is_default_import_package` already exists at `src/resolver/imports.rs:46-48`
and needs only to widen from private `fn` to `pub(super) fn` (it is in a `mod imports;` private to
`resolver`, so `pub(super)` makes it visible to `resolver::infer` exactly as
`KOTLIN_DEFAULT_IMPORT_PACKAGES` already is to `resolver::tie_break`).

Then replace the three extension-registry call sites with it. Each shrinks from a seven-line call to
one line:

| site | today | after |
|---|---|---|
| `src/resolver/extension.rs:91-98` (`resolve_extension_in_scope`) | 8-line `extension_is_in_scope(…)` | `extension_entry_is_in_scope(entry, from_uri, caller_file_data_ref)` |
| `src/resolver/extension.rs:178-185` (`implicit_receiver_extension_match`) | same | same |
| `src/resolver/infer.rs:1936-1945` (`find_extension_fn_return_type_scoped`) | same | same |

**Explicitly not changed:** `src/features/nullable_call_diagnostics.rs:275`. That call site
deliberately passes `None`/`Public`/`false` and requires *stronger* evidence than ordinary scope
(its own doc comment at `:252-272` explains why a member extension needs the declaring file itself).
Widening it would relax a diagnostic's suppression rule with no measurement behind it; out of scope
here, named so it is not an oversight.

#### Decision D2 — the language gate is required, not speculative

The brief left this open. **VERIFIED: non-Kotlin origins really do reach these paths.**
`resolve_extension_in_scope` is called from `candidates_on` (`qualified.rs:404`) and
`resolve_extension_via_supertype_hierarchy` (`qualified.rs:740`), both under `resolve_qualified`,
which is language-agnostic; `resolve_implicit_receiver_callee` is called from
`src/indexer/lookup.rs:126` and `src/features/references.rs:228`, likewise. And the
`resolution-accuracy` benchmark itself scans **`.kt` and `.java`** workspace files
(`src/cli/resolution_accuracy_poc.rs:99`), driving `classify_cursor` + `resolve_identity` over both.
So a Java-origin call site can reach the extension registry today, and marking every `kotlin.*` entry
in-scope for it would be wrong in precisely the way `default_kotlin_import_tie_break`'s Copilot
finding described. Gate on Kotlin, and pin it with a test.

#### Expected effect of Fix 2

Registry-wide, measured: 2360 receiver buckets / 24897 entries, of which **3641 are in a
default-import package (3638 top-level)** and are rejected today for every caller. Spot-checked
individually, all `in_scope=false` today:

```
CharSequence.isNotEmpty : pkg=kotlin.text        fun CharSequence.isNotEmpty(): Boolean
Iterable.firstOrNull    : pkg=kotlin.collections fun <T> Iterable<T>.firstOrNull(): T?
Iterable.firstOrNull    : pkg=kotlin.collections fun <T> Iterable<T>.firstOrNull(predicate: (T) -> Boolean): T?
Iterable.forEach        : pkg=kotlin.collections fun <T> Iterable<T>.forEach(action: (T) -> Unit)
List.firstOrNull        : pkg=kotlin.collections fun <T> List<T>.firstOrNull(): T?
```

Prediction: `isNotEmpty`, `forEach` and the 0-arg `firstOrNull` shape become resolvable through the
**value-root** path (`text.isNotEmpty()`), which — per Part 0.5 — has no scope-blind fallback to hide
behind today. This is the largest single change in the cluster and the one most likely to move the
aggregate number. It is also the one most likely to raise `FilteredCandidate`, since it hands the
shape filter more candidates; watch that counter.

### Fix 3 — `implicit_receiver_extension_match` selects the symbol for *its own* entry

`src/resolver/extension.rs:162-219`. Verified exactly as the brief describes: for each `entry`, the
inner `fd.symbols.iter().find(|s| extension_declaration_matches(s, name, receiver_base, entry.container))`
(`:197-207`) matches on `(name, extension_receiver, container)` only — identical across every overload
— so every iteration of the loop picks the **same** first-declared overload, shape-checks that one at
`:210-216`, and a differently-shaped call is rejected on every iteration. The corpus makes this
concrete: **2589 of 4211 multi-entry `(receiver, name)` registry groups have two or more entries
declared in the same file**, e.g.

```
HttpClientCall.receive : ["suspend fun <T> HttpClientCall.receive(): T",
                          "suspend fun HttpClientCall.receive(info: TypeInfo): Any"]
Writer.write           : ["fun Writer.write(document: Document, prettyPrint: Boolean = ...): Writer",
                          "fun Writer.write(element: Element, prettyPrint: Boolean = ...): Writer"]
```

**Fix: extract PR #321's proven two-step selection into a symbol-returning sibling**, as the brief's
option (a). `select_extension_symbol_range` (`extension.rs:28-54`) already does filter-by-
`extension_declaration_matches` then prefer-exact-`detail`-match, with a first-match fallback; it just
throws the symbol away and keeps the `Range`. Split it:

```rust
/// The declaring symbol for one extension registry `entry`, inside its
/// declaring file's already-parsed symbol table.
///
/// A file may declare several overloads of the same extension — same name,
/// same receiver, same container — and `extension_declaration_matches` alone
/// cannot tell them apart, so prefer the declaration whose `detail` (full
/// signature text) is exactly this entry's own. `SymbolEntry::detail` and
/// `ExtensionEntry::detail` are the same string by construction on both
/// derivation paths (JAR: `src/indexer/jar.rs`; source: `src/indexer/apply.rs`).
/// When nothing matches exactly — a JAR-derived Java method can carry a
/// `"(...)"` placeholder detail, the PR #311 shape — fall back to the first
/// shape-matching declaration rather than returning nothing.
pub(super) fn select_extension_symbol<'file>(
    file_data: &'file FileData,
    name: &str,
    receiver_base: &str,
    container: Option<&String>,
    detail: &str,
) -> Option<&'file crate::types::SymbolEntry> {
    let declaring_symbols: Vec<&crate::types::SymbolEntry> = file_data
        .symbols
        .iter()
        .filter(|symbol| {
            crate::resolver::infer::extension_declaration_matches(
                symbol,
                name,
                receiver_base,
                container,
            )
        })
        .collect();
    let exact_signature_match = declaring_symbols
        .iter()
        .find(|symbol| symbol.detail == detail);
    exact_signature_match
        .or_else(|| declaring_symbols.first())
        .copied()
}

pub(super) fn select_extension_symbol_range(
    file_data: &FileData,
    name: &str,
    receiver_base: &str,
    container: Option<&String>,
    detail: &str,
) -> Range {
    select_extension_symbol(file_data, name, receiver_base, container, detail)
        .map(|symbol| symbol.selection_range)
        .unwrap_or_default()
}
```

`implicit_receiver_extension_match` then replaces its `.find(…).cloned()` with
`select_extension_symbol(&file_data, name, receiver_base, entry.container.as_ref(), &entry.detail).cloned()`
and keeps the rest of the loop (vararg exemption, `shape.accepts`) untouched. Its two existing
callers of `select_extension_symbol_range` (`extension.rs:110`, `qualified.rs:537`) are unaffected.

**Blast radius:** one entry point (`resolve_implicit_receiver_callee`) and its two callers
(`src/indexer/lookup.rs:126`, `src/features/references.rs:228`). No corpus-wide risk. No measurement
required beyond a spot-check.

### Fix 4 — `find_extension_fn_return_type_scoped`: per-entry selection, and stop aborting the loop

Two changes in `src/resolver/infer.rs:1955-1974`, both inside the truncated-`detail` fallback:

1. Replace `.find(|s| extension_declaration_matches(…))?` with
   `select_extension_symbol(file_data, method_name, receiver_base, entry.container.as_ref(), &entry.detail)`
   — the same helper Fix 3 extracts, reused rather than reimplemented.
2. Replace both `?` early-returns with `continue`, so one entry with an unloaded declaring file or an
   unlocatable declaration no longer aborts the search over every remaining entry (Part 0.4).

`find_extension_fn_return_type_global` is **left alone**, with a short comment saying why: no
`ExtensionEntry`, no `detail`, nothing for the two-step selection to select *between*, and no
production caller reaches it (`Indexer::find_method_return_type_for_type` always passes `Some(uri)`).

#### Decision D4 — arity-aware return-type inference is scoped OUT, with the evidence that justifies a future plan

The reviewer's original finding was *"returns the FIRST matching entry's return type with no arity
awareness at all"*. That is real, and the corpus shows it is not rare: of **4211** `(receiver, name)`
registry groups with two or more entries, **482 have entries with differing return types** —

```
Array.max      : Double? | Float? | T? | Double | Float | T          (6 overloads, 3 distinct returns)
Array.minBy    : T? | T
Array.sumOf    : BigDecimal | BigInteger | Double | Int | Long | UInt | ULong
Array.toMap    : Map<K, V> | M
Error.renderError : (no return) | String
```

**But there is no arity to select with, at any point in the chain.**
`find_extension_fn_return_type(indexer, receiver_base, method_name, from_uri)`,
`find_method_return_type(…)` (`infer.rs:1739`), `Resolver::method_return_type(…)`
(`api.rs:162-167`) and `Indexer::find_method_return_type_for_type(…)` (`indexer.rs:483`) are all
arity-blind. Making this right means threading a `CallShape` through the `Resolver` trait — which has
precedent (`find_method_params_text` already takes one, `src/indexer.rs:1109`) but is a trait-surface
change with its own callers, its own regression surface, and no way to attribute its effect inside
this cluster's measurements.

**Decision: fix the two mechanical defects above in Task 4, and open the arity work as its own plan,
citing the 482-group measurement as its motivation.** Do not attempt it here. Also note the honest
limit: because 482 is a count of *registry groups*, not of *call sites*, the real user-visible
impact is still unquantified — quantifying it (how many hover/inlay-hint sites actually land on a
differing-return overload set) is the first step of that future plan, not an assumption it may start
from.

### Decision D5 — item 5 (`rename.rs`): ship as-is, close the item, no code change

`src/features/rename.rs:100-110`, verified current:

```rust
let identity = resolve_identity(&symbol, indexer, uri);
let NavigationSource::CstResolved(definitions) = identity else {
    return Err(refusal("identity is ambiguous — could not resolve a single definition"));
};
if definitions.len() != 1 {
    return Err(refusal("identity is ambiguous — matches more than one definition"));
}
```

**Decision: (a) — already-correct behaviour, ship as-is, close the item. No UX change, no code, no
test.** Reasons, in order of weight:

1. The pre-#321 behaviour was *unsound*: a same-arity `Modifier.weight` collision resolved to one
   arbitrary candidate and `rename` silently rewrote that one declaration and its references — a
   destructive, irreversible edit to an arbitrarily-chosen one of two genuinely distinct symbols.
   Refusing is strictly safer, and the safety direction is the one that matters for a mutating
   operation.
2. It is not a new code path invented by #321. `rename_impl` already had "ambiguous → refuse" as its
   documented contract for every other ambiguity; #321 merely stopped hiding one case from it.
   Shipping it means the contract is now uniform, which is worth more than a special case.
3. The obvious "improvement" — naming both candidates in the message — is probably not free.
   **ASSUMPTION, not verified:** the refusal string is a `tower_lsp::jsonrpc::Error` message, and the
   reasoning here assumes LSP clients surface such an error as a plain toast with no file/line
   affordance, making a message that lists two URIs noise rather than navigation. That was *not*
   checked against a real client (neither VS Code nor nvim was exercised), and it is the load-bearing
   reason this decision picks (a) over (b). If someone does check and finds a client that renders the
   message navigably, reason 3 collapses and (b) becomes worth reconsidering — reasons 1, 2 and 4
   still stand on their own, so the decision would survive, but the cost/benefit would shift.
4. Nothing in the cluster's measurements touches rename, so a change here would ship unmeasured.

The one thing to do is **write the decision down where it will be found**: a `//` line comment
immediately above the `definitions.len() != 1` refusal — it sits inside `rename_impl`'s body, so it
cannot be a `///` doc comment — recording that a same-arity extension collision reaches it by design
after PR #321, and that a disambiguating UX needs a client-side picker. That is a comment-only edit;
fold it into Task 3's commit (the task that already touches the extension-selection code this refusal
is downstream of) rather than opening a task for it.

### Task ordering and file-collision map

| task | item | production files touched | test files |
|---|---|---|---|
| 1 | 1 | `src/resolver/resolve.rs`, `src/resolver/platform_types.rs` | `src/resolver/tests.rs` |
| 2 | 2 | `src/resolver/infer.rs`, `src/resolver/imports.rs`, `src/resolver/extension.rs` | `src/resolver/tests.rs` |
| 3 | 3 | `src/resolver/extension.rs`, `src/features/rename.rs` (one `//` line comment in a function body, no behaviour change) | `src/resolver/tests.rs` |
| 4 | 4 | `src/resolver/infer.rs` | `src/resolver/infer_tests.rs` |
| 5 | 5 | — (Decision D5, folded into task 3) | — |

- **Task 1 is fully independent** of 2/3/4 — no shared production file. It can run in parallel with
  any of them.
- **Task 2 must precede Tasks 3 and 4.** It touches `extension.rs` (which Task 3 rewrites) and
  `infer.rs` (which Task 4 rewrites), and Task 3 extracts the helper Task 4 consumes.
- **Tasks 3 and 4 are independent of each other** once Task 2 has landed — different files
  (`extension.rs` vs `infer.rs`) — **except** that Task 4 calls `select_extension_symbol`, which Task
  3 creates. So: **Task 3 before Task 4**, or Task 4 rebases onto Task 3.
- Recommended shape: **Task 1 ∥ (Task 2 → Task 3 → Task 4)**, four PRs. Three tasks append to
  `src/resolver/tests.rs` (Tasks 1–3), in different regions; Task 4 appends to
  `src/resolver/infer_tests.rs` instead and so collides with none of them. A pre-flight conflict scan
  before any parallel dispatch is still the executor's job.
- Corpus measurements are serial regardless (one release build, one long benchmark run at a time), so
  Tasks 1 and 2 must not have their measurement runs interleaved.

---

## Part 2 — Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: `superpowers:test-driven-development` for each task,
> `superpowers:subagent-driven-development` to run them. Each task is independently testable,
> independently revertable, and ships as its own PR. Steps use checkbox (`- [ ]`) syntax.
>
> Re-read the **Global constraints** block above before writing any code. In particular: full words
> for every identifier in new code including tests (`indexer`, not `idx`), and
> `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` with no `Claude-Session:` line.

---

### Task 1: Prefer the Kotlin built-in platform equivalent over a same-named decoy

**Why first:** independent of every other task, corpus-verified with an exact before/after
enumeration (Part 0.2), and its measurement is the one most at risk of being confounded by Task 2's.

**Files:**
- Modify: `src/resolver/resolve.rs` — four tails (`:403-421` Full, `:449-460` NoRg, `:461-471`
  IndexOnly, `:472-482` HierarchyAmbiguitySafe)
- Modify: `src/resolver/platform_types.rs` — doc comment on
  `resolve_kotlin_builtin_type_platform_equivalent` (`:88-109`); new private
  `android_sdk_source_roots` memo (Decision D1) called from `:132`
- Modify: `src/resolver/tests.rs` — 3 new tests; update the module banner at `:9272-9280`

**Interfaces:** no public signature changes. `resolve_kotlin_builtin_type_platform_equivalent` keeps
`(&Indexer, &str) -> Vec<Location>`; `android_sdk_source_roots` is private to `platform_types`.

- [ ] **Step 1: Write the failing tests**

Append to `src/resolver/tests.rs`, next to the existing platform-equivalent block (`:9272+`), reusing
its `write_fake_android_sdk_source` helper.

```rust
/// Part 0.1's real corpus mechanism, in miniature: bare `List` has MANY
/// same-named index candidates, `ambiguity_safe_tail_with_denylist` survives
/// the denylist and module-scope stages, and then
/// `default_kotlin_import_tie_break` narrows to the single
/// `kotlin.collections`-packaged one -- a kotlin-stdlib decoy with no
/// compiled body and no supertypes. The real `java.util.List` must win
/// instead, or `type_path_anchors` anchors every `List` receiver on a
/// declaration with no walkable supertype chain.
#[test]
fn a_builtin_type_name_prefers_the_platform_declaration_over_a_default_imported_decoy() {
    use crate::types::FileData;
    use std::sync::Arc;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_fake_android_sdk_source(
        root,
        "java/util/List.java",
        "package java.util;\npublic interface List<E> extends Collection<E> {\n}\n",
    );

    let indexer = Indexer::new();
    indexer.workspace_root.set(root.to_path_buf());

    let stdlib_decoy_uri = "jar:file:///kotlin-stdlib.jar!/kotlin/collections/List.class";
    let unrelated_decoy_uri = "jar:file:///unrelated.jar!/com/example/List.class";
    for decoy_uri in [stdlib_decoy_uri, unrelated_decoy_uri] {
        indexer
            .jar_definitions
            .entry("List".to_owned())
            .or_default()
            .push(tower_lsp::lsp_types::Location {
                uri: Url::parse(decoy_uri).unwrap(),
                range: Default::default(),
            });
    }
    indexer.jar_files.insert(
        stdlib_decoy_uri.to_owned(),
        Arc::new(FileData {
            package: Some("kotlin.collections".to_owned()),
            ..Default::default()
        }),
    );
    indexer.jar_files.insert(
        unrelated_decoy_uri.to_owned(),
        Arc::new(FileData {
            package: Some("com.example".to_owned()),
            ..Default::default()
        }),
    );

    let caller_source = "package app\nfun use(items: List<String>) { }\n";
    let caller_path = root.join("Caller.kt");
    std::fs::write(&caller_path, caller_source).unwrap();
    let caller_uri = Url::from_file_path(&caller_path).unwrap();
    indexer.index_content(&caller_uri, caller_source);

    let locations = resolve_symbol_index_only(&indexer, "List", None, &caller_uri);
    assert_eq!(
        locations.len(),
        1,
        "expected exactly the platform declaration, got {locations:?}"
    );
    assert!(
        locations[0].uri.path().ends_with("java/util/List.java"),
        "expected the real java.util.List, got {:?}",
        locations[0].uri
    );
}

/// The second, different failure shape from Part 0.2: `MutableSet` and its
/// four `Mutable*` siblings have exactly ONE same-named index candidate (a
/// kotlin-gradle-plugin decoy), so they never reach a tie-break at all --
/// the unique-match arm returns the decoy outright. The platform equivalent
/// must still win, and note the target's simple name differs from the
/// Kotlin one (`MutableSet` -> `java.util.Set`).
#[test]
fn a_mutable_builtin_type_name_prefers_the_platform_declaration_over_a_unique_decoy() {
    use crate::types::FileData;
    use std::sync::Arc;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_fake_android_sdk_source(
        root,
        "java/util/Set.java",
        "package java.util;\npublic interface Set<E> extends Collection<E> {\n}\n",
    );

    let indexer = Indexer::new();
    indexer.workspace_root.set(root.to_path_buf());

    let decoy_uri = "jar:file:///kotlin-gradle-plugin.jar!/kotlin/collections/MutableSet.class";
    indexer.jar_definitions.insert(
        "MutableSet".to_owned(),
        vec![tower_lsp::lsp_types::Location {
            uri: Url::parse(decoy_uri).unwrap(),
            range: Default::default(),
        }],
    );
    indexer.jar_files.insert(
        decoy_uri.to_owned(),
        Arc::new(FileData {
            package: Some("kotlin.collections".to_owned()),
            ..Default::default()
        }),
    );

    let caller_source = "package app\nfun use(items: MutableSet<String>) { }\n";
    let caller_path = root.join("Caller.kt");
    std::fs::write(&caller_path, caller_source).unwrap();
    let caller_uri = Url::from_file_path(&caller_path).unwrap();
    indexer.index_content(&caller_uri, caller_source);

    let locations = resolve_symbol_index_only(&indexer, "MutableSet", None, &caller_uri);
    assert_eq!(
        locations.len(),
        1,
        "expected exactly the platform declaration, got {locations:?}"
    );
    assert!(
        locations[0].uri.path().ends_with("java/util/Set.java"),
        "expected the real java.util.Set, got {:?}",
        locations[0].uri
    );
}

/// Decoy guard, expected GREEN both before and after: the reorder lives in
/// `resolve_chain`'s TAIL, below steps 1-4.5, so a workspace file that really
/// declares its own `Double` must still win for a caller in its own package.
/// (Not purely hypothetical: the Moneta corpus declares a `Double` at
/// feature/credit_card/clip/.../InitUseCase.kt:73, though as a NESTED data
/// class rather than a top-level one. This fixture uses a top-level
/// declaration, which is the shape the tail ordering actually has to respect.)
#[test]
fn a_workspace_declaration_still_outranks_the_builtin_platform_equivalent() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_fake_android_sdk_source(
        root,
        "java/lang/Double.java",
        "package java.lang;\npublic final class Double extends Number {\n}\n",
    );

    let indexer = Indexer::new();
    indexer.workspace_root.set(root.to_path_buf());

    let declaration_source = "package app\nclass Double(val value: String)\n";
    let declaration_path = root.join("Double.kt");
    std::fs::write(&declaration_path, declaration_source).unwrap();
    let declaration_uri = Url::from_file_path(&declaration_path).unwrap();
    indexer.index_content(&declaration_uri, declaration_source);

    let caller_source = "package app\nfun use(value: Double) { }\n";
    let caller_path = root.join("Caller.kt");
    std::fs::write(&caller_path, caller_source).unwrap();
    let caller_uri = Url::from_file_path(&caller_path).unwrap();
    indexer.index_content(&caller_uri, caller_source);

    let locations = resolve_symbol_index_only(&indexer, "Double", None, &caller_uri);
    assert_eq!(
        locations,
        vec![tower_lsp::lsp_types::Location {
            uri: declaration_uri,
            range: locations
                .first()
                .map(|location| location.range)
                .unwrap_or_default(),
        }],
        "the same-package workspace declaration must still win, got {locations:?}"
    );
}
```

- [ ] **Step 2: Verify the tests fail for the right reason**

```
cargo test --bin kmp-lsp a_builtin_type_name_prefers_the_platform_declaration     # FAIL: kotlin-stdlib decoy
cargo test --bin kmp-lsp a_mutable_builtin_type_name_prefers_the_platform         # FAIL: kotlin-gradle-plugin decoy
cargo test --bin kmp-lsp a_workspace_declaration_still_outranks_the_builtin       # PASS (guard)
```

Read the failure text, not just "FAILED". Test 1 must fail showing the
`kotlin/collections/List.class` URI — if it fails showing `com/example/List.class` instead, the
default-import tie-break is not being exercised and the fixture is wrong; if it fails empty, the
fake SDK was not picked up (check `local.properties` landed at `root`).

Also re-run the two tie-break tests that touch these names before implementing, to record they are
green now: `kotlin_default_import_tie_break_does_not_apply_from_a_java_origin_file`
(`src/resolver/tests.rs:11025`) and the `com.android.internal` denylist test just above it. Both use
`Indexer::new()` with **no** `workspace_root`, so the platform fallback short-circuits at
`platform_types.rs:128` and they must stay green after the reorder. If either goes red, the reorder
was placed above steps 1–4.5 by mistake.

- [ ] **Step 3: Implement the reorder**

In `src/resolver/resolve.rs`, in each of the four tails, move the
`resolve_kotlin_builtin_type_platform_equivalent(indexer, name)` call *above* the global-definitions
lookup and return it when non-empty:

```rust
// 5.4 ── Kotlin built-in-type platform equivalent ──────────────────────
// Ahead of the global definitions tail, not behind it: for the ~22
// compiler-intrinsic mapped types in
// KOTLIN_BUILTIN_TYPE_PLATFORM_EQUIVALENTS there is no compiled `.class`
// anywhere, so every same-named index/JAR candidate is a decoy -- and a
// decoy wins today, either outright (MutableSet: one candidate) or via
// default_kotlin_import_tie_break, which prefers the `kotlin.collections`
// decoy out of `List`'s 32 candidates. Anchoring `List` on a body-less
// decoy leaves it with no walkable supertype chain, so no member or
// extension lookup downstream can ever reach `Iterable`.
let platform_equivalent = resolve_kotlin_builtin_type_platform_equivalent(indexer, name);
if !platform_equivalent.is_empty() {
    return platform_equivalent;
}
```

`ScopedOnly` keeps its `vec![]` arm untouched. Keep the existing global-definitions blocks exactly as
they are, now reached only when the platform fallback declines (which is every name outside the
22-entry table, and every project without Android SDK sources).

Rewrite `resolve_kotlin_builtin_type_platform_equivalent`'s doc comment (`platform_types.rs:88-93`)
per Fix 1 above — the current "every normal resolution step already failed … can only ever turn a
decline into a resolve" claim is false and must not survive this change. Update the test-module
banner at `src/resolver/tests.rs:9272-9280` the same way.

Add the `android_sdk_source_roots` memo from Decision D1 to `platform_types.rs` and call it at
`:132` in place of `crate::workspace_json::detect_android_sdk_source_paths`. This is part of the same
task, not a follow-up: without it the reorder ships a `log::info!` on every built-in-name resolution.

- [ ] **Step 4: Verify**

```
cargo test --bin kmp-lsp          # expect 1956 + 3 new = 1959 passed, 0 failed, 3 ignored
cargo clippy --all-targets -- -D warnings
cargo fmt -- --check
```

- [ ] **Step 5: Measure on the real corpus**

```
cargo build --release
time ./target/release/kmp-lsp resolution-accuracy /home/ocel/Work/Moneta/android
```

Record: member recall before/after, bare recall, `FilteredCandidate` total, the member Gap top-20
diff, **and wall-clock time both runs** (Decision D1).

Success criteria, in order:
1. **No regression.** Member recall must not fall outside the ±0.3–0.6pp noise band. Baseline to beat:
   91.7% (151280/164972) member, 73.5% bare, FilteredCandidate 6234, member Gap 7458.
2. Gap names that depend on a `List`/`Map`/`Set`-typed receiver having real supertypes should thin
   out, but see the Fix 1 prediction — `firstOrNull` specifically is expected to **stay** until Task 2
   lands. Do not treat that as failure.
3. Wall clock recorded for both runs — not a gate (the D1 memo ships in this same task), just so the
   reorder's cost is on the record. Also confirm the `"android-sdk: auto-detected sources at …"`
   log line appears a small, constant number of times in the run's stderr, not once per built-in-name
   resolution: that is the cheapest direct check that the memo is actually working.

- [ ] **Step 6: Commit and open a PR**

The PR description must state: the real mechanism (default-import tie-break preferring a body-less
decoy, not a unique-match shortcut), the exact 7-of-22 before/after table from Part 0.2, and the D1
memo with the measured wall clock.

```bash
git commit -m "fix(resolve): prefer a built-in type's platform declaration over a same-named decoy

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 2: Kotlin's default-import packages count as in-scope for an extension entry

**Depends on nothing; must precede Tasks 3 and 4** (it edits `extension.rs` and `infer.rs`, which
they rewrite).

**Files:**
- Modify: `src/resolver/imports.rs` — widen `is_default_import_package` (`:46`) to `pub(super)`
- Modify: `src/resolver/infer.rs` — new `extension_entry_is_in_scope` next to `extension_is_in_scope`
  (`:1791-1850`); swap the call at `:1936-1945`
- Modify: `src/resolver/extension.rs` — swap the calls at `:91-98` and `:178-185`
- Modify: `src/resolver/tests.rs` — 4 new tests

**Interfaces:** `extension_is_in_scope` keeps its signature and all six callers keep compiling; three
of them switch to the new wrapper. New `pub(crate) fn extension_entry_is_in_scope(&ExtensionEntry,
&Url, Option<&FileData>) -> bool`.

- [ ] **Step 1: Write the failing tests**

> **Trap — do not write this as a naive end-to-end test.** Per Part 0.5,
> `find_definition_qualified_index_only("isNotEmpty", Some("CharSequence"), …)` spells the *type name*
> out, which makes the qualifier a `QualifierRoot::TypePath`, which reaches the **scope-blind**
> `jar_extension_for_type_root` fallback (`qualified.rs:126-129`) and resolves **green before the
> fix**. The rejection this task fixes is only fatal on a *value* root. Test at the
> `resolve_extension_in_scope` layer, where the behaviour actually lives.

Append to `src/resolver/tests.rs`:

```rust
/// Builds an indexer whose registry holds one `kotlin.text`-packaged
/// extension on `CharSequence`, plus a caller file in an unrelated package
/// with no imports at all -- the shape of every real Kotlin call site, since
/// nobody writes `import kotlin.text.isNotEmpty`.
fn indexer_with_a_default_imported_extension(caller_path: &str) -> (Indexer, Url) {
    use crate::types::{ExtensionEntry, FileData, SourceSet, SymbolEntry, Visibility};
    use std::sync::Arc;
    use tower_lsp::lsp_types::{Position, Range, SymbolKind};

    let indexer = Indexer::new();
    let jar_uri = "jar:file:///kotlin-stdlib.jar!/kotlin/text/StringsKt.class".to_owned();
    let declaration_range = Range {
        start: Position {
            line: 7,
            character: 0,
        },
        end: Position {
            line: 7,
            character: 10,
        },
    };
    let detail = "fun CharSequence.isNotEmpty(): Boolean".to_owned();

    indexer.jar_files.insert(
        jar_uri.clone(),
        Arc::new(FileData {
            symbols: vec![SymbolEntry {
                name: "isNotEmpty".to_owned(),
                kind: SymbolKind::FUNCTION,
                visibility: Visibility::Public,
                range: declaration_range,
                selection_range: declaration_range,
                detail: detail.clone(),
                container: None,
                params: String::new(),
                param_counts: (0, 0),
                cold: crate::types::pack_cold_fields(
                    vec![],
                    "CharSequence".to_owned(),
                    String::new(),
                    String::new(),
                ),
                trailing_lambda: false,
                deprecated: false,
            }],
            source_set: SourceSet::Library,
            package: Some("kotlin.text".to_owned()),
            lines: Arc::new(vec![]),
            ..Default::default()
        }),
    );
    indexer
        .extension_by_receiver
        .entry("CharSequence".to_owned())
        .or_default()
        .push(ExtensionEntry {
            file_uri: jar_uri,
            name: "isNotEmpty".to_owned(),
            kind: SymbolKind::FUNCTION,
            detail,
            visibility: Visibility::Public,
            package: Some("kotlin.text".to_owned()),
            trailing_lambda: false,
            deprecated: false,
            container: None,
        });

    let caller_uri = uri(caller_path);
    indexer.index_content(&caller_uri, "package app\nclass Caller\n");
    (indexer, caller_uri)
}

/// Kotlin's default-import packages (`kotlin`, `kotlin.text`,
/// `kotlin.collections`, `java.lang`, ...) are in scope in every Kotlin file
/// with no `import` line -- and no real file writes one. Before this fix
/// `extension_is_in_scope` knew only about same-package and explicit
/// imports, so it rejected all 3638 top-level default-import-packaged
/// registry entries for every caller on the Moneta corpus.
#[test]
fn a_default_import_packaged_extension_is_in_scope_without_an_explicit_import() {
    let (indexer, caller_uri) = indexer_with_a_default_imported_extension("/app/Caller.kt");

    let locations = super::extension::resolve_extension_in_scope(
        &indexer,
        "CharSequence",
        "isNotEmpty",
        &caller_uri,
    );
    assert_eq!(
        locations.len(),
        1,
        "kotlin.text.isNotEmpty must be in scope for an unimporting Kotlin \
         caller, got {locations:?}"
    );
    assert_eq!(
        locations[0].range.start.line, 7,
        "expected the real declaration range, got {:?}",
        locations[0].range
    );
}

/// Decision D2's guard: `resolve_qualified` runs over indexed `.java` files
/// too (the resolution-accuracy benchmark scans both `.kt` and `.java`), and
/// a Java file never implicitly imports `kotlin.*`. Same reasoning, and the
/// same Copilot review finding, that already gates
/// `default_kotlin_import_tie_break` on the origin file's language.
#[test]
fn a_default_import_packaged_extension_stays_out_of_scope_for_a_java_caller() {
    let (indexer, caller_uri) = indexer_with_a_default_imported_extension("/app/Caller.java");

    let locations = super::extension::resolve_extension_in_scope(
        &indexer,
        "CharSequence",
        "isNotEmpty",
        &caller_uri,
    );
    assert!(
        locations.is_empty(),
        "a Java origin must not pick up Kotlin's default imports, got {locations:?}"
    );
}

/// The new rule must NOT leak to a package that merely starts with a
/// default-import prefix: `kotlin.text.regex` is not `kotlin.text`, and
/// `is_default_import_package` is an exact-membership check, not a prefix
/// match. Guard against a future "fix" that swaps it for `starts_with`.
///
/// Targets `extension_entry_is_in_scope`, NOT `extension_is_in_scope` — the
/// latter is the function this task deliberately leaves untouched (Part 0.3),
/// so a guard aimed at it would be green before and after by construction and
/// pin nothing.
#[test]
fn a_package_below_a_default_import_package_is_still_out_of_scope() {
    use crate::resolver::infer::extension_entry_is_in_scope;
    use crate::types::{ExtensionEntry, Visibility};
    use tower_lsp::lsp_types::SymbolKind;

    let indexer = Indexer::new();
    let caller_uri = uri("/app/Caller.kt");
    indexer.index_content(&caller_uri, "package app\nclass Caller\n");
    let caller_file_data = indexer.files.get(caller_uri.as_str());

    let entry = ExtensionEntry {
        file_uri: "jar:file:///kotlin-stdlib.jar!/kotlin/text/regex/RegexKt.class".to_owned(),
        name: "someExtension".to_owned(),
        kind: SymbolKind::FUNCTION,
        detail: "fun CharSequence.someExtension(): Boolean".to_owned(),
        visibility: Visibility::Public,
        package: Some("kotlin.text.regex".to_owned()),
        trailing_lambda: false,
        deprecated: false,
        container: None,
    };
    assert!(
        !extension_entry_is_in_scope(
            &entry,
            &caller_uri,
            caller_file_data.as_deref().map(|value| value.as_ref()),
        ),
        "kotlin.text.regex is not one of Kotlin's default-import packages"
    );
}

/// The third call site, on the type-inference side: an unimporting Kotlin
/// caller must now be able to infer a `kotlin.collections` extension's
/// return type, which `find_extension_fn_return_type_scoped`'s own
/// in-scope check rejected before this fix.
#[test]
fn a_default_import_packaged_extension_return_type_is_inferable_without_an_import() {
    use crate::types::{ExtensionEntry, FileData, SourceSet, Visibility};
    use std::sync::Arc;
    use tower_lsp::lsp_types::SymbolKind;

    let indexer = Indexer::new();
    let jar_uri = "jar:file:///kotlin-stdlib.jar!/kotlin/collections/CollectionsKt.class".to_owned();
    indexer.jar_files.insert(
        jar_uri.clone(),
        Arc::new(FileData {
            source_set: SourceSet::Library,
            package: Some("kotlin.collections".to_owned()),
            lines: Arc::new(vec![]),
            ..Default::default()
        }),
    );
    indexer
        .extension_by_receiver
        .entry("List".to_owned())
        .or_default()
        .push(ExtensionEntry {
            file_uri: jar_uri,
            name: "firstOrNull".to_owned(),
            kind: SymbolKind::FUNCTION,
            detail: "fun <T> List<T>.firstOrNull(): T?".to_owned(),
            visibility: Visibility::Public,
            package: Some("kotlin.collections".to_owned()),
            trailing_lambda: false,
            deprecated: false,
            container: None,
        });

    let caller_uri = uri("/app/Caller.kt");
    indexer.index_content(&caller_uri, "package app\nclass Caller\n");

    assert_eq!(
        crate::resolver::infer::find_extension_fn_return_type(
            &indexer,
            "List",
            "firstOrNull",
            Some(&caller_uri),
        ),
        Some("T?".to_owned()),
        "an unimporting Kotlin caller must be able to infer a \
         kotlin.collections extension's return type"
    );
}
```

- [ ] **Step 2: Verify they fail for the right reason**

Test 3 names `extension_entry_is_in_scope`, which does not exist yet, so the crate does not compile
until Step 3 adds it. **Write Step 3's function signature first (an empty body returning
`extension_is_in_scope(...)`'s result, no default-import rule yet), then run:**

```
cargo test --bin kmp-lsp a_default_import_packaged_extension_is_in_scope          # FAIL: 0 locations
cargo test --bin kmp-lsp a_default_import_packaged_extension_stays_out_of_scope   # PASS (guard)
cargo test --bin kmp-lsp a_package_below_a_default_import_package                 # PASS (guard)
cargo test --bin kmp-lsp a_default_import_packaged_extension_return_type          # FAIL: None
```

Two of the four are green-before-fix by design — they guard against over-reach (a Java caller; a
package merely *below* a default-import package), so there is no bug for them to catch first. That is
deliberate; do not "fix" them into reds. The other two must be genuinely red, and the empty-body
step is what makes that observable rather than a compile error.

- [ ] **Step 3: Implement**

1. `src/resolver/imports.rs:46` — `fn is_default_import_package` → `pub(super) fn is_default_import_package`.
2. `src/resolver/infer.rs` — add `extension_entry_is_in_scope` exactly as written in Fix 2 above,
   directly below `extension_is_in_scope`. Do **not** modify `extension_is_in_scope` itself.
3. Swap the three call sites (`extension.rs:91-98`, `extension.rs:178-185`, `infer.rs:1936-1945`) to
   the wrapper. Each becomes a single `if !extension_entry_is_in_scope(entry, from_uri, caller_file_data_ref) { continue; }`.
4. Leave `src/features/nullable_call_diagnostics.rs:275`, `src/resolver/infer.rs:1626` and
   `src/indexer.rs:1099` alone. Add a one-line note to `extension_is_in_scope`'s doc comment pointing
   at the new wrapper, so the next reader knows which one an extension-registry consumer should use.

- [ ] **Step 4: Verify**

```
cargo test --bin kmp-lsp          # expect 1956 + 4 new = 1960 passed, 0 failed, 3 ignored
cargo clippy --all-targets -- -D warnings
cargo fmt -- --check
```

If any pre-existing test goes red, read it before changing it: a test asserting "out of scope" for a
package that is **not** in `KOTLIN_DEFAULT_IMPORT_PACKAGES` indicates a real bug in the fix (a prefix
match instead of exact membership, most likely). A test asserting out-of-scope for a package that
**is** in the set is asserting the bug and should be discussed with the controller, not silently
rewritten. (Checked in advance: `top_level_extension_function_in_unimported_package_still_out_of_scope`
at `tests.rs:7219` uses `com.example.lib` and is unaffected.)

- [ ] **Step 5: Measure on the real corpus**

```
cargo build --release
time ./target/release/kmp-lsp resolution-accuracy /home/ocel/Work/Moneta/android
```

Success criteria:
1. **PREDICTION to verify in this run, not an established fact:** `isNotEmpty` and `forEach` leave
   the member-ref Gap top-20, and the 0-arg `firstOrNull` shape starts resolving. What the probe
   *did* establish is narrower — that all five probed `kotlin.text`/`kotlin.collections` entries are
   `in_scope=false` today. It did **not** establish that those names are currently in the Gap top-20
   (the 7458-entry member Gap was not enumerated by name in this plan's probe), nor that scope is
   their *only* blocker — a name can clear the scope check and still Gap on receiver-type inference,
   arity, or a missing anchor. **Step 1 of this measurement is therefore to record the before-run's
   Gap top-20 and check whether these names are even in it.** If they are not, this criterion is
   unfalsifiable as written and the real criterion is #2 alone; say so in the PR rather than claiming
   a win the run cannot support.
2. Member recall up. This is the cluster's biggest lever — 3641 previously-unreachable registry
   entries become reachable — so a movement outside the noise band is expected here, unlike Task 1.
3. **Watch `FilteredCandidate` (baseline 6234).** Handing the shape filter more candidates is the
   point, but a large rise means in-scope candidates are being surfaced and then rejected on arity,
   which would point at Task 3/Task 4 territory. Record the number either way; a rise is not by itself
   a reason to hold the PR.

- [ ] **Step 6: Commit and open a PR**

The PR description must state: why the fix is a new wrapper rather than an edit to
`extension_is_in_scope` (four of six callers are not about extensions — Part 0.3), why the language
gate is there (Decision D2, with the `.java`-files-are-scanned evidence), and that
`nullable_call_diagnostics` was deliberately left out.

```bash
git commit -m "fix(resolve): count Kotlin's default-import packages as extension scope

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 3: `implicit_receiver_extension_match` selects its own entry's overload

**Depends on Task 2** (same file). No corpus measurement required.

**Files:**
- Modify: `src/resolver/extension.rs` — split `select_extension_symbol_range` (`:28-54`), rewrite the
  symbol lookup in `implicit_receiver_extension_match` (`:192-209`)
- Modify: `src/features/rename.rs` — Decision D5's `//` line comment above the
  `definitions.len() != 1` refusal (`:106-110`). It is inside `rename_impl`'s body, so a `///` doc
  comment would not compile there. Comment only, no behaviour change.
- Modify: `src/resolver/tests.rs` — 1 new test

**Interfaces:** new `pub(super) fn select_extension_symbol(&FileData, &str, &str, Option<&String>,
&str) -> Option<&SymbolEntry>`. `select_extension_symbol_range` keeps its exact signature and both
existing callers.

- [ ] **Step 1: Write the failing test**

```rust
/// PR #321's overload-collapse family, on the implicit-receiver entry point
/// (`resolve_implicit_receiver_callee` -> `implicit_receiver_extension_match`).
/// The registry correctly holds one entry per overload, but for EVERY entry
/// the loop re-ran the same `extension_declaration_matches` `.find(...)` over
/// the declaring file -- and that predicate compares only
/// (name, receiver, container), identical across overloads -- so every
/// iteration shape-checked the FIRST-declared overload and a differently
/// shaped call was rejected on all of them. The 1-arg overload was
/// unreachable through this entry point entirely.
#[test]
fn an_implicit_receiver_call_reaches_the_arity_matching_extension_overload() {
    use crate::indexer::CallShape;

    let indexer = Indexer::new();
    let extensions_uri = uri("/app/Extensions.kt");
    indexer.index_content(
        &extensions_uri,
        concat!(
            "package app\n",
            "fun Foo.describe(): String = \"\"\n",
            "fun Foo.describe(prefix: String): String = prefix\n",
        ),
    );
    let caller_uri = uri("/app/Caller.kt");
    indexer.index_content(
        &caller_uri,
        concat!(
            "package app\n",
            "class Foo\n",
            "fun Foo.use() {\n",
            "    describe(\"x\")\n",
            "}\n",
        ),
    );

    let shape = CallShape {
        arg_count: 1,
        trailing_lambda: false,
    };
    let locations =
        crate::resolver::resolve_implicit_receiver_callee(&indexer, "Foo", "describe", &caller_uri, shape);
    assert_eq!(
        locations.len(),
        1,
        "expected the 1-arg overload, got {locations:?}"
    );
    assert_eq!(
        locations[0].range.start.line, 2,
        "expected the SECOND declaration (the 1-arg overload, line index 2), \
         got {:?}",
        locations[0].range
    );
}
```

- [ ] **Step 2: Verify it fails for the right reason**

```
cargo test --bin kmp-lsp an_implicit_receiver_call_reaches_the_arity_matching_extension_overload
```

Must fail with **0 locations** (both entries selected the 0-arg declaration, whose arity the 1-arg
shape rejects). If it fails with 1 location at line 1, the loop is picking the wrong overload for a
different reason — investigate before implementing. If it passes, the fixture is leaking through
`implicit_receiver_member_match` or a bare-name fallback; add a decoy so it does not.

- [ ] **Step 3: Implement**

Split `select_extension_symbol_range` into `select_extension_symbol` + a two-line
`select_extension_symbol_range` wrapper, exactly as written in Fix 3 above. Move
`select_extension_symbol_range`'s existing doc comment onto `select_extension_symbol` (it describes
the selection, not the range extraction) and leave a one-line comment on the wrapper.

In `implicit_receiver_extension_match`, replace `:196-208`'s
`.and_then(|fd| fd.symbols.iter().find(|s| …).cloned())` with

```rust
let symbol = indexer
    .files
    .get(&entry.file_uri)
    .or_else(|| indexer.jar_files.get(&entry.file_uri))
    .and_then(|file_data| {
        select_extension_symbol(
            &file_data,
            name,
            receiver_base,
            entry.container.as_ref(),
            &entry.detail,
        )
        .cloned()
    });
```

Note the closure parameter rename from `fd` to `file_data` and the inner `|s|` disappearing. Do both,
but for the right reason: AGENTS.md's no-abbreviated-names rule, not the pre-push hook. The hook's
abbreviation check is a fixed token list (`\b([sc])\b` plus a specific suffix list) that does **not**
catch `fd` or `pkg` — so this rename is house style, not gate-enforced, and nobody should be told
otherwise and then discover the gate was never watching.

Then add Decision D5's note to `src/features/rename.rs`, above the `definitions.len() != 1` refusal:

```rust
// A same-arity extension collision (two unrelated `Modifier.weight`
// declarations, say) reaches this refusal BY DESIGN since PR #321:
// `resolve_identity` correctly returns the whole overload set rather than
// one arbitrary member of it, and silently renaming an arbitrarily chosen
// one of two genuinely distinct declarations is the unsound behaviour that
// PR replaced. Listing both candidates in the message would not help --
// an LSP error is surfaced as a toast with no navigation affordance -- so a
// real disambiguation UX needs a client-side picker this server has no
// protocol hook for. Reviewed and kept as-is, 2026-09-16.
```

- [ ] **Step 4: Verify**

```
cargo test --bin kmp-lsp          # expect Task 2's count + 1 new
cargo clippy --all-targets -- -D warnings
cargo fmt -- --check
```

- [ ] **Step 5: Spot-check on the real corpus (no full benchmark)**

This entry point has no currently-measured Gap name pointing at it and the change is confined to one
loop, so a full `resolution-accuracy` run is not the right instrument. Instead:

```
cargo build --release
cd /home/ocel/Work/Moneta/android
# pick a call inside an extension function's own body that targets a
# same-receiver overload set -- `rg -n "^fun [A-Z]\w*\." --glob '*.kt'` finds
# candidate declaring files; the registry groups named in Fix 3
# (HttpClientCall.receive, Writer.write) are the shape to look for.
kmp-lsp definition <file> <line> <col>
```

Record one before/after pair in the PR description. If no such site can be found on this corpus,
say so explicitly and rest on the unit test — do not invent a site.

- [ ] **Step 6: Commit and open a PR**

```bash
git commit -m "fix(resolve): select the implicit-receiver extension overload the call shape wants

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 4: `find_extension_fn_return_type_scoped` — per-entry selection, no loop abort

**Depends on Task 3** (consumes `select_extension_symbol`). No corpus measurement.

**Files:**
- Modify: `src/resolver/infer.rs` (`find_extension_fn_return_type_scoped`, `:1955-1974`)
- Modify: `src/resolver/infer_tests.rs` — 2 new tests

**Interfaces:** none change.

- [ ] **Step 1: Write the failing tests**

> **Both tests below were written into the tree, run red, fixed, and run green before this plan was
> finalized** — see Appendix C. Their first draft was **false-green** and is preserved in Appendix C
> as a trap to avoid repeating. Two things make this fixture shape mandatory:
>
> 1. **`index_content` auto-registers extensions.** `ExtensionContribution::apply`
>    (`src/indexer/apply.rs:504-509`) and the merge path (`:730-733`) populate
>    `extension_by_receiver` from every indexed file. A fixture that indexes the declaring file *and*
>    hand-pushes its own `ExtensionEntry`s ends up with both sets in the bucket, and the auto-registered
>    ones — which are correct — shadow the hand-written ones so the buggy path never runs.
>    **Let `index_content` build the entries; do not hand-push alongside it.**
> 2. **A hand-written `entry.detail` can never equal a parsed `SymbolEntry.detail`.** The fallback path
>    this task fixes only runs when `extract_return_type_from_detail(&entry.detail)` returns `None`,
>    i.e. when the detail is truncated past the return type — and the fix works by matching
>    `entry.detail` against `symbol.detail`. Author a mangled `entry.detail` by hand and the two can
>    never match, so `select_extension_symbol` always falls back to the first declaration and the test
>    stays red *after* the fix too. **Force the truncation with a genuinely long signature in real
>    source**, the way `extension_fn_return_type_scoped_resolves_member_extension_via_truncated_detail`
>    (`infer_tests.rs:702`) already does with `"w".repeat(150)`: both details then come from the same
>    `MAX_DETAIL_CHARS = 120` truncation (`src/parser.rs:1219`) and are equal by construction, and
>    `lines` is populated so `collect_signature` can still recover the real return type.
>
> Also note `src/resolver/infer_tests.rs` has **no module-level `uri()` helper** — it is redeclared
> inside each test (`:109`, `:170`, `:214`, `:256`, `:1753`). Both tests below follow that convention
> and import `Indexer` themselves.

Append to `src/resolver/infer_tests.rs`, next to
`extension_fn_return_type_scoped_resolves_member_extension_via_truncated_detail` (`:702`).

```rust
/// The truncated-`detail` fallback picked the FIRST declaration in the
/// declaring file matching (name, receiver, container) -- a predicate
/// identical across every overload -- so a second overload's return type was
/// unreachable through it, no matter which registry entry the loop was on.
/// PR #321's `select_extension_symbol` already solves exactly this
/// disambiguation by preferring the declaration whose full signature text
/// equals the registry entry's own.
///
/// Both overloads are made long enough that `detail` truncates past the
/// return type (`MAX_DETAIL_CHARS`, `src/parser.rs:1219`), forcing the
/// `collect_signature` fallback -- the only path carrying the bug -- while
/// still differing inside the first 120 characters (`alpha` vs `beta`) so the
/// two truncated details remain distinguishable.
#[test]
fn extension_fn_return_type_scoped_selects_the_overload_the_entry_names() {
    use super::find_extension_fn_return_type;
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    let indexer = Indexer::new();
    let declaring_uri = uri("/app/Extensions.kt");
    let long_parameter = "w".repeat(150);
    indexer.index_content(
        &declaring_uri,
        &format!(
            "package app\n\
             class Foo\n\
             fun Foo.describe(alpha: Int, {long_parameter}: Int): String = \"\"\n\
             fun Foo.describe(beta: Int, {long_parameter}: Int): Boolean = true\n"
        ),
    );

    let caller_uri = uri("/app/Caller.kt");
    indexer.index_content(&caller_uri, "package app\nfun use() {}\n");

    // `find_extension_fn_return_type` returns on the first entry that yields
    // a return type, so the first entry's (correct) answer would mask the
    // bug. Drop it and the SECOND entry has to stand on its own -- which is
    // exactly what it cannot do today.
    indexer
        .extension_by_receiver
        .entry("Foo".to_owned())
        .or_default()
        .remove(0);

    assert_eq!(
        find_extension_fn_return_type(&indexer, "Foo", "describe", Some(&caller_uri)),
        Some("Boolean".to_owned()),
        "the remaining entry names the SECOND overload, so its return type \
         must be Boolean, not the first declaration's String"
    );
}

/// An entry whose declaring file is not loaded must skip to the next entry,
/// not abort the whole search: both `indexer.files.get(...)?`
/// (`infer.rs:1958`) and `.find(...)?` (`:1969`) returned `None` from the
/// FUNCTION rather than continuing the loop, so one unusable entry hid every
/// usable one behind it.
///
/// This entry is hand-built on purpose -- it must point at a file that is
/// deliberately never indexed, which `index_content` cannot produce -- and it
/// is `insert`ed ahead of the auto-registered real one so it is reached first.
/// Its `detail` carries no return type, so the loop falls past the
/// `extract_return_type_from_detail` shortcut to the file lookup that aborts.
#[test]
fn extension_fn_return_type_scoped_skips_an_entry_whose_file_is_missing() {
    use super::find_extension_fn_return_type;
    use crate::indexer::Indexer;
    use crate::types::{ExtensionEntry, Visibility};
    use tower_lsp::lsp_types::{SymbolKind, Url};

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    let indexer = Indexer::new();
    let declaring_uri = uri("/app/Extensions.kt");
    indexer.index_content(
        &declaring_uri,
        "package app\nclass Foo\nfun Foo.describe(): String = \"\"\n",
    );

    indexer
        .extension_by_receiver
        .entry("Foo".to_owned())
        .or_default()
        .insert(
            0,
            ExtensionEntry {
                file_uri: "file:///app/NeverIndexed.kt".to_owned(),
                name: "describe".to_owned(),
                kind: SymbolKind::FUNCTION,
                detail: "fun Foo.describe(".to_owned(),
                visibility: Visibility::Public,
                package: Some("app".to_owned()),
                trailing_lambda: false,
                deprecated: false,
                container: None,
            },
        );

    let caller_uri = uri("/app/Caller.kt");
    indexer.index_content(&caller_uri, "package app\nfun use() {}\n");

    assert_eq!(
        find_extension_fn_return_type(&indexer, "Foo", "describe", Some(&caller_uri)),
        Some("String".to_owned()),
        "an entry pointing at an unindexed file must be skipped, not abort \
         the search over the remaining entries"
    );
}
```

- [ ] **Step 2: Verify they fail for the right reason**

Observed, not predicted — this is the exact output from running them against unmodified `878cc4ea`:

```
extension_fn_return_type_scoped_selects_the_overload_the_entry_names ... FAILED
  left: Some("String")   right: Some("Boolean")     <- first-match bug
extension_fn_return_type_scoped_skips_an_entry_whose_file_is_missing ... FAILED
  left: None             right: Some("String")      <- early-abort bug
```

If either shows something else — an empty bucket, a panic, or `None` on test 1 — the fixture is not
reaching the `collect_signature` fallback. Do **not** weaken the assertion; re-read the fixture note
above, since one of its two traps is almost certainly back.

- [ ] **Step 3: Implement**

In `src/resolver/infer.rs:1955-1974`:

```rust
let Some(file_data) = indexer
    .files
    .get(&entry.file_uri)
    .or_else(|| indexer.jar_files.get(&entry.file_uri))
else {
    // Not `?`: one entry whose declaring file isn't loaded must not abort
    // the search over the remaining entries.
    continue;
};
let Some(declaring_symbol) = crate::resolver::extension::select_extension_symbol(
    &file_data,
    method_name,
    receiver_base,
    entry.container.as_ref(),
    &entry.detail,
) else {
    continue;
};
let start_line = declaring_symbol.selection_start() as usize;
let full_signature = file_data.lines.collect_signature(start_line);
if let Some(return_type) = extract_return_type_from_detail(&full_signature) {
    return Some(return_type);
}
```

`select_extension_symbol` is `pub(super)` in `resolver::extension`, so `resolver::infer` reaches it
as a sibling submodule — no visibility change needed.

Add a short comment above `find_extension_fn_return_type_global` recording Part 0.4: it has no
`ExtensionEntry` and no `detail`, so the two-step selection does not apply, and
`Indexer::find_method_return_type_for_type` always passes `Some(uri)` so it is not on a production
path. Also record Decision D4 there in one sentence — arity-aware selection needs a `CallShape`
through `Resolver::method_return_type`, 482 registry groups on the Moneta corpus have
differing-return overloads, and that is its own plan.

- [ ] **Step 4: Verify**

```
cargo test --bin kmp-lsp          # expect Task 3's count + 2 new
cargo clippy --all-targets -- -D warnings
cargo fmt -- --check
```

- [ ] **Step 5: Commit and open a PR**

The PR description must state what was fixed (per-entry selection + the loop abort), what was
deliberately **not** fixed (arity-aware overload choice), and the 482-group measurement that
motivates the follow-up plan.

```bash
git commit -m "fix(infer): select the named overload in scoped extension return-type lookup

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Item 5 — closed by Decision D5, no task

Ship the current refusal as-is. The only artefact is the doc comment added in Task 3 Step 3. See
Decision D5 for the reasoning; if the controller disagrees, the alternative is a client-side
disambiguation picker, which is a protocol-level feature and belongs in its own plan, not a message
tweak.

---

## Appendix A — how Part 0 was verified

One throwaway instrument, since deleted. A `throwaway_probe(&index, root)` function appended to
`src/cli/resolution_accuracy_poc.rs`, called from `run_resolution_accuracy` behind
`if std::env::var("KMP_PROBE").is_ok()` placed immediately **after** the benchmark's own JAR indexing
and **before** its file loop, so it saw the benchmark's exact index state. `mod platform_types;` in
`src/resolver/mod.rs` was temporarily widened to `pub(crate)`. Run as:

```
cd /home/ocel/Work/Moneta/android
KMP_PROBE=1 target/release/kmp-lsp resolution-accuracy /home/ocel/Work/Moneta/android
```

It printed four blocks:

1. **Built-in names.** For each of the 22 entries in `KOTLIN_BUILTIN_TYPE_PLATFORM_EQUIVALENTS`:
   `indexer.lookup_definitions(name).len()`, the first four candidate URIs,
   `resolve_kotlin_builtin_type_platform_equivalent(index, name)`, and
   `resolve_symbol_index_only(index, name, None, &caller)` where `caller` was the
   lexicographically-first indexed workspace `.kt` file. → Part 0.2's table.
2. **Extension scope.** For `(CharSequence, isNotEmpty)`, `(String, isNotEmpty)`,
   `(Iterable, firstOrNull)`, `(Iterable, forEach)`, `(List, firstOrNull)`: every registry entry's
   `package`, `container`, `detail`, and `extension_is_in_scope(…)` against that same caller. All
   `false`. → Fix 2's evidence table.
3. **Registry census.** 2360 buckets / 24897 entries / 3641 in a `KOTLIN_DEFAULT_IMPORT_PACKAGES`
   package / 3638 of those top-level (`container.is_none()`).
4. **Overload groups.** Grouping every bucket's entries by `name`: 4211 groups with ≥2 entries, 482
   with differing crudely-extracted return types, 2589 with ≥2 entries declared in the same file,
   plus 12 examples of each. → Fix 3's and Decision D4's evidence.

Both edits reverted; `git status` clean; `cargo test --bin kmp-lsp` = 1956 passed / 0 failed /
3 ignored at the state this document was written.

The call-graph claims (Part 0.3, Decision D2, Part 0.4's caller chain, Part 0.5's
`QualifierRoot::TypePath` gate) were established by reading the actual call sites, not by probe —
each is cited with its file and line above.

## Appendix B — what changed from the context brief

| # | Brief's claim | This plan |
|---|---|---|
| 0.1 | Item 1 is a false-**unique**-win: `lookup_definitions("List")` returns exactly one candidate. | **False.** 32 candidates; `default_kotlin_import_tie_break` narrows them to the `kotlin.collections` decoy. Different mechanism, and a "unique-match" fix would have missed `List` entirely. |
| 0.2 | The reorder "changes resolution for every name in that table (10+ names)"; `String` "not yet independently re-confirmed". | Enumerated all 22 on the corpus: **15 unchanged, 7 fixed**. `String` re-confirmed correct today and unaffected. The 7 split into two distinct failure shapes (tie-break loss vs. genuine unique decoy). |
| 0.3 | `extension_is_in_scope` has "only two callers"; fix it in place. | **Six callers**, four of them not about extensions — including `Indexer::jar_candidate_is_reachable`, which gates bare-name JAR candidate preference corpus-wide. Design changed to a new entry-shaped wrapper; the shared predicate is untouched. |
| 0.4 | Item 4's `_scoped` and `_global` are the same bug; fix both with `select_extension_symbol_range`'s pattern. | **Different shapes.** `_global` has no `ExtensionEntry` and no `detail`, so the pattern cannot apply, and no production caller reaches it. `_scoped` gains the pattern **plus** a newly-found early-abort bug (two `?`s that return from the function instead of the loop). |
| 0.4 / D4 | Item 4 should get "arity awareness". | **Scoped out with evidence.** No `CallShape` exists anywhere in the chain; adding one is a `Resolver`-trait change. Measured 482 differing-return registry groups as motivation for a separate plan, and flagged that 482 counts *groups*, not call sites. |
| Open question | "Does `extension_is_in_scope` ever run for a non-Kotlin origin file?" | **Resolved: yes.** `resolve_qualified` is language-agnostic and the `resolution-accuracy` benchmark itself scans `.java` files. The language gate is required, and is pinned by its own test (Decision D2). |
| Item 2's "compounding factor" | `jar_extension_for_type_root` is scope-blind, so the `CharSequence` case "worked" by same-leaf coincidence. | **Sharpened:** that fallback is reachable only for a `QualifierRoot::TypePath`. A real `text.isNotEmpty()` call has a *value* root and never reaches it — which is why the scope rejection is fatal in practice and why a naive end-to-end test of item 2 is green before the fix. |
| Item 5 | "Decide (a) ship as-is or (b) small UX improvement." | **Decision D5: (a), closed.** With a `//` comment recording why, folded into Task 3. |
| New | — | **Decision D1:** the reorder makes the un-memoized `detect_android_sdk_source_paths` run on hot names. A path-keyed memo ships in Task 1 itself. |
| New | — | **Fix 1 + Fix 2 are jointly necessary** for the trailing-lambda `firstOrNull` shape: even with a correct `List` anchor, the `Iterable`-keyed overloads are out of scope until Fix 2 lands. Stated so Task 1's measurement is not read as a failure. |

## Appendix C — Task 4's tests were run, red then green, before this plan shipped

Added after an independent review found Task 4's first-draft tests were **false-green**: both passed
against unmodified code, so Step 2's stated failures were predictions that had never been observed.
The rebuilt tests in Task 4 Step 1 were pasted into `src/resolver/infer_tests.rs` in this worktree
and run for real.

**Red, against unmodified `878cc4ea`:**

```
running 4 tests
test …extension_fn_return_type_scoped_promotes_a_cache_backed_tier1_only_symbol ... ok
test …extension_fn_return_type_scoped_resolves_member_extension_via_truncated_detail ... ok
test …extension_fn_return_type_scoped_skips_an_entry_whose_file_is_missing ... FAILED
test …extension_fn_return_type_scoped_selects_the_overload_the_entry_names ... FAILED

selects_the_overload_the_entry_names:  left: Some("String")  right: Some("Boolean")
skips_an_entry_whose_file_is_missing:  left: None            right: Some("String")
```

A temporary probe inside test 1 also dumped the auto-registered entries, confirming the two details
truncate to distinguishable strings — the property the fix depends on:

```
"fun Foo.describe(alpha: Int, wwwwwww…"
"fun Foo.describe(beta: Int, wwwwwwww…"
```

**Green, with Task 3's `select_extension_symbol` split and Task 4's implementation applied exactly as
written in those tasks:**

```
test …extension_fn_return_type_scoped_skips_an_entry_whose_file_is_missing ... ok
test …extension_fn_return_type_scoped_selects_the_overload_the_entry_names ... ok
```

Full suite at that state: **1958 passed, 0 failed, 3 ignored** (= the 1956 baseline + these 2), and
`cargo clippy --all-targets -- -D warnings` clean. That also pre-validates Task 3's helper split and
Task 4's implementation compile and break nothing — the two production diffs were written and
compiled, not just designed. All of it was then reverted; only this plan file is staged.

**What the false-green first draft got wrong, recorded so it is not repeated:** it hand-pushed
`ExtensionEntry`s *alongside* an `index_content` call that already auto-registers them
(`src/indexer/apply.rs:504-509`), so the correct auto-entries shadowed the fixture and the buggy path
never ran; and it hand-authored a mangled `entry.detail` to force the truncated-detail fallback,
which by construction can never equal a parsed `SymbolEntry.detail` — so `exact_signature_match`
could never fire and the test would have stayed red even *after* a correct fix. Both traps are now
called out in Task 4 Step 1's fixture note.

## Appendix D — what changed in the independent-review round

An independent reviewer re-derived every claim from scratch against the repo and corpus, and ran
several of the plan's own tests. Verdict: specific fixes, not a rewrite. Everything the reviewer
**confirmed** — the 32-candidate claim, the 7-of-22 table (re-run against three different caller
files, identical result), six-callers-not-two, the `_global`/`_scoped` split, the two `?` early-abort
bugs at `infer.rs:1958`/`:1969`, Task 3's test being a genuine red, the Task 3 → Task 4 helper
signature match, the ordering/conflict analysis, and the commit trailers — is retained unchanged.

| # | Finding | Resolution |
|---|---|---|
| **1 (blocker)** | Task 4's two tests were **false-green** — both passed against unmodified code, proven by running them. Two causes: `index_content` auto-registers extensions (`apply.rs:504-509`), shadowing the hand-pushed fixture; and a hand-authored `entry.detail` can never equal a parsed `SymbolEntry.detail`, so the fix could never fire. | **Fixed.** Both tests rebuilt on the real-source truncation pattern from `infer_tests.rs:702`, then **actually run** — real red, then real green with the fix applied, full suite 1958/0/3, clippy clean. Evidence in **Appendix C**, including both traps written into Task 4 Step 1's fixture note. |
| **2** | Task 4's tests would not compile: `infer_tests.rs` has no module-level `uri()` helper and they never imported `Indexer`. | **Fixed.** Both now declare a local `uri` and `use crate::indexer::Indexer;`, matching the file's per-test convention. Verified by the run. |
| **3** | `let _ = (SourceSet::Library, Arc::new(FileData::default()));` was a junk line silencing unused imports — the "trim if unused" note shipped as a placeholder. | **Fixed.** Line and both unused imports gone. |
| **4** | Task 2's third test guarded `extension_is_in_scope` — the function Task 2 explicitly does *not* modify — so it was green before and after by construction. | **Fixed.** Rewritten to call `extension_entry_is_in_scope` with a `kotlin.text.regex`-packaged `ExtensionEntry`, pinning the new wrapper's exact-membership rule. Step 2 now also says to stub the signature first so the other two reds are observable rather than a compile error. |
| **5** | D1's generation-keyed-memo-deferred-behind-a-measurement was over-built; and the plan missed that `workspace_json.rs:681` `log::info!`s on every detection, which is its own reason not to ship unmemoized even briefly. | **Adopted, not argued.** D1 rewritten: a path-keyed `Mutex<Option<(PathBuf, Vec<PathBuf>)>>` memo, ~15 lines, **shipping in Task 1** rather than deferred. Placed in `platform_types.rs` (the only hot caller) rather than `workspace_json.rs`. Wall clock demoted from gate to record; a log-line-count check added as the direct memo verification. |
| **6** | "3638 of the registry's 24897 top-level entries" conflated two different denominators; "all four `tests.rs` edits" was a count error (only three tasks touch `tests.rs`). | **Fixed,** both. |
| **7** | The pre-push hook's abbreviation check is a fixed token list that does not catch `fd`/`pkg`, so citing it as the authority for Task 3's `fd` → `file_data` rename was an overclaim. | **Fixed.** Rename kept (AGENTS.md is the authority); the Global Constraints block now says explicitly that the hook is a partial backstop, not the definition of the rules. |
| **8** | D5's "LSP clients surface this as a toast" claim — the load-bearing reason for choosing (a) — was never verified against a real client. Files table called a function-body comment a "doc-comment note". | **Fixed.** The claim is now labelled **ASSUMPTION** with what would follow if it is wrong (reasons 1/2/4 carry the decision regardless). Both Files-table entries corrected to `//` line comment. |
| **9** | Task 2 Step 5 stated "`isNotEmpty`/`forEach` leave the Gap top-20" as fact; the probe only established they are out of scope today, not that they are in the top-20 or that scope is their only blocker. | **Fixed.** Reworded as a prediction, with an explicit first step to record the before-run's Gap top-20 and check whether the names are even in it — and instructions to say so in the PR if the criterion turns out unfalsifiable. |
| **10** | `InitUseCase.kt:73` is a `data class Double` **nested** in `sealed class IntRates`, not a top-level declaration. | **Fixed** in both places (design prose and the guard test's doc comment). Confirmed independently by reading the file. The guard test uses its own top-level fixture and was never affected. |
