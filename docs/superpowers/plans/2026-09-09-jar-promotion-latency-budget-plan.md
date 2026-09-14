# `resolve.rs` Gap Closure and JAR-Promotion Latency Budget — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> ## ⚠ REVISION 2026-09-10 — read this before anything else
>
> This plan was written on 2026-09-09 as a *latency-budget architecture* plan whose headline
> justification was closing `navController.navigate(route = "...")` on the Moneta corpus. **That
> justification is gone.** Task 0's own measurement disproved it, a re-diagnosis found a completely
> different bug, and that bug is now fixed and shipping:
>
> | | What it is | State |
> |---|---|---|
> | **PR #314** (`e8080a59`) | `with_supertype_extension_fallback` — appends a supertype-walk extension after a wrong-arity member | **OPEN**, on this branch. Real fix. **Zero measured effect on Moneta.** |
> | **PR #315** (`f0e5ed56`, cherry-picked to `fix/jar-container-scoping-navigate-leak` as `9e6f5320`) | Container-scoping: `find_name_in_uri_after_line` gained a `container_name` filter; `resolve_from_class_hierarchy_scoped`'s ancestor walk switched to the container/overload-aware `find_all_names_with_container_in_uri` | **OPEN**, on this branch. **This is what actually closed Moneta's `navigate`: 220–224 Gap occurrences → 0.** |
>
> Full narrative, including all three false starts and the three critique rounds on the now-parked
> budget architecture:
> `/home/ocel/Work/lsp_tasks/2026-09-09-navhostcontroller-navigate-container-scoping-and-latency-budget-research.md`
>
> **Consequences for this plan, all applied below:**
> 1. Task 0 is **DONE** — see the continuation section appended to it (fresh corpus numbers, and a
>    bug-class taxonomy of the entire current Gap top-20).
> 2. A **new Task 1** (Java getter → Kotlin synthetic property) replaces the old Task 1 at the front
>    of the queue. It is the only remaining top-20 finding with hard measured evidence behind it.
> 3. A **new Task 2** splits `resolve.rs`. This was "Deliberately out of scope"; the reason it was
>    out of scope (signature churn from Tasks 3–5) no longer applies now that the budget work is
>    parked. Boundaries below were **re-verified against the current file on 2026-09-10**, not
>    inherited from the design doc — the design doc's own numbers were materially wrong (see Task 2).
> 4. The old Task 1 (own-type extension tier) becomes **Task 3**, still real, still red, but
>    **no longer justified by any measured corpus gap** — stated plainly rather than left implying
>    it closes Moneta.
> 5. The old Task 4 (`hierarchy.rs` budget leak) is **promoted out of the budget block** as Task 4 —
>    it is a standalone correctness bug that needs no `LatencyClass` at all.
> 6. The old Tasks 2/3/5/6 (the `LatencyClass` architecture) are **PARKED** as Tasks 5–8 behind an
>    explicit, measurable resumption gate. They are good work; they are aimed at nothing currently
>    measured.

> ## ⚠ REVISION 2026-09-11 — Task 2 stage audit
>
> Task 2 was reviewed and the framing pushed back on: **the problem is not file size, it is that
> individual FUNCTIONS mix stages** (input-parsing / normalization / business logic / aggregation) in
> one body. All 60 top-level items in `resolve.rs` were classified against that four-stage flow. The
> audit lives in **"Task 2 audit (2026-09-11)"** immediately before Task 2. Consequences, all applied:
>
> 1. **18 of 60 items mix 2+ stages**; the six worst are 828 lines, 30.7% of the file.
>    `resolve_qualified` is the only four-stage function, and its mixing is the *cause* of findings 9,
>    10 and 11 — those are three symptoms of one missing stage boundary, not three bugs.
> 2. **A real bug was found that three prior review rounds missed:** `resolve_qualified`'s
>    JAR-extension probe (**1582–1604**) is a third, weaker copy of `resolve_extension_in_scope` with
>    **no in-scope check** and a wrong-range declaration match. → **new Task 2a**, before the split.
> 3. **Task 2's file boundaries are KEPT**, with one correction (**`scope_check.rs`**, +1 module).
>    Stage-named files were considered and rejected with reasoning, not forced — see Task 2's banner.
> 4. **New Task 2b** after the split: decompose `resolve_qualified` into named stage functions. It
>    supersedes "Task 3b" and the two matching "Deliberately out of scope" entries.
> 5. Tasks 0, 1, 3, 4 and the parked 5–8 are **unchanged** in substance; Task 3 gains one note.

> ## ⚠ REVISION 2026-09-11 (round 4) — corrections from independent critique
>
> The 2026-09-11 audit above was itself reviewed. Two of Task 2a's four proposed fixes were wrong and
> are corrected in place rather than left standing:
>
> 1. **Task 2a's original item 1 ("merge copy 3 into a call to `resolve_extension_in_scope`") would
>    have shipped a regression** — `tests.rs:3010` only passes because copy 3 (1582–1604) is
>    deliberately more permissive (fail-open, no in-scope check) than `resolve_extension_in_scope`.
>    The real bug is narrower: a wrong-range match (`s.name == name` instead of
>    `extension_declaration_matches`). Fixed as a 1-line swap that keeps the fail-open behaviour;
>    Task 2a's test list now includes an explicit regression guard for it.
> 2. **Task 2a's original item 3 ("`location_package` re-inlined verbatim, delete the inline copy")
>    is false** — the inline variant deliberately omits a branch the original has, with its own
>    justifying comment. Dropped from Task 2a, backlogged as a real (not mechanical) behaviour
>    question next to the two divergent dotted-name parsers.
> 3. **The audit's own arithmetic was wrong**: "1,012 lines / 37.5%" was back-derived from a
>    percentage rather than summed; the real total (from the audit's own corrected per-function line
>    counts) is 828 lines / 30.7%. Corrected everywhere it appears.
> 4. **Task 2b's `ReceiverAnchor` could not construct `own_members`** — it carried `declaring_uri:
>    Option<Url>` instead of a full `Location`, and the container-scoped lookups it feeds both need
>    the declaration's range, not just its file. Fixed: `ReceiverAnchor.declaration: Option<Location>`.
> 5. **Task 2b's unification of the two branches silently picks an order the plan hadn't decided.**
>    The uppercase and lowercase branches don't just differ in which tiers exist (findings 9/10/11) —
>    they check tiers in a DIFFERENT ORDER (uppercase: extension before members; lowercase: members
>    before extension). `QualifiedCandidates` can only encode one order. Now stated as an explicit,
>    justified decision (members win, matching real Kotlin precedence and the lowercase branch's
>    existing order) with its own named regression test, rather than left as an implicit consequence
>    of whichever field order got typed first.
>
> Full critique text is not preserved in this file (it lived in an agent dispatch, not a doc); the
> corrections above are self-contained. Everything else from the 2026-09-11 audit — the six mixing
> violations, the `scope_check.rs` boundary correction, the moderate-violations list, `AGENTS.md`/
> unification-doc grounding — was independently re-verified and stands unchanged.

**Goal (revised):** Close the largest *measured* member-resolution gaps on the real Moneta corpus,
make `resolve.rs` navigable while doing it, and keep the (real, three-times-reviewed) JAR-promotion
budget architecture on the shelf until a measurement justifies it.

**Original goal, still the goal of Tasks 5–8:** Replace the ~20 independent, uncoordinated per-call JAR-promotion budget mints scattered across `resolve.rs`/`infer.rs`/`sig.rs`/`indexer.rs`/`complete.rs` with ONE budget threaded per real request, minted exactly once at a true external entry point and classified by the caller's actual latency tolerance.

**Architecture (Tasks 5–8):** A `LatencyClass` enum (`Interactive` / `Keystroke` / `Exhaustive` / `Background`) whose only job is to mint a non-`Copy`, no-public-constructor `JarPromotionBudget`. Every promotion-capable resolver function takes `&mut JarPromotionBudget` instead of minting its own; only the thin `impl Indexer` wrappers at the crate boundary and the named `features/`/`indexer/` entry points mint. "Do I already have a budget?" becomes *readable* off the signature — `&mut JarPromotionBudget` = you were given one, owned `JarPromotionBudget` = you minted one — but it is **not enforced by visibility**: the mint has to be `pub(crate)` because real entry points live in `indexer/` and `features/`, outside `crate::resolver`. Enforcement is a source-scanning allowlist test (Task 5), and the plan says so plainly rather than crediting the type system with work it cannot do.

**Tech Stack:** Rust, existing `Indexer`/`walk_hierarchy`/`jar::promote_candidates_bounded` infrastructure — no new dependencies.

---

## Verification of the prior findings (done 2026-09-09 against the current worktree)

Every numbered claim from the briefing was re-checked against `fix/extension-supertype-variable-receiver` at the current tip. Line numbers below are **as measured today**, not as briefed. Three claims were materially wrong; two were incomplete; one new blocking finding was discovered that neither prior review round mentions.

> **Line-number drift as of 2026-09-10 (tip `f0e5ed56`, `resolve.rs` now 2697 lines, was 2678).**
> Re-measured; do not re-derive.
>
> - **Findings 1, 9, 10, 11 — line numbers unchanged.** Everything inside `resolve_qualified`
>   (1444, 1473, 1483/1485, 1487, 1511/1517, 1535, 1537, 1553, 1566, 1677, 1680, 1692, 1709) still
>   reads exactly as written. The one exception: the JAR-extension probe finding 11 calls "1578"
>   is now **1581**.
> - **Everything after `resolve_qualified` drifted +19 to +25** (PR #315 added 25 lines to
>   `resolve_from_class_hierarchy_scoped`'s callback and doc comment): `with_supertype_extension_fallback`
>   **2296 → 2315**, `resolve_extension_via_supertype_hierarchy` **2342 → 2361**, `rg_in_package_dir`
>   **2379 → 2404**, `package_dir_in_source_roots` **2440 → 2465**,
>   `import_package_absent_from_source_roots` **2469 → 2494**, the `impl Indexer` block
>   **2637 → 2658**, `resolve_member_only` **2670 → 2689**.
>   `resolve_from_class_hierarchy` (2233) and `_scoped` (2251) did **not** move.
>   **Task 5/6/7/8's mint-site tables below still carry the pre-#315 numbers — re-measure before
>   using them.**
> - **Finding 7 is superseded** by Task 2's own re-verification: the count is 18 private items
>   needing widening, not 8, and `import_package_tie_break` is already `pub(super)` (725).
> - **Findings 10 and 11 are still red.** Verified 2026-09-10 by reading the code, not by memory:
>   `with_supertype_extension_fallback` (2315–2336) calls *only* `resolve_extension_via_supertype_hierarchy`
>   — there is still no own-type extension tier; and `walk_hierarchy_breadth_first` (`hierarchy.rs:85`)
>   still calls `collect` only on values yielded by `supertype_targets`, never on `start_class`.
>   PR #314 built the helper; it did **not** build finding 10's fix. See Task 3.

### 1. `resolve_qualified` re-entrancy — **CONFIRMED, with one sub-claim wrong**

`resolve_qualified` is `src/resolver/resolve.rs:1444`. It calls the module-private `resolve_symbol` internally at **1485** (uppercase root, `ResolveIo::Full` branch; the `IndexOnly` sibling at 1483 calls `resolve_symbol_index_only`), **1625**, **1656**, **1666** (all lowercase-root). Briefed as 1485/1624/1655/1665 — drift of 0–1 lines, claim holds.

**Wrong sub-claim:** "one of those is inside an unbounded loop (`implicit_receiver_member_match`, near 1403)". `implicit_receiver_member_match` (`resolve.rs:1396`) is **not called from `resolve_qualified` at all** — its only caller is `resolve_implicit_receiver_callee` (`resolve.rs:1306`, at line 1318). And the `resolve_symbol` at **1403** is the `for` loop's *iterator expression*: evaluated exactly once, then iterated. It is not a per-iteration call.

**The real unbounded-loop hazard is elsewhere and is worse:** `resolve.rs:1487` opens `for qual_loc in &qual_locs`, and the budget-minting calls at **1553** (`resolve_from_class_hierarchy_scoped`) and **1566** (`resolve_extension_via_supertype_hierarchy`) are *inside that loop body*. Each iteration mints a fresh budget-3. So a qualifier root with N candidate declarations costs up to `6 × N` blocking promotion attempts from a single `resolve_qualified` call, not 6. Use this shape, not `implicit_receiver_member_match`, as the re-entrancy motivating example.

### 2. External callers of `resolve_symbol` — **CONFIRMED, and one caller was missed**

Outside `resolve.rs`, non-test:

| Site | Traffic class |
|---|---|
| `src/indexer/lookup.rs:41`, `:82`, `:91` | mixed (index reads) |
| `src/indexer/resolution.rs:705` | mixed (`IndexRead`) |
| `src/indexer/apply.rs:1250` | **Background** |
| `src/indexer/infer/sig.rs:631` | Keystroke-adjacent (see item 3) |

`apply.rs` confirmed as a third traffic class: `tokio::sync::Semaphore::new(4)` at **apply.rs:1242**, `tokio::task::spawn_blocking` at **1249**, `indexer.resolve_symbol` at **1250** — a fire-and-forget completion-cache prewarm, no user waiting on it. It genuinely belongs in neither the interactive nor the keystroke class.

`sig.rs:631` was not in the briefed list of item 2 (it appears only under item 3). It is a real sixth external caller.

### 3. `sig.rs` name collision — **CONFIRMED**

`src/indexer/infer/sig.rs:990` defines a file-local `resolve_qualified` and `:1139` a file-local `resolve_unqualified`, both returning `Resolution<Signature>` — entirely distinct from `resolve.rs:1444`. Neither reaches the resolver-domain `resolve_symbol`. `features/call_arg_diagnostics.rs` → `resolve_call_signature` is therefore correctly ruled out.

`find_fun_signature_with_receiver` is at **sig.rs:618** and calls `idx.resolve_symbol` at **631**. Its real callers (excluding the two trait-method wrappers `features/traits_impl.rs:159` and `features/traits.rs:231`, which merely forward) are exactly four:

- `src/features/completion.rs:437`
- `src/features/signature_help.rs:33`
- `src/features/signature_help.rs:42`
- `src/features/completion_context.rs:153`

### 4. "Hardcoded zero everywhere" is FALSE — **CONFIRMED, and the true picture has THREE regimes, not two**

There is no single mint policy. Measured today, the codebase runs three:

**Regime A — zero budget (`let mut cache_backed_only = 0usize`), 18 sites:**
`resolve.rs` 1033, 1186, 1246, 1338, 1581, 1991, 2109, 2161 (8) · `infer.rs` 1425, 1683, 1911 (3) · `sig.rs` 240, 737, 1011, 1051, 1164 (5) · `indexer.rs` 471, 545, 1051 (3).

**Regime B — `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` (= 3, `hierarchy.rs:30`), 11 sites:**
`resolve.rs` 2271, 2366 · `infer.rs` 1331, 1404, 2058 · `complete.rs` 302, 587, 860, 1957, 2029 · `features/references.rs:52`.

**Regime C — genuinely unbounded (`usize::MAX`), 5 non-test sites — this regime is NOT mentioned in either prior review:**
`complete.rs:814` · `indexer/lookup.rs:151` · `indexer/resolution.rs:673` · `indexer/resolution.rs:756` · **`features/rename.rs:125`** (found in the third review round — see finding 12; it is not a `let mut unbudgeted` local but a literal `usize::MAX` *argument*, which is why a `grep 'unbudgeted'` sweep missed it).

A sixth unbounded literal exists at `indexer/jar.rs:1349` (`promote_candidates`'s `let mut unbounded = usize::MAX`) but is **not** on any resolver read path: its only caller is `ensure_jar_materialized` (`jar.rs:1285`), whose only non-test caller is `workspace/document_handler.rs:587` — per-import promotion at file-open time, off the per-request path this plan budgets. Verified by `grep -rn 'ensure_jar_materialized('`. Left alone; see "Deliberately out of scope".

The briefing's core correction holds and is if anything understated. The keystroke path really does spend blocking IPC today: `resolve_member_only` (**resolve.rs:2670**, briefed as 2671) → `resolve_qualified` → mint sites 2271 and 2366, reached from resolve.rs 1553/1566/1680/1692. Its live caller chain is `Resolver::resolve_member` (`api.rs:222`) → `features/nullable_call_diagnostics.rs:115`, which runs from the `did_change` diagnostics path (`workspace/document_handler.rs`, `workspace/file_change_handler.rs`). Because 1553/1566 sit inside the `for qual_loc` loop (item 1), the per-keystroke ceiling is not 9 but `6 × |qual_locs| + 3`.

Note that `resolve_from_class_hierarchy` (**2233**) is a one-line delegate to `resolve_from_class_hierarchy_scoped` (**2251**), so resolve.rs holds **two** literal mint expressions (2271, 2366) reached from **five** call sites: 1537→2304→2366, 1553, 1566, 1680→2304→2366, 1692. (An earlier draft of this finding said "four" while listing five — the count is five.)

### 5. `walk_hierarchy` re-mints downstream — **CONFIRMED, with the mechanism identified**

`walk_hierarchy` (`hierarchy.rs:37`) threads its `sidecar_budget` correctly into `ensure_jar_definitions_for` (`hierarchy.rs:208`, `&mut` — genuinely shared and decremented across the whole walk). But `supertype_targets` also calls `resolve_symbol_hierarchy_ambiguity_safe` at **hierarchy.rs:240** and **:275** *without* passing the budget. That function (`resolve.rs:875`) goes to `resolve_chain(… ResolveIo::HierarchyAmbiguitySafe …)`, which reaches the Regime-A zero-budget sites at resolve.rs 1033/1186/2109/2161. So today it does not *re-mint a nonzero* budget — it silently drops to **zero**, which is the mirror-image bug and equally a break in the threading. Claim confirmed, mechanism corrected.

### 6. Threading must reach `infer.rs` for the Moneta case — **CONFIRMED, explicitly**

`resolve_qualified:1609` calls `infer_variable_type` (`infer.rs:636`). For the target case, `val navController = rememberNavController()` requires return-type inference, which lands on `find_fun_return_type_reachable`'s zero-budget promote at **infer.rs:1683–1684**. A cold `navigation-compose` JAR therefore fails to yield `NavController` as the inferred type *before* `resolve_qualified` ever gets to a receiver. Extension-return-type inference has the same wall at **infer.rs:1911–1914**.

`indexer.rs` 471/545/1051 are `find_class_*`/`find_fun_*` index helpers on the same cold path.

**Answer: the threading must extend into `infer.rs` for the Moneta case to be closable at all. It is in scope (Phase 3).** `sig.rs` and `complete.rs` are *not* required for Moneta and are deferred to the same phase only for consistency; they can be dropped if the phase runs long.

### 7. Visibility-widening scope for a hypothetical file split — **CONFIRMED**

All seven named functions are module-private (`fn`, no `pub`): `ambiguity_safe_tail_with_denylist` **556**, `resolvable_via_default_import` **1025**, `import_container_chain` **1789**, `enclosing_container_chain` **1806**, `resolve_via_imports` **1940**, `resolve_same_package` **2071**, `resolve_star_imports` **2194**. Add to the list, also private and cross-cutting: `resolve_chain` **250**, `resolve_symbol_with_io` **149**, `resolve_qualified` **1444**, `resolve_local` **1719**, `resolve_from_class_hierarchy{,_scoped}` **2233**/**2251**, `resolve_extension_in_scope` **1237**. See "Deliberately out of scope" for why no split happens here.

### 8. Test churn — **CONFIRMED, numbers slightly higher than briefed**

`src/resolver/tests.rs` is 9219 lines with **66** `resolve_symbol(` call sites (briefed ~60). `src/indexer/jar_tests.rs` has 5; `src/indexer_tests.rs` has 1; `src/features/nullable_call_diagnostics_tests.rs` also exercises the chain. `tests.rs` already reaches across sibling-module boundaries at **four** paths, not one: `crate::resolver::hierarchy::MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` (7156, 7195, 7198), `crate::resolver::MAX_SYNC_…` (7186), `crate::resolver::complete::DATA_FQN`, and `crate::resolver::infer::{extension_is_in_scope, find_extension_fn_return_type, find_method_return_type, infer_variable_type_from_cst}`.

This churn is the single strongest argument for the Phase-2 shape below: **keep `Indexer::resolve_symbol`'s signature unchanged** and add a budget-carrying sibling, so 66+ test call sites never move.

### 9. Precedence-tiers gap — **CONFIRMED but UNDERCOUNTED (two pairs, not one)**

The lowercase pair the briefing names is real: `resolve_from_class_hierarchy` **1692** → `resolve_extension_in_scope` **1709**, statement-order only, no type carries the tier.

The uppercase branch has the *same* uncovered pair: `resolve_from_class_hierarchy_scoped` **1553** → `resolve_extension_via_supertype_hierarchy` **1566**. `with_supertype_extension_fallback` (**2296**) covers only the two `member_locs`-non-empty exits (1537, 1680); neither 1553→1566 nor 1692→1709 is type-carried.

### 10. NEW FINDING — budget alone cannot close the Moneta case on the lowercase-root path

Neither prior review round found this, and it changes which phase actually closes the gap.

`with_supertype_extension_fallback` (**2296**) delegates to `resolve_extension_via_supertype_hierarchy` (**2342**), which calls `walk_hierarchy_breadth_first` (**hierarchy.rs:85**). Reading that function's body: `current_level` starts as `[(start_class, start_uri)]`, and the loop body calls `collect(idx, &super_name, &super_uri, caller)` **only on values yielded by `supertype_targets`** (hierarchy.rs:112). `start_class` itself is never passed to `collect`.

Consequence: for `navController.navigate(route = "...")` (lowercase root), if `find_name_in_uri` at **1677** finds the wrong-arity JVM `NavController.navigate(...)` members, the fallback at **1680** looks up `resolve_extension_in_scope` for every **ancestor** of `NavController` and never for `NavController` itself. The own-type extension lookup exists only at **1709**, which is unreachable once 1677 returned non-empty. The androidx KTX `navigate(route: String)` extension is keyed on the receiver's own leaf type. **With an infinite promotion budget, this path still returns the wrong-arity members and nothing else.**

The uppercase branch does not have this hole — it tries `resolve_extension_in_scope(root_base, …)` first, at **1473**.

This is why Phase 1 below (a ~10-line tier fix) is what closes Moneta, and the whole budget refactor is architecture cleanup that makes the fix *reliable on a cold cache* rather than the fix itself.

### 11. NEW (third round) — the uppercase branch has the same hole on MULTI-SEGMENT qualifiers

Finding 10 says "the uppercase branch does not have this hole — it tries `resolve_extension_in_scope(root_base, …)` first, at **1473**". That is true **only for a single-segment qualifier**. Verified today:

- **1473** probes the own-type extension keyed on `root_base` — the *root* segment, computed before the nested walk.
- **1510–1524** then walks `segments[1..]`, reassigning `anchor_class_name = nested_segment` on each hop. For `Outer.Inner.member`, `anchor_class_name` ends up `"Inner"` while the 1473 probe was keyed on `"Outer"`.
- Nothing re-probes the own-type extension against the reassigned `anchor_class_name`.

So `Outer.Inner.member()` where `member` is an extension **on `Inner` itself** is missed, exactly as in finding 10.

**Does Task 3's Step 3 fix already close this?** Verified directly, and the answer is *partly* — this is why Step 3 below is stated more precisely than it was:

- **On the `member_locs`-non-empty exit (1537): YES, incidentally.** 1537 already passes the *reassigned* `anchor_class_name`, so once `with_supertype_extension_fallback` gains an own-type-extension tier keyed on its `anchor_class_name` parameter, `Outer.Inner.member` gets the `Inner` extension for free. This was unintentional in the original draft; it is now intentional and gets a test (Step 1, test 5).
- **On the `member_locs`-empty path (1553 → 1566): NO.** `resolve_from_class_hierarchy_scoped` (2251) is `walk_hierarchy(… , |index, _, class_uri, _| find_name_in_uri(index, name, class_uri))` — a pure **member** lookup with no extension probe of any kind (the comment at resolve.rs:1564 claiming that path already covered "or exact-key extension" is simply wrong, and Step 3 should correct it). `resolve_extension_via_supertype_hierarchy` (2342) is ancestors-only per finding 10. So if `Inner` declares no member `name`, inherits none, and the extension is keyed on `Inner`, both miss. Execution then falls out of the `for qual_loc` loop to the JAR-extension probe at **1578**, which re-keys on `root_base` again — a third miss.

**Therefore Task 3 must route the 1553/1566 pair through the shared tier helper too, not only 1537/1680.** That was already the intent of finding 9 ("the uppercase branch has the same uncovered pair"); finding 11 makes it load-bearing rather than a consistency nicety.

Left as a **noted open item, not fixed here:** the JAR-extension probe at **1578** keys on `root_base` for multi-segment qualifiers as well. Fixing it means threading `anchor_class_name` out of the loop, which changes the loop's control flow rather than adding a tier — a different shape of change from Task 3, and un-exercised by the Moneta case. Named in "Deliberately out of scope".

### 12. NEW (third round) — a fifth unbounded site, and it is `rename`

`src/features/rename.rs:125` passes a literal `usize::MAX` as the `sidecar_budget: usize` argument of `verified_references_for` (`src/features/references.rs:80`, parameter at **:87**), which forwards it to `verify_candidates` (`src/features/references_verify.rs:41`) and on into three `indexer.receiver_type_agreement(…, sidecar_budget)` calls at **references_verify.rs 126 / 157 / 180**. Verified today.

The sibling caller `src/features/references.rs:45` fills the *same* parameter with `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` (**references.rs:52**). So find-references is bounded at 3 and rename is unbounded, deliberately: rename passes `detect_reverse_overrides: true` and refuses the whole edit when `proven_overrides` is non-empty. An under-promoted supertype walk there does not degrade a read — it **fails to detect an override and lets a wrong workspace edit through**.

Consequence for this plan: an earlier draft listed rename in `Interactive`'s doc comment, and `Interactive` is minted at 3. Mapping rename to `Interactive` would be a silent behaviour regression *and* a violation of Global Constraint 1. Task 5 below therefore adds a fourth class, `Exhaustive`, and Task 8 owns the `features/` migration (which no task previously claimed).

---

## Global Constraints

**Apply to every task:**

- Every fix ships with a test, written and seen to fail first (AGENTS.md).
- `cargo test` and `cargo clippy -- -D warnings` clean after every task; `cargo fmt` before committing.
- Serena must be pointed at this worktree (`mcp__serena__activate_project` with
  `/home/ocel/Work/lsp/.worktrees/extension-supertype-variable-receiver`) before any symbolic edit;
  use `find_referencing_symbols` / `replace_content`, not grep and sed.
- Never commit to `main`; branch, push, open a PR.
- **Any task claiming a recall effect must state a measured before/after from
  `resolution-accuracy /home/ocel/Work/Moneta/android` in its PR description, and must name which
  Gap entries moved.** A percentage alone is not evidence on a 165k-reference denominator — this is
  the discipline that killed the original Task 1 premise and found PR #315, and it is the single
  most valuable habit in this plan.
- **Rebuild the release binary before measuring.** Task 0's first attempt measured a stale binary;
  so did this revision's, until caught. `cargo build --release`, check the mtime against the commit.

**Apply to the parked Tasks 5–8 only:**

- **No behaviour change in Phases 2–3.** Each class's numeric budget must reproduce today's value at every migrated site (`Keystroke` → 0 or 3 exactly as the site has today; `Background`/library-read sites → `usize::MAX`; **rename → `Exhaustive` → `usize::MAX`, per finding 12 — it is unbounded today and capping it silently is a correctness regression, not a latency tune**). The whole point of the split is that Phase 4 is the *only* commit where a number changes, so a benchmark regression has one suspect.
- **`Indexer::resolve_symbol`'s signature must not change.** 66 test call sites plus 6 production ones depend on it. Add a budget-carrying sibling; make the old name a thin `LatencyClass::Interactive` minting wrapper.
- **No `usize` budgets in new code.** Once `JarPromotionBudget` exists, a bare `&mut usize` promotion parameter is a bug in new code.

  A file-level "no literals left" claim is *not* checkable as originally worded: many files carry unrelated `0usize` counters, and the budget locals are named inconsistently across the codebase (`cache_backed_only`, `jar_promotion_budget`, `sidecar_budget`, `unbudgeted`, `unbounded`, plus bare `usize::MAX` arguments with no local at all — which is how finding 12's site hid). The checkable form, run per migrated file:

  ```bash
  grep -n 'ensure_jar_materialized_with_budget\|ensure_jar_materialized_for_extension_receiver\|promote_candidates_bounded\|extension_entries_for\|receiver_type_agreement\|walk_hierarchy' <file>
  ```

  **A file is migrated when every budget argument at those call sites is `budget.as_mut_usize()` (or a `&mut JarPromotionBudget` pass-through) — no local, no literal.** That is a finite, greppable set of consumers rather than an unbounded search for numbers that might be budgets.
- For budget-threading tests, the assertion shape already exists — `src/resolver/tests.rs:7186–7198` counts attempted promotions against the constant; copy that harness rather than inventing one.

---

### Task 0: Reproduce and instrument — decide which phase closes Moneta — **DONE**

**Files:** none modified (measurement only).

**Why first:** finding 10 says the tier hole, not the budget, is the Moneta blocker. That is a code-reading conclusion. Confirm it on the real corpus before spending Phases 2–4.

- [x] **Step 1: Capture the current failure**

Run the existing accuracy harness against the Android monorepo:

```
cargo run --release --bin kmp-lsp -- resolution-accuracy <moneta-root> 2>&1 | tee /tmp/moneta-baseline.txt
```

Record: overall member recall %, and whether `navigate` appears in the `Gap` or `FilteredCandidate` top-20.

- [x] **Step 2: Isolate the tier hole from the budget wall** — done; prediction did NOT hold, see below

Temporarily (do not commit) patch `resolve.rs:1680` so `with_supertype_extension_fallback` is preceded by a direct `resolve_extension_in_scope(indexer, &current_type_base, name, from_uri)` probe, and re-run Step 1.

Expected per finding 10: `navigate` resolves, on a **warm** cache, with no budget change at all.

- [ ] **Step 3: Isolate the budget wall** — **NEVER RUN.** Deliberately skipped per Step 4's stop instruction. This is now the resumption gate for Tasks 5–8.

Revert Step 2. Temporarily raise the zero at `resolve.rs:1246` (`resolve_extension_in_scope`) to `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` and re-run against a **cold** JAR cache (clear the sidecar cache dir first).

Expected: budget alone does NOT resolve `navigate`; the tier fix alone does NOT resolve it cold.

- [x] **Step 4: Write down the result**

Append a short "Task 0 measurement" section to this plan file with the three numbers. If Step 2 does *not* fix it, **stop and re-diagnose** — the rest of this plan is built on finding 10 and would be aimed at the wrong target.

---

#### Task 0 measurement (done 2026-09-09) — BLOCKED, Step 2's prediction did not hold

Run against the real corpus (`/home/ocel/Work/Moneta/android`, warm sidecar cache reused from prior sessions), binary built via `cargo build --release` (`target/release/kmp-lsp resolution-accuracy <root>`, not `cargo run`).

**Step 1 (baseline):** member recall **90.2%** (149613/165947 `CstResolved`). `navigate` is **#3 in the Gap top-20** (220 occurrences), e.g. `core/common_screen/.../RetailLendingAmlFormScreen.kt:29:23` — `navController.navigate(route = ERetailLendingAmlScreen.DENIED.name)`, receiver declared type `NavHostController` (a parameter, not an inferred local).

**Step 2 (tier-hole patch at `resolve.rs:1680`, warm cache):** patched exactly as specified — `with_supertype_extension_fallback` preceded by a direct `resolve_extension_in_scope(indexer, &current_type_base, name, from_uri)` probe. Rebuilt, re-ran. Result: member recall **90.3%** (149593/165732), `navigate` **still #-ranked in Gap, still 220 occurrences, identical example line**. No change.

**Root-cause re-diagnosis (temporary `eprintln!` tracing, reverted with the patch — not part of the numbers above):** the patched line 1680 is in `resolve_qualified`'s **lowercase-root branch** (`root` = a variable name needing type inference). It is never reached for this benchmark's `navigate` misses. `classify_cursor` (`indexer/infer/cst_symbol.rs`) already resolves `navController`'s declared type via CST before calling `resolve_symbol`, so the qualifier passed into `resolve_qualified` is the **already-resolved type name `"NavHostController"`** (uppercase) — the benchmark's member-reference path (`resolve_identity_with_io`'s `Reference { receiver_type: Some(_), .. }` arm) always goes through `resolve_qualified`'s **uppercase branch** (`resolve.rs:1468+`), not the lowercase-root branch finding 10 and the Step 2 patch target.

Traced the uppercase branch directly (220/220 occurrences, all identical):
- `1473` own-type extension probe on `"NavHostController"` → 0 locs (expected — the real navigation code is keyed elsewhere).
- `qual_locs` (resolving the `NavHostController` class itself) → 1 loc.
- `1535` `member_locs` (name `"navigate"` scoped directly to `NavHostController`'s own JAR-derived declaring file, `androidx.navigation:navigation-runtime-android:2.9.8`) → **1 loc** (non-empty) — so this hits the **`1537` branch** (`member_locs` non-empty → `with_supertype_extension_fallback`), *not* the `1553`/`1566` uncovered pair findings 9/11 describe.
- Inside `with_supertype_extension_fallback`, `resolve_extension_via_supertype_hierarchy` (the ancestor-walking extension probe that finding 10 says already exists and should find an ancestor-keyed extension) → **0 supertype_ext_locs**, for all 220 occurrences.

So `with_supertype_extension_fallback` *is* reached (contrary to being architecturally unreachable), and its ancestor walk runs, but finds no `navigate` extension candidate on any ancestor either. The single `member_locs` entry (wrong arity for a `route = "..."`-shaped named-arg call) then survives to `shape_filter_locations` downstream, gets arity-rejected, and the reference is demoted to `NameScan` → falls through to a bare-name scan that also fails → counted as `Gap`.

This does not match finding 10's mechanism (own-type-only miss on an ancestors-covered path). The evidence instead points at a **different bug**: androidx.navigation 2.9.8's route-based `navigate(route: String, ...)` is no longer a KTX extension function in this AAR version — it appears to be a real member of `NavController` with default parameters (`@JvmOverloads`), and the JAR-derived member index only surfaced **one** overload (arity mismatch for the observed call shapes), not all of `navigate`'s `@JvmOverloads`-generated overloads. That is an **indexing/arity gap** on JAR-derived Java members, not a resolution-tier gap — closer in kind to the bug PR #311 (`fix(resolve): give JAR-derived Java methods real arity and every overload`) already fixed once, possibly regressed or incomplete for this specific class/overload set.

**Per the critical instruction, stopping here — did not run Step 3.** Phases 2–4 of this plan (the budget-carrying refactor) are built on finding 10's mechanism; that mechanism is not what's blocking `navigate` in the real corpus, so building the rest of the plan on it would aim at the wrong target. Re-diagnosis needed before proceeding: confirm what `navigate`'s actual indexed member set looks like for `androidx.navigation:navigation-runtime-android:2.9.8` (is it a member with default args, or an extension in a sibling KTX artifact the indexer isn't reaching?), and whether this is a `find_all_names_scoped_to_container`/JAR-overload-arity gap rather than a precedence-tier or budget gap.

All temporary tracing/patches (Step 2's patch and the diagnostic `eprintln!`s) were fully reverted; `git diff` on tracked files is clean.

---

#### Task 0 continuation (2026-09-10) — **DONE**. The re-diagnosis landed, and it was neither a tier gap nor a budget gap.

The "stop and re-diagnose" instruction above was followed. `javap` on the real AAR plus temporary
`eprintln!` tracing (both reverted) found the actual cause, which the BLOCKED section above guessed
at but got wrong in one important way: `androidx.navigation.NavController.navigate` **is** a real
overloaded JVM member (the BLOCKED section's guess), but the reason only one wrong-arity overload
surfaced was **not** an `@JvmOverloads` indexing gap. It was two compounding *lookup* bugs, both
fixed in PR #315 (`f0e5ed56`):

1. **`find_name_in_uri_after_line`** (`find.rs`) had a final, position-only fallback — "closest
   same-named symbol at or after this line", with **no container check at all**. A compiled JAR packs
   several classes into one synthetic per-JAR `FileData` (exactly `NavHostController` /
   `NavController`, both from `navigation-runtime-android:2.9.8`), so `NavController`'s own
   `navigate` overloads got attributed to `NavHostController` purely by file position. That
   corrupted `resolve_qualified` into treating an *inherited* member as an *own, wrong-arity* one —
   which is precisely why the trace above saw `member_locs` → 1 loc and took the 1537 exit rather
   than the 1553/1566 pair findings 9/11 predicted.
2. **`resolve_from_class_hierarchy_scoped`**'s ancestor walk used the single-result, arity-blind
   `find_name_in_uri` per ancestor instead of the container- and overload-aware lookup, so even
   reaching the correct ancestor returned one arbitrary overload rather than the arity-complete set
   the downstream shape filter needs.

Fix: `find_name_in_uri_after_line` gained `container_name: Option<&str>` and never trusts position
over a real container tag when the container is known; a new `find_all_names_with_container_in_uri`
helper lets the hierarchy walk enumerate every overload scoped to an ancestor's *name* without first
resolving that ancestor's declaration `Location`.

**Neither finding 10's tier hole nor any promotion budget was involved.** The whole
`LatencyClass` effort is, for this gap, a decoy — see Tasks 5–8's parking notice.

##### Fresh corpus measurement (2026-09-10, warm cache, `target/release/kmp-lsp resolution-accuracy /home/ocel/Work/Moneta/android`, binary rebuilt at tip `f0e5ed56`)

| | 2026-09-09 baseline (Task 0 Step 1) | 2026-09-10 (both fixes in) |
|---|---|---|
| member recall | 90.2% (149613 / 165947) | **90.3% (149718 / 165795)** |
| member Gap total | — | **8862** |
| `navigate` in Gap top-20 | **#3, 220 occurrences** | **absent — 0** |

Recall moved +0.1pp, which is within this corpus's run-to-run noise; the meaningful, unambiguous
signal is that `navigate` left the top-20 entirely. (Per the PR-#301 methodology memo: a name
leaving the Gap list is a stronger signal than the aggregate percentage, which is diluted by the
165k denominator.)

##### Bug-class taxonomy of the CURRENT Gap top-20 — the input to every decision below

Each site below was opened in the real Moneta source at the exact `line:col` the harness reported,
the receiver expression read, and the receiver's declaration traced. This is the evidence base for
"is there more low-hanging fruit of PR #315's kind?" — **the answer is no, but there is one
different, cheaper class**, which becomes Task 1.

| # | Gap name | Receiver at the flagged site | Bug class |
|---|---|---|---|
| 311 | `name` | `Insurance.EType.entries.firstOrNull { it.name … }` | **C** lambda-`it` element type (+ enum `entries`, + `Enum` implicit supertype) |
| 224 | `finish` | `activity?.finish()`, `val activity = LocalContext.current.findActivity()` | **B** return-type inference (Compose `CompositionLocal.current` → local extension returning `ComponentActivity?`) |
| 195 | `title` | `textModel.required("textModel").title` | **B** generic return-type inference (`fun <T> T?.required(field): T`) |
| 164 | `text` | `…firstOrNull { … } }?.text` | **B** generic return-type inference |
| 134 | `scenes` | `screenData.text.scenes` | **B** chained property type |
| 123 | `fragmentArguments` | `CaliforniaActivity.Builder(…).fragmentArguments(…)` | **B** builder-chain return type on a nested class |
| 120 | `start` | `if (drawable is Animatable) drawable.start()` | **D** smart cast |
| 118 | `firstOrNull` | `countries.firstOrNull { … }` | **B** receiver element type |
| 111 | `run` | `scope.run { … }` | **E** extension on an unconstrained type parameter (`fun <T> T.run`) — never matches an exact-string receiver key |
| 104 | `launch` | `pinContract.launch(intent)`, `pinContract = rememberLauncherForActivityResult(…)` | **B** Compose generic return type (`ManagedActivityResultLauncher`). `ActivityResultLauncher.launch` **is** indexed (`activity-1.13.0.aar:245/253`) — the member is findable, the receiver type is not |
| **103** | **`fail`** | `liveApiProperty.fail`, param typed `LiveApiProperty<CommonData>` | **A — Java getter → Kotlin synthetic property.** `LiveApiProperty.java` declares `private final Throwable mFail` and `public Throwable getFail()` (`:72`). There is no member named `fail` anywhere. |
| 93 | `toString` | `findVersion(alias).get().toString()` | **B** generic return type (`Optional<T>.get()`) |
| 81 | `finishAffinity` | `is DisagreeEffect.CloseActivity -> if (effect.finishAffinity)` | **D** smart cast in a `when` branch |
| 81 | `forEach` | `fun Card.atLeastOneRightAllowed(vararg rights: CardRight)` → `rights.forEach` | **F** `vararg` parameter's receiver type is `Array<out T>`, not `T` |
| 81 | `toInt` | `termMax?.toInt()` | **B** nullable receiver type inference |
| 77 | `isNotEmpty` | `takeIf { it.isNotEmpty() }` | **C** lambda-`it` type |
| 76 | `await` | `texts.await()`, `texts` from `scope.async { … }` | **B** generic return type |
| 76 | `filter` | `split.getOrNull(1)?.filter { … }` | **B** generic return type |
| **74** | **`currentIdentity`** | `identityManager.currentIdentity`, `identityManager: IdentityManager` | **A — Java getter.** `IdentityManager.java:98` declares `public Identity getCurrentIdentity()`. No `currentIdentity` member exists. |
| 69 | `add` | `.add(Date::class.java, …)` (Moshi builder chain) | **B** builder-chain return type |

**Verdict — the question this revision was commissioned to answer:**

- **Zero of the top-20 are PR #315's bug class** (container-scoping / arity-blind ancestor lookup).
  The two that *looked* like it on first read — `finish` and `launch`, both "member declared on a
  JAR/SDK ancestor" — were checked directly and are not: `android.app.Activity.finish` is indexed
  (`Activity.java:7524`, confirmed via `kmp-lsp find finish`, which returns 199 hits including 66
  from the Android SDK sources) and `ActivityResultLauncher.launch` is indexed. In both cases the
  *member* is reachable and the *receiver type* is what never gets computed. PR #315 appears to have
  genuinely exhausted this class on this corpus.
- **Class B dominates (~10 of 20, ≈1000 occurrences): receiver-type inference through generic and
  chained calls.** This is `infer.rs`/`chain.rs` territory, not `resolve.rs`, and it is a research
  effort, not a cheap fix. It is the single largest remaining target but it is **not** low-hanging.
- **Class A is the low-hanging fruit: 177 confirmed top-20 occurrences (`fail` 103 +
  `currentIdentity` 74), a well-defined mapping, and no implementation anywhere on the resolve
  path.** `grep -rn '"get"' src/` finds exactly one `get`-prefix stripper in the whole crate —
  `features/code_actions.rs:568`, a code-action helper that never runs during resolution.
  **This becomes Task 1.**
- Classes D (smart cast, ~200), E (`fun <T> T.run`, 111) and F (`vararg`, ≤81) are each real,
  each cheap-ish, and each smaller than Class A. They are recorded in the backlog at the end of this
  plan rather than promoted to tasks — Class A first, then re-measure before picking the next.

---

### Task 1: Resolve Java getters as Kotlin synthetic properties

**Why this is first:** it is the only remaining top-20 finding backed by direct measurement
(177 confirmed occurrences), the fix is a name-mapping retry at a member-lookup site this plan
already touches, and it is independently shippable in one small PR.

**Files:**
- Modify: `src/resolver/resolve.rs` (`resolve_qualified`'s uppercase-branch member tier — the
  `find_all_names_scoped_to_container` call at **1535** and the `resolve_from_class_hierarchy_scoped`
  call at **1553**)
- Modify: `src/resolver/tests.rs` (new tests)

**Interfaces:** internal only. No signature outside `resolve.rs` changes.

**The mapping, deliberately minimal.** Kotlin's Java-interop synthetic-property rule is broader than
what this task implements. Implement exactly one direction and one prefix:

- reference `foo` → also try member `getFoo` (first char of `foo` upper-cased), **only when the
  declaring file is Java** and **only after every existing tier has returned empty**.

Explicitly **not** in scope, and each needs its own measurement before being added:
`setFoo` (write access — the benchmark measures reads), `isFoo` (Kotlin maps `isFoo()` to the
property `isFoo`, i.e. the *identity* mapping, so it already works if the member is indexed),
records/`@JvmRecord` accessors, and Kotlin-declared `get`-prefixed functions (a Kotlin `fun getFoo()`
is **not** accessible as `.foo` from Kotlin — applying the mapping there would invent a resolution
Kotlin itself rejects, which is why the Java-file guard is load-bearing, not a nicety).

**On where the fix goes — the alternative that was considered and rejected.** The broader fix is
index-side: when indexing a Java file, additionally register `getFoo` under the name `foo`. That
would fix hover, completion, find-references and diagnostics in one change instead of just
`resolve_qualified`. It is rejected here for three reasons, all of which should be re-examined if
Task 1 measures well: (a) it doubles symbol-table entries for every Java getter in the Android SDK
sources and every AAR, on a corpus where memory has already been a tracked problem; (b) it forces a
cache-version bump, and a stale cache masking a correct fix has burned this project before (see the
PR #298–300 memo); (c) it makes the synthetic name indistinguishable from a real one at every
consumer, where the lookup-site retry keeps the fallback explicit and ordered *after* real members,
so a genuine `foo` always wins. Start narrow, measure, then widen if the numbers justify the churn.

- [ ] **Step 1: Write the failing tests**

Add to `src/resolver/tests.rs`, next to the existing JAR/Java-member fixtures PR #315 added
(the `NavHostController`/`NavController` same-JAR-file test is the closest structural template):

1. `resolve_qualified_java_getter_resolves_as_kotlin_property` — a Java file declaring
   `public class Holder { private Throwable mFail; public Throwable getFail() { … } }` and a Kotlin
   caller `holder.fail`. Assert the location of `getFail` comes back. This is the exact
   `LiveApiProperty.fail` shape and is red today.
2. `resolve_qualified_real_member_wins_over_a_getter_of_the_same_name` — the Java class declares
   **both** a real field `fail` and a method `getFail`. Assert the field is returned, first. The
   getter tier must never outrank a real member.
3. `resolve_qualified_kotlin_get_prefixed_function_is_not_a_synthetic_property` — a **Kotlin** class
   declaring `fun getFail(): Throwable`, referenced as `.fail`. Assert **nothing** resolves. Kotlin
   does not expose Kotlin-declared `getX()` as `.x`; this is the guard that stops the task from
   inventing resolutions.
4. `resolve_qualified_inherited_java_getter_resolves_as_kotlin_property` — the getter is declared on
   a **supertype** of the receiver, so the retry has to route through
   `resolve_from_class_hierarchy_scoped`, not just the direct-container lookup. This is the
   `IdentityManager` shape and is the test that forces Step 3 to retry **both** 1535 and 1553 rather
   than only the first.

- [ ] **Step 2: Verify they fail**

`cargo test --bin kmp-lsp resolve_qualified_java_getter` → FAIL (empty result).

- [ ] **Step 3: Implement**

In `resolve_qualified`'s uppercase branch, after the existing member tier (1535) and the inherited
member tier (1553) have both returned empty, and **before** the supertype-extension tier at 1566,
retry the *same two lookups* with the getter name.

Reach for a type, not a boolean flag (AGENTS.md): the retry is the same two calls with a different
name, so extract the pair into one named helper — `member_or_inherited_member(indexer, name, &anchor,
anchor_class_name, from_uri) -> Vec<Location>` — call it once with `name`, then once with the getter
name. That deletes the duplication the retry would otherwise create *and* makes the ordering
("real member first, synthetic second") readable as two sequential calls rather than a flag.

The Java-file guard: derive it from the anchor's declaring URI (`anchor.uri`) — a `.java` path, or a
JAR-derived synthetic file whose symbols came from a `.class`. Do **not** guess from the symbol
name. If the existing `FileData`/`SymbolEntry` types do not already carry a language tag, read one
off the URI extension rather than adding a field for this task alone.

Do **not** touch budgets in this task. Do **not** change the index.

- [ ] **Step 4: Verify**

`cargo test --bin kmp-lsp` (all pre-existing green + 4 new) · `cargo clippy -- -D warnings` ·
`cargo fmt --check`.

- [ ] **Step 5: Measure on the real corpus**

Rebuild release, re-run `resolution-accuracy /home/ocel/Work/Moneta/android`, and record in the PR
description: member recall before/after, and whether `fail` and `currentIdentity` left the Gap
top-20. **The success criterion is those two names leaving the list, not the aggregate percentage.**

If they do *not* leave the list, stop and re-diagnose before widening the mapping — the same
discipline Task 0 applied, for the same reason.

- [ ] **Step 6: Commit and open a PR**

```bash
git commit -m "fix(resolve): resolve a Java getter as the Kotlin synthetic property it exposes

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01L7ZonwYUh94VsuQphiHykF"
```

---

### Task 2 audit (2026-09-11): unidirectional data flow — where the stages are mixed

> **Why this section exists.** Task 2 below was scoped as a *file*-size fix. Review of it raised a
> sharper question: the problem is not that `resolve.rs` is 2698 lines, it is that individual
> FUNCTIONS mix concerns — one body doing input-handling, data normalization, business-logic
> filtering AND result aggregation, with no boundary between stages. This section is that audit. It
> does **not** replace Task 2; it adds a precondition (Task 2a) and a successor (Task 2b), and makes
> one boundary correction inside Task 2 itself.

Every one of `resolve.rs`'s **60 top-level items** (Task 2's own count, re-used here rather than
recounted independently) was read and classified against a four-stage unidirectional flow (raw
input → clean data → domain rules → final answer):

- **P — input/parsing**: a raw string / qualifier / URI → a structured, typed value. No `Indexer`, no IO.
- **N — normalization**: a name or type reference → the canonical thing the rest of the pipeline can
  trust (a `Location`, a declaring file, a package, a nullability-stripped type name).
- **B — business logic**: what Kotlin actually does — member-over-extension precedence, arity/shape
  filtering, visibility and in-scope rules, tie-break ordering, hierarchy-walk order.
- **A — aggregation**: combining / ranking / deduplicating candidates into the one returned answer.

**Result: 18 of 60 items occupy two or more stages in one body.** The six worst account for
**828 of 2,697 lines (30.7%)** of the file. (Round-4 review caught this arithmetic wrong on first
pass — 1,012/37.5% was back-derived from a percentage rather than summed from the six line counts
below, two of which were themselves off by one. Corrected here; the per-function counts in the table
below are the real numbers, and the total is their sum.)

#### AGENTS.md already predicted exactly this finding

This is not an imported methodology. Two AGENTS.md "Code Quality" rules, quoted verbatim:

> **Section comments inside a function body signal a split.** Each phase becomes a named helper.

> **Reach for a type before reaching for a comment.** When you feel the need to explain control flow
> — why a fallback exists, what a `None`/empty result means, **which order branches run** […] — first
> ask whether an enum, newtype, or struct would encode that meaning so the compiler carries it and
> the signature tells the story.

Measured against the current file (`grep`-verified 2026-09-11, not recalled):

| Function | Numbered section comments in ONE body |
|---|---|
| `resolve_chain` (250–494) | **11** — `// 0.5 ──`, `// 1 ──`, `// 1.5 ──`, `// 2 ──`, `// 2.5 ──`, `// 3 ──`, `// 4 ──`, `// 4.5 ──`, `// 5 ──`, `// 5.4 ──`, `// 5.5 ──` |
| `receiver_provides_member` (1161–1226) | **4** — `// 1.` `// 2.` `// 3.` `// 4.` |
| `resolve_via_imports` (1940–2065) | **3** — `// i)` `// ii)` `// iii)` |

Eighteen named phases that the project's own rule says should be eighteen named helpers. The second
rule is the same finding from the other side: **findings 9, 10 and 11 in this plan are all instances
of "which order branches run" encoded as statement order inside one long body** — and all three are
real bugs, not style complaints. The audit invents nothing; it counts.

#### Do the two unification docs already gesture at a stage separation?

Partly — and the honest answer differs per doc:

- **`2026-06-30-cst-resolution-unification-design.md` explicitly excludes this file, and says so
  while leaving the door open.** Its "Context (why)" splits the world into a string path
  (`resolver/resolve.rs` … "Intentionally heuristic; 'good enough' is the bar. **Out of scope**
  here") and a CST path ("**This is what we unify**"), closing with: *"Whether anything is shareable
  among its ad-hoc heuristics is a separate later investigation."* **This audit is that
  investigation** — it is the doc's own named follow-up, not a competing effort. What the doc does
  supply is the pattern language, and that language *is* a stage pipeline under other names: its
  governing rule "Signatures maximize agent information", its three sub-rules (**Outcome enums over
  `Option`**, **Named newtypes over primitives**, **A struct over loose params**), and its capability
  table's *"Invariant — applied internally in every method; an unfiltered result never escapes the
  facade"*. `CstQuery` is a normalization stage with a typed boundary: `expr_type() ->
  Resolution<ResolvedType>` seals P+N off from B by construction. So the doc predicts the **shape**
  of the fix while disclaiming the **file**. Extend it; do not invent a fresh vocabulary.
- **`2026-08-24-qualified-resolution-unification-design.md` is the direct methodological precedent.**
  Its entire method is this audit's: find one question answered four different ways, extract a named
  **Primitive**, migrate the four sites onto it. Its reuse inventory names `range_encloses`,
  `enclosing_container_chain` and `resolve_companion_member` as *"the existing precedent for 'look up
  a container's full range, scope a name search to it'"* and generalizes that one case into a shared
  fn — a normalization-stage extraction under a different word. PR #315 then ran the same play a
  second time (`find_all_names_with_container_in_uri`). **The codebase has already executed this
  pattern twice and measured a recall gain both times.** What is new here is applying it *inside*
  `resolve.rs`'s own function bodies rather than to a primitive several functions share.

Neither doc predicts the *file split* — Task 2's boundaries stand on their own re-verification.

#### The six major mixing violations

Line ranges measured 2026-09-11 at tip `f0e5ed56`; `resolve.rs` = 2697 lines.

**1. `resolve_qualified` (1444–1710, 267 lines) — P + N + B + A, all four, twice.**
See the dedicated breakdown in Task 2b below. Headline: the uppercase branch (1468–1606) and the
lowercase branch (1608–1710) independently re-implement the *same* three-stage sequence — parse the
qualifier, normalize it to a receiver anchor, run a precedence ladder over that anchor — and the
two implementations have drifted apart. **Findings 9, 10 and 11 are not three bugs; they are three
symptoms of one missing stage boundary.**

**2. `resolve_chain` (250–494, 245 lines) — N + B + A + a write.**
- 263–265 **N**: decodes the `ResolveIo` enum into three loose bools (`full_io`, `allow_fd`,
  `star_rg`). The enum's meaning is re-derived at the use site — AGENTS.md's "reach for a type" in
  miniature, inverted.
- 270–276 **not a stage at all**: `indexer.index_content(...)` — a *write* to the index, inside a
  read path, phase "0.5".
- 279–365 **B**: the ordered strategy ladder, nine early-return exits.
- 305–328 **A inlined into B**: the Swift branch does its own candidate collection, `.swift`-file
  preference narrowing and fallback — a complete aggregation stage written inline in the middle of
  the dispatcher instead of behind a name.
- 343–357 **P inlined into B**: re-derives the star-import package list (copy 1 of 3, see below).
- 368–432 **B + IO + A**: an absence gate, an `rg` subprocess, a shape filter, an ambiguity tail,
  and a last-resort normalization, in one branch.
- 456–493 **A**: four tail arms, three of which are the same `lookup → tail rule → fall through to
  platform equivalent` shape written out three times.

**3. `resolve_via_imports` (1940–2065, 126 lines) — P + N + B + A + IO.**
- 1941–1944 **P** (non-star import list) · 1946 **B** (`local_name == name`)
- 1950–1963 **N** (qualified index → `Location`)
- 1969–1976 **P** — three separate parses of `imp.full_path` (`last_segment`,
  `import_package_prefix`, `import_container_chain`)
- 1978–1995 **N + IO** (definitions, JAR promote, `jar_definitions`)
- 1997–2021 **B** (package filter, with the JAR-vs-source fail-open rule)
- 2023–2040 **A** (container-chain narrowing: "only narrows when at least one candidate matches")
- 2047–2062 **IO** (`fd` subprocess) behind a business gate

**4. `resolve_in_scope_strict` (1081–1149, 69 lines) — a second resolution ladder, in `bool`.**
Ten sequential "if this strategy says yes, return true" checks. It is `resolve_chain`'s ladder
rewritten for a different return type, sharing no code with it: the two encode the same domain
knowledge ("what is reachable from this file") independently, and drift independently. Embeds its
own business rule at 1125–1133 (a stdlib star import means assume in scope) and its own copy of the
star-package parse at 1134–1142 (copy 2 of 3).

**5. `receiver_provides_member` (1161–1226, 66 lines) — B + N + A across four hand-numbered strategies.**
Step 3 (1190–1203) hand-rolls "is this symbol a member of this container" against
`fd.symbols.get(line).container` — the exact question `find_all_names_with_container_in_uri`
(`find.rs`, added by PR #315) already answers. Step 4 (1209–1224) runs a hierarchy walk at depth
**24** where every other walk in the file uses **12**, justified by a comment rather than a type.

**6. `resolve_extension_in_scope` (1237–1291, 55 lines) — N + B + N, with the tail N duplicated
three times across the file.**
- 1246–1251 **N + IO**: promote, then fetch `ExtensionEntry`s.
- 1254–1266 **B**: name match + `extension_is_in_scope` visibility rule.
- 1267–1287 **N**: re-materializes an `ExtensionEntry` into a `Location` by re-finding the symbol's
  `selection_range` in the declaring file. This is a normalization step buried inside a business
  filter loop — **and it exists three times**:

| Copy | Location | Scope gate | Declaration match |
|---|---|---|---|
| 1 | `resolve_extension_in_scope` 1267–1287 | `extension_is_in_scope` ✅ | `extension_declaration_matches` ✅ |
| 2 | `implicit_receiver_extension_match` 1361–1377 | `extension_is_in_scope` ✅ | `extension_declaration_matches` ✅ |
| 3 | **`resolve_qualified` 1582–1604** | **none** ❌ | **`s.name == name` only** ❌ |

> **New bug found by this audit, not by the three prior review rounds.** Copy 3 — the JAR-extension
> probe at the end of `resolve_qualified`'s uppercase branch — is a third, *weaker* re-implementation
> of `resolve_extension_in_scope`. Verified by `grep`: `extension_declaration_matches` has exactly
> two call sites in `resolve.rs` (1276, 1369) and **1596 is not one of them**. Two consequences,
> both real: (a) it can return an extension that is **not in scope** from `from_uri` (no
> package/import/visibility check at all, unlike copies 1 and 2); (b) its `range` is whatever
> symbol in the declaring file happens to be named `name` first — an unrelated member, or an
> extension on a *different* receiver — so goto-definition lands on the wrong line. Not
> speculative: this is what a stage boundary would have made impossible, since copies 1–3 would be
> one call to one named function. **Fixed in Task 2a, with a test.**

#### The twelve moderate violations

Each occupies 2–3 stages but is small and honestly named; they are recorded so the split does not
enshrine them, not because any is urgent on its own.

`resolve_symbol_with_io` (149–214, P+N+B — the dotted-name parser at 186–211) ·
`resolve_type_index_only` (919–940, P+N — **a second, divergent dotted-name parser**: splits only at
the first dot and does *not* skip lowercase package segments, unlike 186–211) ·
`resolvable_via_default_import` (1025–1071, B+IO+A, two near-identical scan loops) ·
`implicit_receiver_extension_match` (1331–1388, N+B+N+B+A — copy 2 above plus an arity tier) ·
`implicit_receiver_member_match` (1396–1428, N+B+A) ·
`import_package_tie_break` (725–758, P at 733–743 + A at 747–757) ·
`resolve_companion_member` (1847–1924, **two entirely different algorithms in one body**, switched
by `if indexer.jar_files.contains_key` at 1860 — the JAR-vs-source `FileData` shape difference, a
normalization concern, leaking into a business function; the same shape difference `find.rs` already
absorbs once) ·
`resolve_same_package` (2071–2129, N+A+IO+B; two separate aggregations of one question) ·
`find_symbol_in_package` (2138–2184, same two-loop shape; **2171–2176 re-inlines a narrower variant
of `location_package`** (831) — round-4 review found it deliberately omits `location_package`'s
`indexer.files` branch, with its own justifying comment at 2166–2170, so it is not a same-file
duplication safe to delete; see Task 2a's dropped item 3 for the full reasoning) ·
`resolve_star_imports` (2194–2220, P (copy 3 of 3) + A + IO) ·
`resolve_from_class_hierarchy_scoped` (2251–2306, N+B+A — dedup tacked onto a walk) ·
`rg_location_satisfies_call_shape` (1752–1769, N+B).

#### What is already clean — the target shape exists in this file

The audit is not a verdict on the whole file. Nineteen items are single-stage and are the model:

- **P**: `import_container_chain` (1789), `candidate_gradle_meta` (794), `pos_tuple` (1830).
- **N**: `ensure_file_data` (51), `location_package` (831), `jar_symbol_package` (1775),
  `owning_module_dependencies` (770), `fqns_for_name` (73), `enclosing_container_chain` (1806).
- **B**: `is_denylisted_package_prefix` (816), `is_stdlib` (2519), `is_default_import_type` (992),
  `has_explicit_import` (960), `import_package_absent_from_source_roots` (2494),
  `resolve_implicit_receiver_callee` (1306 — a two-line precedence statement, nothing else),
  `resolve_local` (1719).
- **A**: `ambiguity_safe_tail_with_denylist` (556), `module_scoped_tie_break` (680),
  `default_kotlin_import_tie_break` (628), `with_supertype_extension_fallback` (2315).

Two of these deserve naming as precedent. **`ambiguity_safe_tail_with_denylist` + `ModuleScopedOutcome`
(649) is the file's best existing work**: a pure aggregation stage whose three-variant outcome enum
exists specifically so "the caller must not collapse these into a single declined bucket" is carried
by the compiler rather than a comment — AGENTS.md's rule, applied. And
**`with_supertype_extension_fallback` (2315, PR #314, the newest code in the file) is already a
pure aggregation stage**: it takes two tiers and returns them in precedence order, and does nothing
else. The file is drifting toward this shape on its own; the audit accelerates it, it does not
impose it.

---

### Task 2a (NEW, runs BEFORE Task 2): extract the duplications that straddle the split

**Why this must precede the move, not follow it.** Task 2 Step 3's rule — *"Move code, do not rewrite
it"* — is correct and stays. But it means the move **freezes** whatever is there. The star-import
package list (item 2) has copies that land in *different* proposed modules, so after the split,
fixing it becomes a two-file change with a `pub(super)` widening to negotiate, instead of a
same-file deletion. Doing it first also *reduces* Task 2's own work: item 3 (the default-import
constant) removes one of the eighteen visibility widenings.

Scope is deliberately limited to duplications that **straddle a Task 2 module boundary, or are a
real, provable bug fixable without touching behaviour a test already depends on**. Everything else
in the audit — including two items round-4 review found were not safe duplications at all — is left
for Task 2b or the backlog. This is not a licence to refactor the file before moving it.

| # | Duplication | Copies | Straddles | Fix |
|---|---|---|---|---|
| 1 | `ExtensionEntry` → `Location` materialization, wrong-range match only | 1582–1604 | `qualified.rs` (all copies land here — see correction below) | Keep the fail-open, fix the range: swap `s.name == name` (1596) for `extension_declaration_matches(s, name, root_base, entry.container.as_ref())` |
| 2 | Star-import package list | 349, 1138, 2199 (`grep`-confirmed, 3 copies) | spine ↔ `package_scope.rs` | One `fn star_import_packages(indexer, uri) -> Vec<String>` |
| 3 | Kotlin's default-import package set | `KOTLIN_DEFAULT_IMPORT_PACKAGES` (596) vs `is_default_import_package`'s `matches!` (971) — identical 10-entry lists | both land in `imports.rs`, but the const is one of the 18 forced widenings | `is_default_import_package` reads the const; **removes 1 widening** |

**Round-4 review correction:** item 1 above was originally proposed as "merge copy 3 (1582–1604)
into a call to `resolve_extension_in_scope`, which is what it was always trying to be." That is
wrong and would have shipped a regression: `resolve_extension_in_scope` (reached from 1473) runs
`extension_is_in_scope` — a real package/import/container check. Copy 3 at 1582–1604 has **no such
check at all**; it is a deliberate fail-open fallback, and `tests.rs:3010`
(`resolve_extension_fn_on_uppercase_qualifier`) only passes **because** 1582 is more permissive than
1473, not despite it. 1473 and 1582 compute the same `root_base` and can return different answers
for the same input — that is not duplication, it is two different scoping policies for the same
receiver, and only one bug lives in the weaker one: it matches by `s.name == name` (1596) instead of
`extension_declaration_matches(s, name, root_base, entry.container.as_ref())`, so it can return a
same-named symbol from the wrong class entirely. Fixed as the new item 1 above — a 1-line range-match
swap, no merge, no behaviour lost.

The design doc's original item 3 (`location_package` at 831 "verbatim" vs the inline at 2171–2176)
is **dropped from this task.** Round-4 review found the inline deliberately omits
`location_package`'s middle branch (`indexer.files`), with its own justifying comment at 2166–2170 —
not a duplicate, a narrower intentional variant. Unifying it is a real behaviour question, not a
provable no-op; backlogged next to the two divergent dotted-name parsers below rather than folded in
here.

Not in this task, deliberately: the two divergent dotted-name parsers (186–211 vs 926–937) and the
`location_package` question above — both are genuine behaviour differences needing their own
measurement, not same-file duplication. Backlogged.

- [ ] **Step 1: Failing test for the real bug first.**
  `resolve_qualified_jar_extension_probe_respects_declaration_match_not_just_name` — a declaring file
  containing a **different, wrong-range** symbol that merely shares `name`, alongside the real
  extension declaration. Red today: 1582–1604's `s.name == name` match picks the wrong one. Also add
  `resolve_qualified_jar_extension_probe_stays_fail_open_when_the_in_scope_check_would_reject`,
  copying `tests.rs:3010`'s fixture shape — asserts the fix does **not** start requiring
  `extension_is_in_scope` at 1582 (that would be the regression the original item 1 would have
  shipped). This second test is the actual regression guard for this task; treat it as load-bearing,
  not optional.
- [ ] **Step 2: Verify both red/green as expected** (`_wrong_declaration` red, `_stays_fail_open`
  green against today's code — it documents current behaviour, it is not testing the fix yet).
- [ ] **Step 3: Implement items 1–2 above.** Item 1 is a one-line match swap, not a deletion.
  Item 2 is a deletion plus a call. Serena `find_referencing_symbols` per item, per AGENTS.md.
- [ ] **Step 4: Verify.** `cargo test --bin kmp-lsp` (both new tests green, `tests.rs:3010` still
  green) · `cargo clippy -- -D warnings` · `cargo fmt --check`.
- [ ] **Step 5: Measure.** Fix #1 changes real resolution behaviour, so it needs a corpus number:
  re-run `resolution-accuracy /home/ocel/Work/Moneta/android`, state before/after in the PR. A flat
  result is acceptable and expected (the wrong-range case is a goto-definition precision bug the
  harness does not score); a *regression* is not.
- [ ] **Step 6: Commit.** `fix(resolve): match the JAR-extension probe's fallback by declaration, not name alone, and de-duplicate the star-import package list and default-import constant`

---

### Task 2: Split `resolve.rs` into a spine plus named siblings

> **Revised 2026-09-11 by the Task 2 audit above.** Verdict on the audit's own central question —
> *do stage-based boundaries differ from the resolution-order boundaries already proposed here?*
> **No, and forcing four stage-named files would be strictly worse.** The distribution is not even:
> a `business_logic.rs` would be ~1,800 lines while `parsing.rs` would be ~40, because the parsing
> and normalization stages are mostly 2–6 line fragments *inline inside* business functions, not
> separable bodies. Four stage files would produce one unreadable file and three anaemic ones.
>
> The real finding is the opposite and more useful: **seven of the nine proposed modules already
> collapse onto the stage model under different names** — `tie_break.rs` *is* the aggregation module
> (every item in it is single-stage **A**), `package.rs` + `container.rs` *are* the normalization
> modules (every item **N** or **P**), `platform_types.rs` is static data + one **N**,
> `imports.rs`/`package_scope.rs`/`extension.rs` are coherent **B**-with-local-**P**. The taxonomy
> was right. The two modules that *don't* collapse cleanly are the **spine** and **`qualified.rs`**,
> and in both cases the reason is function-shaped, not file-shaped — which is why the fix is
> Task 2b, not a different set of files. Boundaries below are **kept**, with **one** correction.

**Why now, and why it moved.** This was in "Deliberately out of scope" with a specific, sound
reason: a split "would force widening ~14 private functions to `pub(crate)` right as Tasks 3–5 are
changing every one of their signatures, guaranteeing conflicts". **Tasks 3–5 are now parked
(Tasks 5–8), so that reason no longer holds.** The remaining active work — Task 3's ~20-line tier
fix and Task 4's `hierarchy.rs` change — touches a handful of functions, not every signature in the
file, and both are small enough to rebase across a pure-move commit trivially.

**Why after Task 1, not before it.** Task 1 is the only change with measured recall behind it and
should not wait behind a 2700-line mechanical move. Task 1's diff is ~30 lines in one function;
rebasing it onto the split, or the split onto it, is a one-file conflict either way. Ship the
measurable thing first.

**Files:**
- Modify: `src/resolver/resolve.rs` (shrinks to the spine)
- Create: `src/resolver/qualified.rs`, `extension.rs`, `imports.rs`, `package_scope.rs`,
  `tie_break.rs`, `package.rs`, `container.rs`, `platform_types.rs`, **`scope_check.rs`** (the
  audit's one boundary correction — see below)
- Modify: `src/resolver/mod.rs` (module registration + re-export paths)

**Boundaries — re-verified against the current file on 2026-09-10, not inherited.** The
2026-09-09 design doc (`docs/superpowers/specs/2026-09-09-resolve-latency-precedence-split-design.md`,
Gap 3) proposed these nine modules. The *groupings* survived re-verification and are kept. Its
*numbers* did not, and are corrected here. Method: enumerate every top-level item in `resolve.rs`
(60 items), assign each to a proposed module, then build the intra-file call graph with string
literals and `//` comments stripped, matching bare identifiers (so a function passed as a value —
e.g. `is_some_and(is_default_import_package)` at 1039/1048/1064 — is not missed).

| File | Contents | Verified size |
|---|---|---|
| `resolve.rs` (spine, kept) | `ensure_file_data` 51, `fqns_for_name` 73, `ResolveIo` 83, `resolve_symbol` 125, `resolve_symbol_index_only` 140, `resolve_symbol_with_io` 149, `resolve_callee_definition` 221, `resolve_chain` 250, `resolve_symbol_no_rg` 860, `resolve_symbol_hierarchy_ambiguity_safe` 875, `resolve_symbol_scoped_only` 895, `resolve_type_index_only` 919, `resolve_type_index_only_simple` 943, `resolve_local` 1719, `rg_location_satisfies_call_shape` 1752, the `impl Indexer` facade 2658 | **~673 lines** |
| `qualified.rs` | `resolve_qualified` 1444, `resolve_from_class_hierarchy` 2233, `resolve_from_class_hierarchy_scoped` 2251, `with_supertype_extension_fallback` 2315, `resolve_extension_via_supertype_hierarchy` 2361 | ~446 |
| `extension.rs` | `resolve_extension_in_scope` 1237, `resolve_implicit_receiver_callee` 1306, `implicit_receiver_extension_match` 1331, `implicit_receiver_member_match` 1396 | ~207 |
| **`scope_check.rs`** (audit correction) | `resolve_in_scope_strict` 1081, `receiver_provides_member` 1161 | ~135 |
| `imports.rs` | `KOTLIN_DEFAULT_IMPORT_PACKAGES` 596, `has_explicit_import` 960, `is_default_import_package` 971, `is_default_import_type` 992, `resolvable_via_default_import` 1025, `resolve_via_imports` 1940 | ~284 |
| `tie_break.rs` | `DENYLISTED_PACKAGE_PREFIXES` 516, `ambiguity_safe_tail_with_denylist` 556, `default_kotlin_import_tie_break` 628, `ModuleScopedOutcome` 649, `module_scoped_tie_break` 680, `import_package_tie_break` 725, `owning_module_dependencies` 770, `candidate_gradle_meta` 794, `is_denylisted_package_prefix` 816 | ~283 |
| `package_scope.rs` | `find_in_star_imports` 848, `resolve_same_package` 2071, `symbols_in_package` 2133, `find_symbol_in_package` 2138, `resolve_star_imports` 2194 | ~174 |
| `container.rs` | `import_container_chain` 1789, `enclosing_container_chain` 1806, `pos_tuple` 1830, `range_encloses` 1835, `resolve_companion_member` 1847 | ~151 |
| `package.rs` | `location_package` 831, `jar_symbol_package` 1775, `rg_in_package_dir` 2404, `package_dir_in_source_roots` 2465, `import_package_absent_from_source_roots` 2494 | ~146 |
| `platform_types.rs` | `is_stdlib` 2519, `KOTLIN_BUILTIN_TYPE_PLATFORM_EQUIVALENTS` 2554, `resolve_kotlin_builtin_type_platform_equivalent` 2614 | ~139 |

**Corrections to the design doc, each verified:**

1. **The spine is ~742 lines, not "~350".** `resolve_chain` alone (250–494) is 245 lines. The doc's
   350 figure is unreachable without also splitting `resolve_chain`, which is a *function*-shaped
   problem, not a file-shaped one, and is not attempted here. Say 742 in the PR description; do not
   repeat 350.
2. **18 private items need `fn` → `pub(super)`, not 8.** Verified list, with the module that forces
   each widening:

   | Item | Line | Home | Called from |
   |---|---|---|---|
   | `ambiguity_safe_tail_with_denylist` | 556 | tie_break | spine (`resolve_chain`) |
   | `KOTLIN_DEFAULT_IMPORT_PACKAGES` | 596 | imports | tie_break (`default_kotlin_import_tie_break`) |
   | `location_package` | 831 | package | tie_break, package_scope |
   | `find_in_star_imports` | 848 | package_scope | spine |
   | `has_explicit_import` | 960 | imports | spine (`resolve_in_scope_strict`) |
   | `resolvable_via_default_import` | 1025 | imports | spine (`resolve_in_scope_strict`) |
   | `resolve_extension_in_scope` | 1237 | extension | qualified |
   | `resolve_qualified` | 1444 | qualified | spine (`resolve_symbol_with_io`, `impl Indexer`) |
   | `import_container_chain` | 1789 | container | imports |
   | `enclosing_container_chain` | 1806 | container | extension, imports |
   | `resolve_companion_member` | 1847 | container | qualified |
   | `resolve_via_imports` | 1940 | imports | spine |
   | `resolve_same_package` | 2071 | package_scope | spine |
   | `resolve_star_imports` | 2194 | package_scope | spine |
   | `resolve_from_class_hierarchy` | 2233 | qualified | spine |
   | `rg_in_package_dir` | 2404 | package | package_scope |
   | `package_dir_in_source_roots` | 2465 | package | imports |
   | `import_package_absent_from_source_roots` | 2494 | package | spine |

   Note one of these is a **`const`**, not a `fn` — the doc's "8 functions" framing missed that
   category entirely. `pub(super)` is the right level for all 18: it means `pub(in crate::resolver)`,
   and every new sibling hangs directly off `resolver/`, so siblings can see each other's
   `pub(super)` items.
3. **`import_package_tie_break` (725) is already `pub(super)`** — no change needed. `range_encloses`
   (1835) is already `pub(crate)`, as the doc correctly noted.
4. **Items that stay private after the split** (verified: every caller lands in the same module) —
   `resolve_symbol_with_io`, `resolve_chain`, `resolve_type_index_only_simple`, `resolve_local`,
   `rg_location_satisfies_call_shape` (spine); `DENYLISTED_PACKAGE_PREFIXES`,
   `default_kotlin_import_tie_break`, `ModuleScopedOutcome`, `module_scoped_tie_break`,
   `owning_module_dependencies`, `candidate_gradle_meta`, `is_denylisted_package_prefix`
   (tie_break); `is_default_import_package`, `is_default_import_type` (imports);
   `implicit_receiver_extension_match`, `implicit_receiver_member_match` (extension); `pos_tuple`
   (container); `symbols_in_package` (package_scope); `resolve_from_class_hierarchy_scoped`,
   `with_supertype_extension_fallback`, `resolve_extension_via_supertype_hierarchy` (qualified);
   `KOTLIN_BUILTIN_TYPE_PLATFORM_EQUIVALENTS` (platform_types).
   Note `DENYLISTED_PACKAGE_PREFIXES` looks cross-module at first pass because `is_stdlib`
   (platform_types) mentions it — that mention is inside a **doc comment**, not code. Verified;
   it stays private.
5. **`mod.rs`'s re-export paths change for 4 of its 12 `resolve::` re-exports**, which the design
   doc does not mention at all: `find_symbol_in_package` → `package_scope`,
   `receiver_provides_member` → `extension`, `resolve_implicit_receiver_callee` → `extension`, and
   the `#[cfg(test)]` `resolve_kotlin_builtin_type_platform_equivalent` → `platform_types`. The
   other eight stay on `resolve::`. Because these are all `pub(crate)` already, only the path moves;
   no visibility changes and no consumer outside `resolver/` is touched.
6. **NEW (2026-09-11 audit) — `scope_check.rs`, the one boundary the resolution-order taxonomy got
   wrong.** `resolve_in_scope_strict` (1081) and `receiver_provides_member` (1161) are **not
   resolution-chain code at all** — they are the missing-import diagnostic's two reachability
   predicates, and `grep` confirms they have exactly **one** consumer between them:
   `src/features/missing_import_diagnostics.rs` (`:31` imports both; `:260` calls the first, `:268`
   and `:283` the second). Nothing else in the crate calls either. The resolution-order taxonomy has
   no slot for "predicate for a different feature", so it put one in the spine and the other in
   `extension.rs` — splitting a pair with a single shared consumer across two files, and leaving
   `extension.rs` holding a four-strategy in-scope check that merely *starts* with an extension
   lookup. Both are `pub(crate)` today, so `mod.rs`'s re-export paths move (add `scope_check` to the
   4 already changing in correction 5, making it 6 of 12) and no external consumer is touched.

   **Cost, stated honestly:** this is **+1 net widening, not −3.** `has_explicit_import` (960) and
   `resolvable_via_default_import` (1025) are widened either way (they move from spine-callers to
   scope_check-callers); `find_in_star_imports`, `resolve_same_package`, `resolve_via_imports` and
   `resolve_from_class_hierarchy` stay widened because `resolve_chain` also calls them; and
   `resolve_local` (1719) gains a **new** `pub(super)` it would not otherwise need. Task 2a's item 3
   (the default-import constant) removes one widening, so the table's total lands back at **18**.
   The justification is cohesion,
   not arithmetic: it makes `extension.rs` one job, removes the spine's second, independently-drifting
   resolution ladder (audit violation #4), and puts the missing-import diagnostic's two helpers in one
   reviewable file.

**Consistency with the two unification designs** (checked, per AGENTS.md and the design doc's own
"Relation to existing designs" section):

- `2026-06-30-cst-resolution-unification-design.md` — that doc fixes `resolver/resolve.rs` as the
  **string domain**, explicitly out of scope for CST unification. This split stays entirely inside
  the string domain: no new module imports `CstQuery`, `Resolution<T>`, or anything under
  `indexer/infer/`. What is borrowed is only that doc's *structural* rule — spine-as-catalogue with
  zero unrelated logic, each sibling named for its one responsibility.
- `2026-08-24-qualified-resolution-unification-design.md` — that doc's Primitive 1
  (`find_name_scoped_to_container`, `range_encloses`, `resolve_companion_member`) is the direct
  precedent for `container.rs`, which groups exactly the primitives that doc's reuse inventory
  already names as reused adjacent code. Its own `find_name_scoped_to_container` **stays in
  `find.rs`** — moving it would drag a file this task otherwise doesn't touch, and `find.rs` (364
  lines, and the file PR #315 just modified) is already the right home for the container primitives
  that operate on the symbol table rather than on `resolve.rs`'s chain.

- [ ] **Step 1: Establish the invariant — this task changes no behaviour**

Record the current full-suite pass count and a `resolution-accuracy` run *before* touching anything.
There is no failing test to write first: this task is a pure move, so the pre-existing suite **is**
the test. Say so in the PR rather than inventing a ceremonial new test.

- [ ] **Step 2: Move one module at a time, innermost-first**

Order: `platform_types` → `container` → `package` → `tie_break` → `package_scope` → `imports` →
`extension` → `scope_check` → `qualified`. Each is its own compile-and-test cycle; the borrow
checker and the privacy checker walk you outward. (`scope_check` after `extension` because it is
the last thing the spine sheds before `qualified` — it only calls outward, nothing calls into it
from inside `resolver/`.)

Per AGENTS.md, run `mcp__serena__find_referencing_symbols` on **each item immediately before moving
it** — the table above is a plan, the per-item check is the authority. Use
`mcp__serena__replace_content`, not sed. Activate Serena on this worktree first
(`mcp__serena__activate_project` with
`/home/ocel/Work/lsp/.worktrees/extension-supertype-variable-receiver`).

- [ ] **Step 3: Move code, do not rewrite it**

No signature changes, no renames, no logic edits, no comment rewrites — with exactly one exception,
already owed: the wrong comment at **resolve.rs:1564** claiming the 1553 path has ruled out an
"exact-key extension" (it has not; `resolve_from_class_hierarchy_scoped`'s callback is a pure member
lookup). Fix it as it moves into `qualified.rs`. Anything else that looks wrong gets a follow-up
issue, not a drive-by edit in a 2700-line move.

- [ ] **Step 4: Verify**

`cargo test --bin kmp-lsp` — pass count must **equal** Step 1's exactly · `cargo clippy -- -D warnings`
· `cargo fmt --check` · `git diff --stat` should show near-symmetric insertions/deletions.

- [ ] **Step 5: Confirm inertness on the corpus**

Re-run `resolution-accuracy` and diff against Step 1. Recall and the Gap top-20 must be
**byte-identical**. A pure move that changes a number is not a pure move.

- [ ] **Step 6: Commit**

```bash
git commit -m "refactor(resolver): split resolve.rs into a spine and nine named siblings

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01L7ZonwYUh94VsuQphiHykF"
```

**Explicitly not in this task:** splitting `src/resolver/tests.rs` (9368 lines) along the same
seams, and splitting `resolve_qualified` itself (~275 lines, still the largest function after the
move) — **that is now Task 2b, immediately below, rather than an unowned "legitimate follow-up".**
Also not in this task:
`complete.rs` (2346), `infer.rs` (2167) and `indexer/jar.rs`, all comparable in size — the
asymmetric result is accepted and named, not an oversight.

---

### Task 2b (NEW, runs AFTER Task 2): decompose `resolve_qualified` into named stages

**Why after the split, not before.** `qualified.rs` is then a ~446-line file whose entire subject is
this one function; the diff is reviewable in isolation, and the spine is not in it. Doing this before
the split buries a control-flow rewrite inside a 2700-line move — exactly what Task 2 Step 3's
"move, do not rewrite" rule exists to prevent.

**Why it is worth a task at all.** `resolve_qualified` is the file's only four-stage function, and
the mixing is not cosmetic — it is the documented cause of three of this plan's own findings:

> The uppercase branch (1468–1606) and the lowercase branch (1608–1710) independently implement the
> **same three-stage sequence**: parse the qualifier → normalize it to a receiver anchor → run a
> member/extension precedence ladder over that anchor. Because the sequence is written twice with no
> shared boundary, the two ladders have drifted, and **findings 9, 10 and 11 are all one bug class:
> "a tier that exists on one branch's ladder and not the other's."**

Current stage map, measured:

| Lines | Stage | What it does |
|---|---|---|
| 1451–1452 | **P** | `qualifier.split('.')`, `segments[0]` |
| 1455–1466 | **B** | `this` / `super` keyword branches |
| 1468–1469 | **P** | `root.starts_with_uppercase()`, `root.last_segment()` |
| 1473–1476 | **B** | own-type extension tier, keyed on `root_base` |
| 1482–1486 | **N** | root → candidate declaration `Location`s (IO-policy-aware) |
| 1487–1527 | **N** | per candidate: nested-segment walk, re-anchoring `anchor` + `anchor_class_name` |
| 1498–1504 | **B** | companion-member rule (single-segment only) |
| 1535–1575 | **B** | member → inherited-member → supertype-extension ladder, three early returns |
| 1580–1604 | **N+B** | the weak third extension probe (Task 2a fixes this) |
| 1609–1622 | **N+P** | `infer_variable_type`, `strip_nullable`, **third** dotted-name splitter |
| 1625–1631 | **N** | outer type → file; `current_type_base` tracked alongside |
| 1640–1670 | **N** | the *same* nested-segment walk as 1487–1527, written again for the lowercase case |
| 1676–1696 | **B** | member → inherited-member ladder — **two tiers, where uppercase has three** |
| 1709 | **B** | own-type extension tier — **present here, absent from the uppercase ladder's tail** |

Rows 1487–1527 and 1640–1670 are one normalization stage written twice. Rows 1535–1575 and
1676–1709 are one business stage written twice, with different tiers. That is the whole finding.

**Interfaces produced** (real signatures, each stage independently testable):

```rust
/// STAGE: parsing. Decided ONCE, instead of re-sniffed by `starts_with_uppercase`
/// at four separate points in one body. Takes no `Indexer` and does no IO, so its
/// tests need no fixture at all — today there is no way to test "did we read this
/// qualifier correctly" separately from "did we find the symbol".
enum QualifierRoot<'a> {
    /// `this.member` — current file, then its own hierarchy.
    This,
    /// `super.member` — hierarchy only.
    Super,
    /// `Foo.member` / `Outer.Inner.member` — every segment names a type.
    TypePath { root: &'a str, nested: &'a [&'a str] },
    /// `variable.field.member` — the root needs type inference before anything else.
    ValuePath { root: &'a str, rest: &'a [&'a str] },
}
fn parse_qualifier(qualifier: &str) -> QualifierRoot<'_>;

/// STAGE: normalization output. The receiver a qualified lookup is anchored on,
/// after the root AND every nested segment have been walked. `class_name` is the
/// LEAF type's simple name — the extension-registry key — never the root's.
/// Carrying the two together by construction is what makes finding 11's
/// "probe keyed on `root_base` while the anchor has already moved to `Inner`"
/// unrepresentable rather than merely fixed.
///
/// **Round-4 review correction:** the original draft carried `declaring_uri:
/// Option<Url>` instead of a full `Location` and could not construct
/// `own_members` at all — `find_all_names_scoped_to_container` (`find.rs`)
/// scopes its member search by matching the container's own declaration
/// *range*, not just its file, and `resolve_companion_member` needs the same
/// pair. `declaration` below carries what both actually need.
struct ReceiverAnchor {
    /// The leaf type's own declaration `Location` (file + range) — `None`
    /// only for a compiler built-in (`String`, `Int`) with no indexed
    /// declaration, which can still carry in-scope extensions (today's
    /// untyped `current_file: Option<String>`, which loses the range
    /// entirely and is why `own_members` had no way to be built pre-fix).
    declaration: Option<Location>,
    /// Leaf type's simple name, nullability and package prefix already stripped.
    class_name: String,
}

/// STAGE: normalization. Qualifier → receiver, and NOTHING else: no member
/// lookup, no extension probe, no precedence. Both of today's branches collapse
/// into this one function — which is what makes the ladder below provably share
/// an anchor instead of two anchors that drift.
///
/// Returns a `Vec` because an uppercase root can resolve to several candidate
/// declarations (today's `for qual_loc in &qual_locs` at 1487). Hoisting that
/// loop to the caller makes finding 1's `6 × |qual_locs|` re-entrancy cost
/// visible in a signature instead of buried mid-body.
fn anchors_for(
    indexer: &Indexer,
    root: &QualifierRoot<'_>,
    from_uri: &Url,
    io: ResolveIo,
) -> Vec<ReceiverAnchor>;

/// STAGE: aggregation. The four tiers, named and separately populated. Collapses
/// to the flat `Vec<Location>` `resolve_qualified` still returns at exactly one
/// seam, so "which order branches run" is carried by field order in one struct
/// rather than by statement order in two branches (AGENTS.md).
///
/// **Round-4 review found the order itself is not a formality to pick freely.**
/// Today's two branches disagree, not just about which tiers exist but about
/// the ORDER they run in: the uppercase branch probes `own_type_extension`
/// FIRST and returns early on a hit (1473–1476, before `own_members` is even
/// computed); the lowercase branch runs `own_members` → `inherited_members`
/// first and only reaches its extension tier last (1709). `into_precedence_ordered`
/// can only encode one order — so unifying the two branches through this one
/// struct is not neutral, it is a real, decided behaviour change for whichever
/// branch's current order loses.
///
/// **Decision, stated instead of left implicit: `own_members` →
/// `inherited_members` → `own_type_extension` → `supertype_extension`** — i.e.
/// the LOWERCASE branch's order, matching Kotlin's actual member-over-extension
/// precedence rule (stated repeatedly elsewhere in this plan and codebase: a
/// real member always wins over a same-named extension when both are
/// arity-compatible). The uppercase branch's current extension-first check is
/// the outlier, not the target — Task 2b intentionally corrects it, it does
/// not average the two. Any Moneta corpus movement this causes (a case where
/// an own-type member and an own-type extension share a name AND the member
/// used to lose) is exactly the kind of "explained per Gap entry" movement
/// Step 5 already gates; expect it to be rare, and do not treat its existence
/// as a sign Step 5 was skipped.
struct QualifiedCandidates {
    /// `find_all_names_scoped_to_container(indexer, name, &anchor.declaration?)`
    /// — `vec![]` when `anchor.declaration` is `None` (built-in receiver).
    own_members: Vec<Location>,
    /// `resolve_from_class_hierarchy_scoped` on the anchor.
    inherited_members: Vec<Location>,
    /// `resolve_extension_in_scope(anchor.class_name, …)` — finding 10/11's tier.
    own_type_extension: Option<Location>,
    /// `resolve_extension_via_supertype_hierarchy` — PR #314's tier. `Option`,
    /// not `Vec`: that function already collapses to at most one match.
    supertype_extension: Option<Location>,
}
impl QualifiedCandidates {
    /// `own_members` → `inherited_members` → `own_type_extension` →
    /// `supertype_extension`, per the decision above.
    fn into_precedence_ordered(self) -> Vec<Location>;
    fn is_empty(&self) -> bool;
}

/// STAGE: business logic. Kotlin's member-over-extension precedence, in ONE place,
/// for ONE anchor. Every tier here is a tier both of today's branches should have
/// had.
fn candidates_on(
    indexer: &Indexer,
    anchor: &ReceiverAnchor,
    name: &str,
    from_uri: &Url,
) -> QualifiedCandidates;

/// STAGE: business logic, kept separate because it answers a DIFFERENT question —
/// `Foo.member` with `Foo` a class name can only ever reach a companion member
/// (today's 1498–1504, correctly gated to the single-segment form). Reads
/// `anchor.declaration`'s `uri` for `resolve_companion_member`'s `file_uri`
/// parameter; `vec![]` when `anchor.declaration` is `None`, same as today (a
/// built-in receiver has no companion object to look up).
fn companion_member_on(indexer: &Indexer, anchor: &ReceiverAnchor, name: &str) -> Vec<Location>;
```

`resolve_qualified` itself then becomes roughly **25 lines**: parse, dispatch the two keyword
variants, get anchors, loop, rank, return.

**Compatibility with the `MemberTier` / `MemberExtensionCandidates` ideas already in this plan.**
Checked directly against both, because "supersedes" is a claim that needs stating rather than
implying:

- **`MemberExtensionCandidates`** (shelved design doc, Gap 2: `{ member: Vec<Location>,
  extension_fallback: Option<Location> }`) is **superseded in shape, vindicated in spirit.**
  `QualifiedCandidates` is the same idea with four tiers instead of two, and the shelved doc's own
  self-critique explains why it could only reach two: its Gap-2 WHY, item 1 admits the struct
  "doesn't see this branch at all, because it never reaches `with_supertype_extension_fallback`",
  and item 2 admits a third call site "computes the identical two-tier precedence by hand instead of
  through the type". **Both limitations are artifacts of typing the ladder's *output* while the
  ladder itself is still spread across four early-return exits in two branches.** Extract the anchor
  first — as Task 2b does — and the four-field version is constructible at every exit, because there
  is only one exit. The shelved doc's structural argument for a *struct* over a `Resolution<T>`-style
  outcome enum ("member-empty/extension-nonempty, member-nonempty/extension-empty, and both-nonempty
  are all real, simultaneously-possible states") is correct and is carried forward unchanged.
- **`MemberTier`** (the four-variant enum cut from Task 3) **stays cut, but for a different reason
  than the one recorded there.** Task 3's argument is that two of four variants would never be
  constructed and would be dead code under `-D warnings` — which is true *only* under Task 3's own
  stated constraint that "the member and inherited-member tiers stay where they are". Under Task 2b
  all four tiers are constructed in `candidates_on`, so that objection evaporates. The enum is still
  the wrong shape, for the shelved doc's reason above: the tiers are simultaneously possible, not
  mutually exclusive. **Four fields, not four variants.** Record this so nobody re-derives the cut
  from a premise Task 2b has removed.

**Interaction with Tasks 1 and 3.** Both should land first (Task 1 is measured; Task 3 is cheap and
red). Task 2b then *absorbs* most of Task 3's diff rather than conflicting with it: Task 3's Step 3
table routes four separate exits through a shared tier helper, and after Task 2b there is one exit,
so the own-type extension tier is one `Option` field being populated. If Task 3 has not landed when
Task 2b starts, do not fold it in silently — land Task 3's **tests** first (they are the proof the
decomposition preserved the tier order) and let Task 2b make them pass structurally. Task 1's
`member_or_inherited_member` helper becomes `candidates_on`'s first two fields; the getter retry
stays a separate name-mapping concern, per Task 1's own note.

- [ ] **Step 1: Lock behaviour before touching control flow.** This is the highest-risk task in the
  plan — a control-flow rewrite of the file's hottest function. Record the full-suite pass count and
  a `resolution-accuracy` baseline (same discipline as Task 2 Step 1).
- [ ] **Step 2: Write the stage tests, and see them pass against today's code where they can.**
  `parse_qualifier` gets pure unit tests with no fixture (`"this"`, `"super"`, `"Foo"`,
  `"Outer.Inner"`, `"account.holder"`, `"a.b.C.d"`). `anchors_for` gets tests that assert on the
  **anchor** — `class_name == "Inner"` for `Outer.Inner.member` — which is the assertion that is
  impossible to write today and is the one finding 11 needed. Also add
  `resolve_qualified_uppercase_receiver_own_member_now_wins_over_own_type_extension` — a fixture with
  BOTH a same-named own-type member and a same-named own-type extension on an uppercase-root
  receiver, asserting the MEMBER location wins. This is red against today's uppercase branch (which
  checks the extension first, at 1473, and returns before `own_members` is ever computed) and green
  after Task 2b — it is the test for the precedence-order decision above, not incidental coverage.
  Note in the PR description that this is a deliberate, named behaviour correction, not a regression.
- [ ] **Step 3: Extract, innermost-first.** `parse_qualifier` → `ReceiverAnchor`/`anchors_for` →
  `QualifiedCandidates`/`candidates_on` → rewrite `resolve_qualified`'s body. Each is its own
  compile-and-test cycle. Serena `find_referencing_symbols` per item.
- [ ] **Step 4: Verify.** Pass count must **equal** Step 1's. `cargo clippy -- -D warnings` ·
  `cargo fmt --check`.
- [ ] **Step 5: Confirm on the corpus.** Re-run `resolution-accuracy`. Unlike Task 2, this one is
  **not** required to be byte-identical — unifying two drifted ladders necessarily gives the weaker
  branch the tiers it was missing, which is the point. Any movement must be **explained per Gap
  entry**, not waved through as noise, and any *regression* blocks the commit.
- [ ] **Step 6: Commit and open a PR.**

```bash
git commit -m "refactor(resolve): give resolve_qualified one parse, one anchor, and one precedence ladder

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01L7ZonwYUh94VsuQphiHykF"
```

**Explicitly not in this task:** `resolve_chain` (audit violation #2, 11 section comments) and
`resolve_via_imports` (#3). Both are real and both are larger than they look — `resolve_chain` is
the whole IO-policy ladder and every `ResolveIo` behaviour hangs off its statement order. They go to
the backlog, to be reconsidered **after** `qualified.rs` proves the pattern on the smaller case.
Shipping one decomposition and measuring it beats designing three.

---

### Task 3: Close the own-type extension tier (was Task 1)

> **Status change, stated plainly.** This was "the actual Moneta fix". It is not — Task 0's
> measurement and PR #315 settled that. Findings 10 and 11 are still real and still red (re-verified
> 2026-09-10), and the fix is still ~20 lines, but **no currently-measured Gap requires it.** It is
> kept because it is cheap, it is correct, and finding 11's `Outer.Inner.member` shape is a real
> Kotlin construct that will eventually appear in a corpus. It is **not** kept on a promise of
> recall. If it measures flat, that is the expected outcome, not a failure — say so in the PR.
>
> PR #314 shipped `with_supertype_extension_fallback` and its two call sites (1537, 1680). That is
> this task's *scaffolding*, not this task. Verified 2026-09-10: the helper's body (2315–2336) still
> calls only `resolve_extension_via_supertype_hierarchy`, and `walk_hierarchy_breadth_first`
> (`hierarchy.rs:85`) still never passes `start_class` to `collect`.



**Files:**
- Modify: `src/resolver/resolve.rs` — or `src/resolver/qualified.rs` if Task 2 has landed first, which it should have (`with_supertype_extension_fallback` **2315**, not 2296; its two call sites 1537 / 1680; the two statement-order pairs 1553→1566 and 1692→1709; and the wrong comment at 1564 — **already fixed as part of Task 2 Step 3, so skip it here if Task 2 is in**)
- Modify: `src/resolver/tests.rs` (new tests)

**Interfaces:** internal only. `with_supertype_extension_fallback`'s signature already carries everything needed (`anchor_class_name`, `anchor_uri`, `name`, `from_uri`).

**On the `MemberTier` enum an earlier draft proposed — cut.** It listed four variants (`OwnMembers`, `OwnTypeExtension`, `InheritedMembers`, `SupertypeExtension`) but this task only ever *constructs* two of them: the member and inherited-member tiers stay where they are, as `find_all_names_scoped_to_container` (1535) and `resolve_from_class_hierarchy_scoped` (1553) calls that this task does not move. Two never-constructed variants are dead code under `cargo clippy -- -D warnings` and are exactly the speculative flexibility AGENTS.md says not to build.

The remaining two tiers are appended in a fixed order inside **one shared helper** called from every exit — which is what actually delivers the "both branches provably share one order" property. A two-variant enum matched in the single function that also builds both variants adds a name, not a guarantee. Routing all four tiers through a real type means restructuring both branches' control flow (each tier has a different `return` shape and a different anchor), which is a materially larger diff than the ~10 lines this task is sized at; that is **Task 3b** in "Deliberately out of scope".

> **Updated 2026-09-11 by the Task 2 audit.** The cut stands, but the *reason* recorded above is
> conditional on this task's own constraint that the member and inherited-member tiers stay put.
> **Task 2b now owns the restructuring this paragraph defers**, and under it all four tiers are
> constructed, so the "two never-constructed variants = dead code" argument no longer applies —
> `MemberTier` stays cut because the tiers are *simultaneously possible, not mutually exclusive*
> (four struct fields, not four enum variants). "Task 3b" is superseded by Task 2b; see there.
> This task's ~20-line version remains the right thing to ship **now**, before Task 2/2b, and its
> tests are what prove Task 2b preserved the ordering.

**Interaction with Task 1, if Task 1 landed first (it should have).** Task 1 introduces
`member_or_inherited_member` — the extracted pair of the 1535 and 1553 lookups. Task 3's own-type
extension tier goes **after** both of that helper's invocations (real name, then getter name) and
**before** the supertype-extension tier, preserving Kotlin's member-over-extension precedence across
both. Do not fold the getter retry into the tier helper: they answer different questions (one is a
name mapping, one is a lookup scope) and `and` in a name means it is doing two things.

- [ ] **Step 1: Write the failing tests**

Add to `src/resolver/tests.rs` (place next to the existing `resolve_qualified_appends_supertype_extension_alongside_a_wrong_arity_concrete_member` at line 1665, which is the closest sibling and the right fixture template):

1. `resolve_qualified_lowercase_receiver_appends_own_type_extension_alongside_wrong_arity_member` — a `val nav: NavController` where `NavController` declares `navigate(uri: Uri)` and a separate file declares `fun NavController.navigate(route: String)`. Assert **both** locations come back, extension second. This is the exact Moneta shape and is red today.
2. `resolve_qualified_uppercase_receiver_own_type_extension_unchanged` — the uppercase branch already handles this at 1473; assert it still does (regression guard for the refactor).
3. `resolve_qualified_own_type_extension_does_not_shadow_an_arity_compatible_member` — member and extension both named `foo`, member arity matches: assert the member is first. Kotlin precedence must not invert.
4. `resolve_qualified_supertype_extension_still_reached_when_no_own_type_extension_exists` — the PR #314 behaviour must survive; the existing tests at 1479/1528/1576 already cover most of this, so this one only needs to assert ordering relative to the new tier.
5. `resolve_qualified_nested_type_qualifier_finds_an_extension_on_the_nested_type_itself` (finding 11) — `Outer.Inner.member()` where `member` is `fun Outer.Inner.member()` / an extension keyed on `Inner`, **not** on `Outer` and **not** on any ancestor. Write **two** variants, because they exercise different exits and only one of them is closed incidentally:
   - `…_alongside_a_wrong_arity_member` — `Inner` declares a wrong-arity `member`, so 1535 returns non-empty and 1537 is the exit. Verified red today; Step 3's change to `with_supertype_extension_fallback` closes it for free, since 1537 already passes the *reassigned* `anchor_class_name`.
   - `…_when_the_nested_type_has_no_such_member` — `Inner` declares no `member` at all, so the 1553/1566 pair is the exit. Verified red today and **not** closed by the 1537 change alone; this is the test that forces Step 3 to route the 1553/1566 pair through the shared helper too.

- [ ] **Step 2: Verify they fail**

`cargo test --bin kmp-lsp resolve_qualified_lowercase_receiver_appends_own_type_extension` → FAIL (returns only the member location).

- [ ] **Step 3: Implement**

Refactor `with_supertype_extension_fallback` into a tier-ordered helper (rename it — `with_extension_fallback` — since it is no longer supertype-only) that appends, in order: own-type extension (`resolve_extension_in_scope(indexer, anchor_class_name, name, from_uri)`) then supertype extension (existing `resolve_extension_via_supertype_hierarchy`). It keys **only** on its `anchor_class_name` parameter, never on `root_base`, so a nested-type anchor is handled by construction (finding 11).

Route **all four** exits through it:

| Exit | Anchor passed | Closes |
|---|---|---|
| 1537 (uppercase, member found) | `anchor_class_name` (already reassigned) | findings 9 + 11 |
| 1680 (lowercase, member found) | `current_type_base` | finding 10 — the Moneta case |
| **1553/1566 pair** (uppercase, no member) | `anchor_class_name` | finding 11's second variant |
| 1692/1709 pair (lowercase, no member) | already tries own-type at 1709 | finding 9's statement-order drift |

For the 1553/1566 pair specifically: `resolve_from_class_hierarchy_scoped` stays a pure member walk; the own-type extension probe is inserted **between** 1553 and 1566, i.e. the helper is called with an empty `member_locs`. While there, **fix the comment at resolve.rs:1564** — it claims that path has already ruled out an "exact-key extension", which is false (verified: `resolve_from_class_hierarchy_scoped`'s `walk_hierarchy` callback is `find_name_in_uri` only, no extension probe). That wrong comment is very likely how the hole survived review in the first place.

Leave the *member-first* semantics exactly as they are — this only adds a tier between "member found" and "supertype extension".

Do **not** touch budgets in this task.

- [ ] **Step 4: Verify**

`cargo test --bin kmp-lsp` (expect all pre-existing green + 6 new) · `cargo clippy -- -D warnings` · `cargo fmt --check`.

- [ ] **Step 5: Commit and open a PR**

```bash
git commit -m "fix(resolve): try the receiver's own extension key after a wrong-arity member

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01L7ZonwYUh94VsuQphiHykF"
```

- [ ] **Step 6: Measure, and expect nothing**

Re-run `resolution-accuracy`. **A flat result is the predicted outcome** — no name in the current
Gap top-20 is this bug class (see Task 0's taxonomy). Record the flat number honestly in the PR
rather than hunting for a narrative. The tests are the justification here, not the corpus.

**This task is independently shippable. It does not close any measured gap.**

---

### Task 4: Close the `hierarchy.rs` budget leak (was Task 4 — promoted out of the parked block)

> **Why this is no longer part of the budget architecture.** Finding 5 is a real bug on its own
> terms: `supertype_targets` calls `resolve_symbol_hierarchy_ambiguity_safe` at `hierarchy.rs:240`
> and `:275` **without** passing the walk's `sidecar_budget`, so those calls silently fall to
> Regime A's zero. That is a dropped budget, not a mis-classified one, and fixing it needs no
> `LatencyClass`, no `JarPromotionBudget`, and no threading refactor — it needs the existing
> `&mut usize` passed to two call sites that currently do not receive it.
>
> Doing it here, with the existing type, is a handful of lines. Doing it as part of Task 7 couples
> a real correctness fix to a parked architecture. Decoupled.

**Files:** `src/resolver/hierarchy.rs`, `src/resolver/hierarchy_tests.rs`

- [ ] **Step 1: Failing test** — `hierarchy_walk_shares_its_budget_with_the_ambiguity_safe_tail`:
  a fixture where the *supertype's own name resolution* (not the supertype promotion) needs a cold
  JAR. Red today because 240/275 receive 0. Model the promotion-counting assertion on the existing
  harness at `tests.rs:7156–7198`.
- [ ] **Step 2: Verify red.**
- [ ] **Step 3: Implement** — thread `supertype_targets`' existing `sidecar_budget: &mut usize`
  into the `resolve_symbol_hierarchy_ambiguity_safe` calls at 240 and 275. `resolve.rs:875`'s
  signature will need to accept it; keep the change to `&mut usize` (**not** a new type) so this
  task stays independent of Tasks 5–8. If Task 5 ever lands, it subsumes this parameter naturally.
- [ ] **Step 4: Verify** — full `cargo test`, clippy, fmt.
- [ ] **Step 5: Commit** — `fix(resolver): stop dropping the hierarchy walk's budget at the ambiguity-safe tail`.

---

## ⏸ PARKED: Tasks 5–8 — the `LatencyClass` / `JarPromotionBudget` architecture

**Status: parked 2026-09-10, not deleted.** These four tasks (originally Tasks 2, 3, 5 and 6) are
real, correct, and were hardened across three independent critique rounds. They are parked for one
reason: **every justification they were given turned out to be a decoy.**

- The Moneta `navigate` gap they were meant to unblock was a container-scoping bug (PR #315).
- Every measurement taken so far — Task 0's baseline, the tier-hole probe, and the 2026-09-10
  re-run — was on a **warm** cache. Task 0 Step 3 (the cold-cache isolation) was never run.
- Not one name in the current Gap top-20 is budget-limited. The dominant remaining class is
  receiver-type inference (Class B, ~1000 occurrences), which no budget change reaches.

**What is still true and independently valuable in them, so nobody re-derives it:**

- Finding 4's three-regime picture (18 zero-budget sites, 11 bounded-at-3, 5 genuinely unbounded)
  is verified and is the only complete inventory that exists.
- Finding 12's discovery that `features/rename.rs:125` is unbounded **on purpose** — an
  under-promoted supertype walk there does not degrade a read, it fails to detect an override and
  lets a wrong workspace edit through — is a live trap for anyone who "tidies up" budget literals.
  **Read finding 12 before touching any `usize::MAX` in this codebase, parked or not.**
- Finding 1's re-entrancy shape (`resolve.rs:1487`'s `for qual_loc` loop mints a fresh budget-3 per
  iteration, so cost is `6 × |qual_locs| + 3`, not 6) is a genuine per-keystroke cost nobody has
  measured against a wall clock.

**Resumption gate — do not restart these on reasoning alone.** Resume only when *both* hold:

1. A **cold-cache** `resolution-accuracy` run (sidecar cache dir cleared) shows a materially worse
   Gap list than the warm run, **and** a named Gap entry is traced to a budget exhaustion — the
   Task 0 methodology (temporarily patch the budget, re-measure, live-trace) applied to that
   specific entry. This is Task 0 Step 3, still unrun.
2. …or a **wall-clock** measurement shows the keystroke-diagnostics path actually spending the
   `6 × |qual_locs| + 3` blocking round trips finding 1 predicts. Note the design doc's own open
   question: budget bounds *how many* blocking calls happen, never *how long* any one takes, so a
   latency complaint may not even be a budget problem.

If neither is measurable, the honest outcome is that Tasks 5–8 stay parked indefinitely, and
Task 4 above (the one real bug in the block) is the whole of the value that gets extracted. That is
an acceptable ending for three rounds of good review — the review was not wasted, it produced
finding 12 and the regime inventory.

**When resuming: every line number in Tasks 5–8 below predates PR #315 and is stale by +19 to +25
for anything after `resolve_qualified`. Re-measure first** (see the drift note under "Verification
of the prior findings").

---

### Task 5 (PARKED, was Task 2): Introduce `LatencyClass` + `JarPromotionBudget` (types only, zero call sites)

**Files:**
- Create: `src/resolver/budget.rs`
- Create: `src/resolver/budget_tests.rs`
- Modify: `src/resolver/mod.rs` (register + re-export)

**Interfaces produced:**

```rust
/// How much blocking sidecar IPC the *request* behind this resolution can
/// afford. Not a property of any function — a property of who is waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LatencyClass {
    /// A user explicitly asked and is watching a spinner: goto-definition,
    /// hover, signature help, code actions, find-references.
    Interactive,
    /// Fired by typing, with the next keystroke already queued behind it:
    /// `did_change` diagnostics, inlay hints, completion-as-you-type.
    Keystroke,
    /// A user is waiting, but a *wrong answer edits their code*, so latency
    /// loses to correctness: rename (`features/rename.rs:125`), which today
    /// passes `usize::MAX` and refuses the whole edit when
    /// `verify_candidates` reports a `proven_overrides` hit. Under-promoting
    /// a supertype walk there does not degrade a read — it fails to detect
    /// an override and lets a wrong workspace edit through. Rename is also
    /// rare, explicitly invoked, and already blocking on a workspace-wide rg
    /// scan, so the marginal sidecar cost is bounded by candidate count
    /// rather than by keystroke rate.
    ///
    /// This class exists so no generic mapping can quietly cap rename at 3.
    /// Adding a member here needs the same argument: correctness, not taste.
    Exhaustive,
    /// Nobody is waiting: `indexer/apply.rs`'s completion-cache prewarm
    /// (`spawn_blocking` under a `Semaphore::new(4)`, apply.rs:1242-1250),
    /// CLI batch scans.
    Background,
}

/// Remaining blocking JAR-promotion round trips for ONE request.
///
/// The struct has no public constructor — the only way to obtain an owned
/// value is [`LatencyClass::budget`] — so the shape is at least readable
/// off the signature: owning one means you minted it, holding `&mut` means
/// you were handed one. Not `Copy`: a copy would silently double the
/// request's budget. See `budget_tests.rs::mint_sites_match_the_allowlist`
/// for what actually *enforces* where minting happens.
#[derive(Debug)]
#[must_use]
pub(crate) struct JarPromotionBudget { remaining: usize }
```

with `LatencyClass::budget(self) -> JarPromotionBudget` (**`pub(crate)`**, see below), and on `JarPromotionBudget`: `fn as_mut_usize(&mut self) -> &mut usize` (the single bridge to the existing `jar::*` helpers, so `jar.rs` needs no change at all this phase) and `fn remaining(&self) -> usize` (tests only).

Initial numbers, chosen to be inert (Constraint 1): `Interactive` → `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK`, `Keystroke` → `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK`, `Exhaustive` → `usize::MAX`, `Background` → `usize::MAX`. Yes, two pairs are numerically identical today. `Interactive`/`Keystroke` diverge in Task 6; `Exhaustive`/`Background` may never diverge and that is fine — they are distinguished so a latency tune cannot touch rename's correctness budget by accident.

**On visibility, corrected:** an earlier draft declared `budget()` `pub(super)` and claimed that "blocks every minter outside `crate::resolver`, which covers `features/`, `indexer/`, `workspace/`, `cli/`". That claim does not survive this plan's own later tasks. Task 5 migrates `src/indexer.rs` (471/545/1051), Task 6 migrates `indexer/lookup.rs:151`, `indexer/resolution.rs:673/756`, `features/rename.rs:125` and the `references.rs`/`references_verify.rs` chain — **all outside `crate::resolver`**, and every one of them must mint. `pub(super)` would not compile there; the implementer would widen it to `pub(crate)` the moment they reached Task 5, silently deleting the stated safety property. So declare it **`pub(crate)` from the start**, and do not sell visibility as enforcement it cannot provide.

**On enforcement, honestly:** Rust cannot express "only entry points may call `LatencyClass::budget`", and with a `pub(crate)` mint the *only* real mechanism is the source-scanning allowlist test below. Visibility does exactly one thing here: it keeps the type out of the crate's public API. Everything else is the test.

That makes hardening the test load-bearing rather than decorative. The naive version — line-by-line, non-recursive `src/resolver/*.rs` — has at least four false-negative modes, all of which must be handled:

1. **rustfmt line-wrapping.** `let mut budget = LatencyClass::Interactive.budget();` fits on one line, but a mint inside a long call expression wraps, putting `LatencyClass::Exhaustive` and `.budget()` on different lines. Scan the **whitespace-normalized whole file** (read to `String`, collapse all runs of whitespace to a single space) and match on that, never line-by-line.
2. **Doc-comment and string false positives** — this plan's own text, and the doc comments above, contain `LatencyClass::budget`. Strip `//`-prefixed content (after normalization, strip from `//` to the original line end — so do the comment strip *before* collapsing newlines) before matching.
3. **The test file and fixtures themselves.** `budget_tests.rs` mints freely; so may other `*_tests.rs`. Exclude every file whose name ends in `_tests.rs` from the scan, and say so — the allowlist governs production mint sites only.
4. **Non-recursive glob.** The real mint surface after Tasks 5–6 spans `src/resolver/`, `src/indexer/`, and `src/features/`. Walk `src/` **recursively** (a ~15-line `fn visit(dir, &mut Vec<PathBuf>)` over `std::fs::read_dir`; no new dependency) and collect every `.rs` file.

The allowlist is `(relative path, enclosing fn name)` pairs; deriving the enclosing fn from a normalized string is fragile, so use the cheap approximation: track the most recent `fn <name>(` seen before the match offset. If that proves flaky in practice, fall back to `(relative path, count)` — the point is that a *new* mint site fails CI with a readable diff, not that the report is beautiful.

- [ ] **Step 1: Write the tests**

`src/resolver/budget_tests.rs`:

- `budget_is_not_copy_and_decrements_once` — mint, hand `&mut` to two helpers, assert total spend is the class's budget, not double.
- `mint_sites_match_the_allowlist` — recursively walk `src/` (relative to `env!("CARGO_MANIFEST_DIR")`), skipping `*_tests.rs`; per file, strip `//` comments, collapse whitespace, then collect every `LatencyClass::<Variant>.budget()` occurrence and assert the resulting `(relative path, enclosing fn)` set equals a hardcoded allowlist. This is **the** mechanical enforcement of "only entry points mint" — not visibility. A new mint site fails CI with a diff of what changed. Start the allowlist empty (Task 5 adds no call sites).
- `allowlist_test_catches_a_wrapped_mint` — a meta-test with a fixture string containing a rustfmt-wrapped `LatencyClass::Exhaustive\n    .budget()` and a doc-comment mention of the same, asserting the scanner finds exactly the first and not the second. Without this the hardening is untested and will silently rot back to a naive line scan. (The scanner must therefore be a plain `fn scan(src: &str) -> Vec<&str>` taking source text, not a function that reads files itself.)

- [ ] **Step 2: Verify red** — `cargo test --bin kmp-lsp budget` → compile failure.
- [ ] **Step 3: Implement `src/resolver/budget.rs`** per the interfaces above; register `pub(crate) mod budget;` in `src/resolver/mod.rs` and re-export `{LatencyClass, JarPromotionBudget}` alongside the existing `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` re-export at mod.rs:26.

  Note: nothing consumes these items outside tests yet. The pre-commit hook runs bare `cargo clippy -- -D warnings` (no `--tests`), so add `#![cfg_attr(not(test), allow(dead_code))]` at the top of `budget.rs` with a comment naming Task 3 as its removal point — the same pattern `unresolved_symbol_diagnostics.rs` used in the 2026-08-15 plan.
- [ ] **Step 4: Verify green** — `cargo test --bin kmp-lsp budget` · `cargo clippy -- -D warnings` · `cargo fmt --check`.
- [ ] **Step 5: Commit** — `feat(resolver): add LatencyClass + JarPromotionBudget (unused)`.

---

### Task 6 (PARKED, was Task 3): Thread the budget through `resolve.rs`

> If Task 2 has landed, this task's single-file scope is wrong: the mint sites are now spread across
> `resolve.rs`, `qualified.rs`, `extension.rs`, `imports.rs` and `package_scope.rs`. Re-derive the
> table below from the split files, and note the split makes this task *easier* to review, not
> harder — each file's budget threading is its own reviewable unit.

**Files:**
- Modify: `src/resolver/resolve.rs`
- Modify: `src/resolver/budget.rs` (remove the dead-code suppression; populate the allowlist)
- Modify: `src/resolver/budget_tests.rs` (allowlist entries)
- Modify: `src/resolver/tests.rs` (only where a test asserts promotion counts)

**Shape (this is the part that must not be improvised):**

Every promotion-capable private function in `resolve.rs` gains a `budget: &mut JarPromotionBudget` parameter, and its Regime-A `let mut cache_backed_only = 0usize` / Regime-B `MAX_SYNC…` literal is deleted. Concretely:

| Function | Line | Today | After |
|---|---|---|---|
| `resolvable_via_default_import` | 1025 | mints 0 @1033 | takes `&mut` |
| `receiver_provides_member` | 1161 | mints 0 @1186 | takes `&mut` |
| `resolve_extension_in_scope` | 1237 | mints 0 @1246 | takes `&mut` |
| `implicit_receiver_extension_match` | 1331 | mints 0 @1338 | takes `&mut` |
| `resolve_qualified` | 1444 | mints 0 @1581 | takes `&mut` |
| `resolve_via_imports` | 1940 | mints 0 @1991 | takes `&mut` |
| `resolve_same_package` | 2071 | mints 0 @2109 | takes `&mut` |
| `find_symbol_in_package` | 2138 | mints 0 @2161 | takes `&mut` |
| `resolve_from_class_hierarchy_scoped` | 2251 | mints 3 @2271 | takes `&mut` |
| `resolve_extension_via_supertype_hierarchy` | 2342 | mints 3 @2366 | takes `&mut` |
| `with_supertype_extension_fallback` | 2296 | — | takes `&mut` (pass-through) |
| `resolve_chain`, `resolve_symbol_with_io`, `resolve_local`, `resolve_companion_member`, `resolve_star_imports`, `ambiguity_safe_tail_with_denylist` | 250, 149, 1719, 1847, 2194, 556 | — | take `&mut` (pass-through) |

The **only** mint sites become the `impl Indexer` wrappers at resolve.rs 2637–2678. Each existing wrapper keeps its exact signature (Constraint 2) and mints:

```rust
pub(crate) fn resolve_symbol(&self, name: &str, qualifier: Option<&str>, from_uri: &Url) -> Vec<Location> {
    let mut budget = LatencyClass::Interactive.budget();
    resolve_symbol(self, name, qualifier, from_uri, &mut budget)
}
```

plus **new** siblings that take the class explicitly, for the callers that know better:

- `resolve_symbol_for(&self, …, class: LatencyClass)` — used by `apply.rs:1250` (`Background`) and `sig.rs:631` (`Keystroke`).
- `resolve_member_only_for(&self, …, class: LatencyClass)` — used by `Resolver::resolve_member` (`api.rs:222`), which passes `Keystroke` because its only caller is `nullable_call_diagnostics.rs:115` on the `did_change` path.

**This is where the re-entrancy hazard (item 1) dies:** `resolve_qualified`'s internal calls at 1485/1625/1656/1666 go to the *private* `resolve_symbol(…, &mut budget)`, reborrowing the caller's budget. There is no owned `JarPromotionBudget` in scope inside `resolve_qualified` to double-mint from, and the `for qual_loc` loop at 1487 now shares one draining budget across all iterations instead of minting `6 × N`.

- [ ] **Step 1: Write the failing test**

Add to `src/resolver/tests.rs`, modelled on the existing promotion-counting harness at 7156–7198:

`resolve_qualified_shares_one_budget_across_multiple_qualifier_candidates` — a fixture with two same-named qualifier-root candidates, each with a JAR-backed ancestor chain; assert total attempted promotions `<= Interactive`'s budget, not `2 ×` it. Red today (the loop mints per iteration).

- [ ] **Step 2: Verify red.**

- [ ] **Step 3: Implement, one function at a time, innermost-first**

Order: `resolve_extension_in_scope` → `resolve_from_class_hierarchy_scoped` / `resolve_extension_via_supertype_hierarchy` → `with_supertype_extension_fallback` → `resolve_qualified` → `resolve_chain`/`resolve_symbol_with_io` → the `impl Indexer` wrappers. Compile after each; the borrow checker walks you outward.

Use `mcp__serena__replace_symbol_body` / `replace_content` per AGENTS.md, not sed.

- [ ] **Step 4: Add the allowlist entries** in `budget_tests.rs` — exactly the `impl Indexer` wrapper functions in `resolve.rs`. Remove `budget.rs`'s `#![cfg_attr(not(test), allow(dead_code))]`.

- [ ] **Step 5: Verify** — `cargo test --bin kmp-lsp` (all green; the 66 `tests.rs` call sites must be untouched — if any needed editing, the wrapper signature changed and Constraint 2 was violated) · `cargo clippy -- -D warnings` · `cargo fmt --check`.

- [ ] **Step 6: Confirm inertness** — re-run `resolution-accuracy <moneta-root>` and diff against `/tmp/moneta-baseline.txt` + Task 1's and Task 3's post-fix numbers. Recall must be **unchanged**. A change here means a budget value moved and the refactor was not inert.

- [ ] **Step 7: Commit** — `refactor(resolve): thread one JarPromotionBudget per request through resolve.rs`.

---

### Task 6b (PARKED, was Task 4): Convert `hierarchy.rs` to the budget type

> **The bug half of the old Task 4 was promoted to Task 4 above and is no longer parked.** What
> remains here is the type conversion only: `walk_hierarchy`/`walk_hierarchy_breadth_first` change
> `sidecar_budget: usize` → `budget: &mut JarPromotionBudget`; `supertype_chain_contains` (336) and
> `receiver_type_agreement` (368) — and therefore `Resolver::receiver_type_agreement` (`api.rs:196`
> doc, impl ~245) — take the budget through as well. **Do the correctness fix (Task 4) first and
> independently; this is cosmetics on top of it.**

- [ ] **Step 1: Implement.** `api.rs`'s public `receiver_type_agreement` already takes an explicit `sidecar_budget: usize` — change it to `LatencyClass` and mint inside, keeping the "caller states its tolerance" property while removing the raw number from the trait surface. Its callers are `references_verify.rs` **126 / 157 / 180** (all fed by the one `sidecar_budget` parameter at `:41`); to keep this task inert, forward whatever class reproduces today's value at each — `Interactive`/3 for the `references.rs` path, `Exhaustive`/`usize::MAX` for the `rename.rs` path. The `features/` **signatures** (`verified_references_for`, `verify_candidates`) are Task 8 Step 1b's job; this task only changes what they forward into.
- [ ] **Step 2: Verify** — full `cargo test`, clippy, fmt. Re-check `tests.rs:7156/7186/7195/7198`, which reference the constant directly and will need the new API.
- [ ] **Step 3: Commit** — `refactor(resolver): carry the hierarchy walk's budget as JarPromotionBudget`.

---

### Task 7 (PARKED, was Task 5): Extend into `infer.rs` and `indexer.rs`

> The old subtitle was "this is what makes Task 1 work on a cold cache". Task 1 is now Task 3, and
> nothing measured needs it warm *or* cold — so this task's premise is exactly what the resumption
> gate above exists to re-test. Note the one part of finding 6 that survives independently:
> `infer.rs` **1683–1684** is the wall that stops `rememberNavController()` yielding `NavController`
> on a cold JAR, and the current Gap top-20's dominant Class B (receiver-type inference) lives in
> these same files — so if anything here gets resumed, resume it as *inference* work with its own
> measurement, not as budget threading.

**Files:** `src/resolver/infer.rs`, `src/indexer.rs`, `src/resolver/tests.rs`

Migrate the Moneta-relevant sites identified in finding 6:

- `infer.rs` **1683–1684** (`find_fun_return_type_reachable`) — the wall that stops `rememberNavController()` from yielding `NavController` cold.
- `infer.rs` **1911–1914** (extension return type).
- `infer.rs` **1331**, **1404**, **1425**, **2058** — the remaining mints, for consistency.
- `indexer.rs` **471**, **545**, **1051**.
- `infer_variable_type` (`infer.rs:636`) and its `_impl`/`_core` chain gain the pass-through parameter so `resolve_qualified:1609` can hand its budget down.

- [ ] **Step 1: Failing test** — cold-cache variant of Task 3's Moneta test: the extension's JAR is present in Tier-1 but not materialized, and the receiver's type must be inferred from a function return. Assert it resolves. This is the test that proves the whole plan.
- [ ] **Step 2: Verify red** (Task 3's fix is green warm, red cold).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify** — full suite, clippy, fmt, and a **cold-cache** `resolution-accuracy` run (clear the sidecar cache dir first).
- [ ] **Step 5: Commit** — `fix(resolve): thread the promotion budget into type inference so cold JAR extensions resolve`.

**Deferred within this task if it runs long:** `sig.rs` (240, 737, 1011, 1051, 1164) and `complete.rs` (302, 587, 814, 860, 1957, 2029) are not on the Moneta path. Split them into a Task 5b rather than stretching this one.

---

### Task 8 (PARKED, was Task 6): Retire Regime C, migrate `features/`, and tune the numbers (the only behaviour change)

**Files:** `src/indexer/lookup.rs`, `src/indexer/resolution.rs`, `src/resolver/complete.rs`, `src/features/rename.rs`, `src/features/references.rs`, `src/features/references_verify.rs`, `src/resolver/budget.rs`

This task owns the `features/` migration — no earlier task covered it, and finding 12 is why it cannot be skipped.

- [ ] **Step 1:** Replace the five non-test `usize::MAX` sites with an explicit `LatencyClass`. Each needs a decision recorded in the commit message: is this read really unbounded-tolerant, or was `usize::MAX` an untriaged default?

  | Site | Class | Behaviour change? |
  |---|---|---|
  | `indexer/lookup.rs:151` | triage | possibly — record the decision |
  | `indexer/resolution.rs:673` | triage | possibly |
  | `indexer/resolution.rs:756` | triage | possibly |
  | `resolver/complete.rs:814` | almost certainly `Keystroke` | **yes** — inside completion; this is a real cap where there was none |
  | `features/rename.rs:125` | **`Exhaustive`** | **no — must stay unbounded** |

  Rename is the one site in this table that is *not* a triage question. Per finding 12 it is unbounded today, and it is unbounded for a reason: `verify_candidates`'s `receiver_type_agreement` calls (references_verify.rs 126/157/180) feed `proven_overrides`, which rename uses to **refuse** an unsafe edit. A missed promotion there does not produce a worse read, it produces a wrong workspace edit. Map it to `Exhaustive` and leave the number at `usize::MAX`.

- [ ] **Step 1b: Thread the class through `features/`.** `verified_references_for` (`references.rs:80`, param at `:87`) and `verify_candidates` (`references_verify.rs:41`) change `sidecar_budget: usize` → `class: LatencyClass` (they hand it to `indexer.receiver_type_agreement`, whose signature Task 4 already changed to take a `LatencyClass`; if that call needs a shared draining budget instead, mint once in `verify_candidates` and pass `&mut` to 126/157/180 — but note this **would** be a behaviour change for find-references, which today gets a fresh 3 per candidate, so if you do that, say so and measure). `references.rs:45` passes `Interactive`, `rename.rs:118` passes `Exhaustive`. Three further `usize::MAX` arguments in `references_verify.rs`'s own tests (680/715/745) move with the signature.

  Note `MAX_VERIFICATION_IO_OPERATIONS = 48` (`references_verify.rs:13`) is a *different* budget — IO-costed verification steps, not sidecar promotions — and is deliberately out of this plan's scope. Do not merge the two.

- [ ] **Step 2:** Split `Interactive` from `Keystroke` numerically in `budget.rs`. Suggested starting point: `Keystroke` = 3 (unchanged), `Interactive` = 12. Not more — an interactive request that spends 12 × ~200ms cold round trips is already a 2.4s goto-definition. **Do not touch `Exhaustive`** — it is a correctness budget, not a latency knob; capping it needs its own PR with a rename-specific override-detection test proving nothing is missed at the cap.
- [ ] **Step 3: Measure.** `resolution-accuracy` warm and cold, plus a manual latency check: open a large Kotlin file in the editor, type in a body, and confirm diagnostics still land sub-second. Record both in the commit message.
- [ ] **Step 4:** Revert any number that costs latency without buying recall. This step is allowed to end with "no change" — the architecture from Tasks 2–5 is worth having regardless.
- [ ] **Step 5: Commit** — `perf(resolver): classify every promotion site by latency tolerance`.

---

## Phase / shipping order (revised 2026-09-10)

| Task | Ships independently? | Measured recall effect | Kind | State |
|---|---|---|---|---|
| 0 | n/a | n/a | measurement | **DONE** |
| — | PR #314 (`e8080a59`) | **flat (measured)** | bug fix | open, on this branch |
| — | PR #315 (`f0e5ed56`) | **`navigate` 220→0** | bug fix | open, on this branch |
| **1** | **yes** | **expected: `fail` (103) + `currentIdentity` (74) leave the Gap top-20** | bug fix | **next** |
| **2a** | yes | expected flat; **must not regress** | bug fix + de-duplication (2026-09-11 audit) | before Task 2 |
| **2** | yes (inert by construction) | **must be zero** | refactor (file split, 9 siblings) | after Task 2a |
| **2b** | yes | movement allowed, **must be explained per Gap entry** | refactor (`resolve_qualified` stages) | after Task 2 |
| **3** | yes | expected **flat** — no measured gap needs it | bug fix (findings 10 + 11) | after Task 2, before 2b |
| **4** | yes | none (correctness, not recall) | bug fix (finding 5) | any time; independent |
| 5–8 | — | — | `LatencyClass` architecture | **PARKED** behind the resumption gate |

**If time runs out, ship Task 1 alone.** It is the only item on this list with hard measured
evidence in front of it. Task 2 is the user-requested structural work and is inert by construction.
Task 3 is cheap and correct but should not be sold as a recall win. Task 4 is a genuine bug that
happens to be small. Tasks 5–8 buy nothing observable and must not be started without a measurement
that satisfies the resumption gate.

**Ordering rationale, since this is the question the revision turned on:** the split moved from
"after Tasks 3–5" to second because its original blocker — signature churn from the budget
threading — was parked. It stays *behind* Task 1 because a 2700-line mechanical move should never
block the one change with numbers behind it, and rebasing a 30-line fix across a pure move costs
nothing either way.

**Ordering rationale for 2a / 2b (added 2026-09-11):** 2a before 2 because a pure move *freezes*
duplication, and four of the audit's duplications have copies landing in different modules — after
the split each becomes a two-file change with a visibility negotiation instead of a same-file
deletion. 2b after 2 because it is a control-flow rewrite of the file's hottest function and is only
reviewable once `qualified.rs` exists as a ~446-line file about that one function. Task 3 sits
between them deliberately: its tests are what prove 2b preserved the tier order, so they must exist
and be green before 2b starts.

---

## Deliberately out of scope

- ~~**Splitting `resolve.rs` into 8 files.**~~ **Promoted to Task 2 on 2026-09-10.** The stated
  reason for deferring it — "a split would force widening private functions right as Tasks 3–5 are
  changing every one of their signatures" — died when those tasks were parked. The old entry also
  under-counted the widening (~14 → verified 18) and mis-sized the result (~350-line spine →
  verified ~742). Both corrected in Task 2.
- **`sig.rs` and `complete.rs` budget migration** — named in Task 7's deferral note as Task 7b. They carry 10 of the 33 mint sites but are provably off the Moneta path (finding 6). Leaving them un-migrated means the codebase temporarily has two conventions; that is an accepted, named cost, not an oversight.
- **Making `jar.rs`'s helpers take `JarPromotionBudget` directly.** `as_mut_usize()` bridges to the existing `&mut usize` API. Changing `jar.rs` would add ~10 files of churn (including `indexer/jar_tests.rs`) for a purely cosmetic gain. Do it in the same commit as Task 5b or never.
- **A per-request budget carried in a context/request object.** That is the "right" long-term shape, but it means an ambient parameter through `Indexer`'s entire read surface. `&mut JarPromotionBudget` gets the same safety with a diff that stops at the resolver.
- **Caching negative promotion results across requests.** `promote_candidates_bounded` already memoizes `materialized`/`materialization_failed` per JAR per session (hierarchy.rs:9–22 documents this). Nothing more is needed.
- **Item 9's uppercase pair as a separate change.** Folded into Task 3 — same helper, same file, same test fixtures. Finding 11 made it load-bearing rather than optional: the 1553/1566 exit is the one Task 3's 1537 change does *not* close incidentally.
- ~~**Task 3b — routing all four precedence tiers through a real type.**~~ **Promoted to Task 2b on
  2026-09-11.** Its stated blocker was "reconsider only after Task 2 has landed and `qualified.rs`
  exists as its own reviewable file" — that is now a scheduled precondition rather than an open
  question, and the audit supplied the missing piece the old entry lacked: the tiers cannot be typed
  until the *anchor* is extracted first (see Task 2b's `MemberExtensionCandidates` analysis).
- ~~**Splitting `resolve_qualified` (~275 lines) itself.**~~ **Promoted to Task 2b on 2026-09-11.**
  The old entry called it "a function-shaped problem the file split deliberately does not address",
  which was correct — the 2026-09-11 audit's finding is that it is *the* function-shaped problem in
  this file, and the cause of findings 9/10/11 rather than a cosmetic sibling of them.
  **Still out of scope: splitting `src/resolver/tests.rs`** (9368 lines) along Task 2's seams —
  `tests.rs` already treats the resolver as one flat test surface, and nothing in Tasks 2/2a/2b
  changes that.
- **The rest of the 2026-09-11 stage audit.** Named so it is backlog rather than forgotten, and
  deliberately not planned until Task 2b has proven the pattern on the smaller case:
  - `resolve_chain` (audit violation #2, **11** numbered section comments in one body). The largest
    single instance of AGENTS.md's own "section comments signal a split" rule in the crate, but also
    the riskiest: every `ResolveIo` behaviour hangs off its statement order, and its Swift branch
    (305–328) and four-arm tail (456–493) each want a different extraction shape.
  - `resolve_via_imports` (#3, P+N+B+A+IO across `// i)` `// ii)` `// iii)`).
  - **The two divergent dotted-name parsers** — `resolve_symbol_with_io` 186–211 (skips lowercase
    package segments, walks N levels) vs `resolve_type_index_only` 926–937 (splits at the first dot
    only, does not skip package segments). They behave differently *today*, so unifying them is a
    behaviour change needing its own measurement, not a de-duplication. A third, `resolve_qualified`
    1617–1642, is absorbed by Task 2b.
  - `resolve_companion_member` (1847–1924): two different algorithms in one body, switched on
    `indexer.jar_files.contains_key` — the JAR-vs-source `FileData` shape difference leaking into a
    business function. `find.rs` already absorbs that same difference once
    (`find_all_names_with_container_in_uri`, PR #315); this is the second place that wants it.
  - `receiver_provides_member`'s depth-**24** hierarchy walk (1209–1224) where every other walk in
    the file uses 12 — justified by a comment, not a type.
- **`indexer/jar.rs:1349`'s `promote_candidates` unbounded local.** Real (finding 4), but off the per-request path: sole caller `ensure_jar_materialized` (`jar.rs:1285`), sole non-test caller of *that* is `workspace/document_handler.rs:587` — per-import promotion at file open. Bounding it is a file-open-latency question with its own measurement, not a resolution-budget question. Noted, not fixed.
- ~~**The JAR-extension probe at `resolve.rs:1578`** (now **1582–1604**).~~ **Partly promoted to
  Task 2a, fully closed by Task 2b.** The old entry described *one* defect (it re-keys on
  `root_base`, so `Outer.Inner.member` never probes `Inner` — finding 11's third miss). The
  2026-09-11 audit found **two more, worse ones** in the same block, and they do not need the
  control-flow change the old entry deferred for: it applies **no in-scope check at all** (unlike
  `resolve_extension_in_scope`) and matches its result range with `s.name == name` instead of
  `extension_declaration_matches` (`grep`-verified: that helper's only `resolve.rs` call sites are
  1276 and 1369). Both are fixed in **Task 2a** by making the block call
  `resolve_extension_in_scope`, with a test. The `root_base`-vs-`anchor_class_name` keying is the
  part that genuinely needs the loop hoisted, and **Task 2b's `ReceiverAnchor` makes it
  unrepresentable** rather than fixed.
- **`references_verify.rs`'s `MAX_VERIFICATION_IO_OPERATIONS` (48).** A per-request cap on IO-costed *verification steps* (fresh disk reads, walk attempts), not on sidecar promotions. Different unit, different failure mode. Explicitly not merged into `JarPromotionBudget`.

---

## Backlog — the rest of the 2026-09-10 Gap taxonomy, sized but not planned

Recorded so the next round starts from evidence instead of re-deriving it. Ordered by measured
top-20 volume. **Re-measure before picking one: Task 1 will change the list.**

| Class | Measured top-20 volume | What it is | Assessment |
|---|---|---|---|
| **B — receiver-type inference through generic and chained calls** | ~1000 across 10 names (`finish` 224, `title` 195, `text` 164, `scenes` 134, `fragmentArguments` 123, `firstOrNull` 118, `launch` 104, `toString` 93, `toInt` 81, `await` 76, `filter` 76, `add` 69) | The *member* is indexed and findable; the *receiver's type* is never computed. Sources: generic return types (`fun <T> T?.required(field): T`), builder chains, Compose `CompositionLocal.current`, `Deferred` from `async {}`, `Optional.get()`. | **The largest remaining target by a wide margin, and the least cheap.** Lives in `resolver/infer.rs` + `indexer/infer/chain.rs`, not `resolve.rs`. Needs its own design doc and its own measurement per sub-shape — several of these are probably 3–4 distinct bugs wearing one hat. Do **not** open as one task. |
| **D — smart casts** | ~200 (`start` 120, `finishAffinity` 81) | `if (x is T) x.member()` and `when (e) { is T -> e.member }`. The receiver's *declared* type is used; the narrowed type is not. | Cheap-ish and self-contained. PR #271 already handled a smart-cast/field collision in the when-diagnostic; this is the resolution-path sibling. Best next candidate after Task 1. |
| **E — extensions on an unconstrained type parameter** | 111 (`run`) | `fun <T> T.run(block: T.() -> R): R` is keyed on a type *variable*, so an exact-string receiver-key lookup can never match it. PR #309's default-import tie-break helped `apply`/`run` in one direction but 111 remain. | Needs a decision about how `extension_by_receiver` represents a type-parameter receiver at all — a small type-design question, not a small patch. |
| **F — `vararg` parameter receiver type** | ≤81 (`forEach`, in the `fun Card.atLeastOneRightAllowed(vararg rights: CardRight)` shape) | A `vararg t: T` parameter's type is `Array<out T>`, not `T`. The resolver appears to use the declared element type as the receiver. | Smallest and sharpest of the four. Verify the share of `forEach`'s 81 that is actually vararg before committing — the example site is, the other 80 may not be. |
| **G — enum synthetic members** | contributes to `name` 311 | `it.name` on an enum entry needs `entries` → element type → the implicit `Enum` supertype. At least two compounding gaps stacked, which is why `name` is #1. | Do not attack `name` as one item; it is a bucket, not a bug. |

**Method note, for whoever picks the next one up.** The taxonomy above was built by opening each
harness-reported `file:line:col` in the real corpus, reading the receiver expression, and tracing the
receiver's declaration — plus `kmp-lsp find <name>` to settle whether the *member* was indexed at
all. That last check is what separated Class A (member missing under that name) from Class B
(member present, receiver type missing), and it is what stopped two plausible "same class as
PR #315" leads (`finish`, `launch`) from becoming wrong tasks. **Run it before writing a task, not
after.**
