# `resolve.rs` Latency Signal, Member/Extension Precedence, and File Split — Design

Status: **proposed** (2026-09-09). Scope: `src/resolver/resolve.rs` (2673 lines). Written
against `fix/extension-supertype-variable-receiver`, after the supertype-extension fallback
(`with_supertype_extension_fallback` / `resolve_extension_via_supertype_hierarchy`) landed on
this branch. Builds on three existing documents without re-deriving them:

- `docs/architecture/unified-resolution-handler.md` — introduced `ResolveIo` as the chain's
  **IO policy** axis (which subprocesses/fallbacks a resolution pass may use). This document
  does not touch that axis; it adds an orthogonal one (see Gap 1).
- `docs/superpowers/specs/2026-06-30-cst-resolution-unification-design.md` — establishes that
  `resolver/resolve.rs` is the **string domain**, deliberately separate from the CST domain,
  and explicitly out of scope for that unification. This document stays entirely inside the
  string domain; it borrows that doc's *structural* discipline (mod.rs-as-catalogue,
  type-driven correctness) without importing any CST-domain machinery.
- `docs/superpowers/specs/2026-08-24-qualified-resolution-unification-design.md` — the
  string-domain per-hop scoping primitives (`find_name_scoped_to_container`, `range_encloses`,
  `resolve_companion_member` as precedent). This document's file split organizes around
  exactly that reuse inventory rather than inventing a competing one.

## Context (why)

`resolve.rs` is 2673 lines and answers three unrelated complaints at once:

1. **No latency-class signal.** `resolve_qualified` and the bare-name spine (`resolve_chain`)
   share `ResolveIo::Full` between two callers with opposite latency tolerance — one-shot
   interactive navigation (`resolve_symbol`, `resolve_callee_definition`) and the per-keystroke
   nullable-dot-call diagnostic (`resolve_member_only`). Every JAR-promotion budget call site
   (`let mut cache_backed_only = 0usize;`) is hardcoded to zero because there is no way to grant
   real budget to the first group without also granting it to the second. This blocks a real,
   confirmed gap: the supertype-extension fallback can find the right ancestor class but still
   can't promote a never-before-seen extension JAR (the Moneta `navController.navigate(...)`
   case), because the promotion lookup underneath the walk is zero-budget regardless of caller.
2. **Member-vs-extension precedence isn't a type.** `with_supertype_extension_fallback` returns
   `Vec<Location>` with the real member first and the extension appended after — Kotlin's own
   "member always wins" rule lives only in `.extend()` call order plus a comment.
3. **The file itself.** 2673 lines, ~58 top-level items, is too large to safely reason about or
   review a diff against. It needs real module boundaries, not a token split.

All three are entangled: the latency-signal gap (1) lives inside the exact function
(`resolve_qualified`) whose internal ordering is also gap (2)'s target, and both live inside the
file being split in (3). This document treats them as one piece of work with three deliverables.

## Goals / non-goals

**Goals**
1. Give `resolve_symbol` itself — not just `resolve_qualified`/`resolve_chain` — an explicit,
   compiler-required, threaded latency-budget parameter, independent of `ResolveIo`, so a future
   caller can grant real JAR-promotion budget to interactive navigation without silently granting it
   to the keystroke diagnostics path *and* without a re-entrant internal call (`resolve_qualified`
   calling `resolve_symbol` on itself) accidentally re-minting a fresh budget mid-chain. See Gap 1
   WHY for why threading through `resolve_qualified`/`resolve_chain` alone is not sufficient.
2. Replace `with_supertype_extension_fallback`'s `Vec<Location>` + `.extend()`-order precedence
   encoding with a type that makes "member always tried before extension" a property of the type,
   not a comment.
3. Split `resolve.rs` into cohesive sibling modules under `resolver/`, matching the file's own
   existing "resolution order" taxonomy and the qualified-resolution doc's reuse inventory,
   each landing in the same size range as the existing `hierarchy.rs`/`find.rs` siblings.

**Non-goals**
- Re-tuning `ResolveIo` itself, or touching its four other variants (`NoRg`/`IndexOnly`/
  `ScopedOnly`/`HierarchyAmbiguitySafe`) — `unified-resolution-handler.md` already scoped
  `ResolveIo` to IO/subprocess policy; this document does not extend that scope.
- Actually granting a nonzero interactive budget to `resolve_symbol`/`resolve_callee_definition`
  (i.e., fixing the confirmed Moneta `navController.navigate` gap end-to-end). This document
  makes that change *safe* (Gap 1's mechanism); flipping the default from 0 to a real budget for
  those two callers is a one-line follow-up once the mechanism lands, not part of this design.
- The CLI `hover` subcommand's separate, shallow resolution path (`src/cli/hover.rs`) — unrelated
  gap, not touched by this split.
- `resolve_root_node_type`'s suspected literal-type-name fallthrough (`indexer/infer/chain.rs`) —
  CST domain, unverified, out of scope.
- Merging the string and CST domains, or building any shared IR between them — both prior docs
  already rejected this; nothing here revisits it.
- Splitting `src/indexer/jar.rs` (1633 lines) or `src/resolver/infer.rs` (2167 lines). Both are
  read for context (jar.rs is the seam Gap 1's budget threads into; infer.rs is the sibling
  string-domain file `find_field_type_in_class` lives in, per the qualified-resolution doc's Site
  1). Neither is restructured here — flagged as a follow-up in Open questions.

## Gap 1: a latency-class signal, safe against the diagnostics-regression failure mode

### WHY — revised after review; the original "three entry points" model was unsound

**The original version of this document was wrong about the shape of the problem, not just its
detail.** It claimed `resolve_qualified` has exactly two direct callers, that the whole chain is
driven by exactly three `ResolveIo::Full`-passing "entry points" (`resolve_symbol`,
`resolve_callee_definition`, `resolve_member_only`), and that each entry point could safely
materialize its own fresh `sidecar_budget` once "per request." Both claims are true only in the
narrowest, syntactic sense — direct call sites *inside `resolve.rs`* — and both break down as soon
as the real call graph is followed past that file's boundary and past `resolve_qualified`'s own
body. Two independent problems, confirmed by direct code reading (not `find_referencing_symbols`
alone, which only reports symbol-to-symbol edges, not which of those edges are self-referential):

**Problem A — `resolve_symbol` is re-entrant, called from *inside* `resolve_qualified` itself.**
`resolve_qualified`'s own body (resolve.rs:1443-1709) calls the free function `resolve_symbol` at
four separate points, not zero:

- resolve.rs:1485 — resolving the qualifier root itself (`Foo.member` → resolve `Foo`), guarded by
  an `io == IndexOnly` check that only chooses between `resolve_symbol`/`resolve_symbol_index_only`
  — the `Full` path always reaches `resolve_symbol`.
- resolve.rs:1624 — resolving a lowercase-root variable's inferred type to its declaring file.
- resolve.rs:1655 — resolving an uppercase nested-type segment when it isn't found in the current
  file.
- resolve.rs:1665 — resolving a field's inferred type to its declaring file, mid-chain-walk.

A fifth, structurally identical instance sits one hop away: `implicit_receiver_member_match`
(resolve.rs:1396-1428, in the proposed `extension.rs` grouping) loops `for type_loc in
resolve_symbol(indexer, receiver_base, None, from_uri)` at line 1403 — an *unbounded* loop over
however many locations `resolve_symbol` returns, not a fixed count. This function is reached via
`resolve_implicit_receiver_callee`, a sibling top-level entry point to `resolve_qualified` (used by
the implicit-receiver-call diagnostics/goto-def path), not literally nested inside
`resolve_qualified` — but it is driven into existence by the exact same design flaw described next.

Under the original design — "each of the 3 entry points materializes its own fresh
`sidecar_budget` once" — `resolve_symbol` is simultaneously **one of those three entry points**
(classified `Interactive`, since `features/definition.rs`/`hover.rs` call it directly for goto-def
and hover) **and** a helper called from deep inside another entry point's own call graph
(`resolve_member_only`'s `Keystroke` chain, via `resolve_qualified`). A call chain that starts as
`resolve_member_only` (`Keystroke`, budget 0) descends into `resolve_qualified`, which — per its
own body, not a hypothetical future caller — calls `resolve_symbol` again at line 1485/1624/1655/
1665. If `resolve_symbol` is the place that decides "I am an entry point, materialize my own
budget," it has no way to know it is being called mid-chain from a `Keystroke` request instead of
fresh from `features/hover.rs`, and would grant a full `Interactive` budget right there — the exact
diagnostics-hot-path regression this document exists to prevent, caused by the design's own
internals, not a future caller failing to opt in correctly.

**Fix:** `resolve_symbol` itself (not just `resolve_qualified`/`resolve_chain`) must take
`sidecar_budget: &mut usize` as a required parameter. This is the load-bearing change — it turns
"don't accidentally re-materialize budget mid-chain" from a convention someone has to remember into
something the compiler enforces: a function that receives `&mut usize` can only reborrow it, it
cannot manufacture a second one out of nothing. The corollary: **there is no clean, enumerable set
of "entry points."** The correct mental model is one shared, re-entrant call graph
(`resolve_symbol` ⇄ `resolve_qualified` ⇄ `resolve_chain` ⇄ their helpers), and budget gets
threaded in wherever a *true* external caller — a function that does not itself receive
`sidecar_budget` from someone else — first calls into that graph. See WHAT/HOW below for the
threading design and Problem B for why the list of true external callers is also longer than
originally scoped.

**Problem B — `resolve_symbol` has far more external callers than the original three-entry-point
story accounted for, including a second, independent per-keystroke diagnostic surface.**
Re-derived via `find_referencing_symbols` on both the free function `resolve::resolve_symbol` and
the `Indexer::resolve_symbol` facade method (resolve.rs:2634-2641, the thin wrapper the method form
calls through to), followed past `resolve.rs`'s file boundary:

| Caller | File:line | Nature |
|---|---|---|
| `resolve_symbol_with_io` | resolve.rs:162 | spine dispatch (unchanged from original doc) |
| `Indexer::resolve_member_only` | resolve.rs:2670 | per-keystroke nullable-dot-call diagnostic (unchanged) |
| `resolve_qualified` (×4, internal) | resolve.rs:1485,1624,1655,1665 | **re-entrant — Problem A above, missed entirely by the original doc** |
| `implicit_receiver_member_match` | resolve.rs:1403 (unbounded loop) | reached via `resolve_implicit_receiver_callee`, a sibling entry point |
| `Indexer::find_definition` | indexer/lookup.rs:40 | `#[allow(dead_code)]` today, but a real `pub(crate)` API — must still be classified |
| `Indexer::find_definition_qualified_with_io` (×2) | indexer/lookup.rs:81,90 | the real goto-definition path `features/definition.rs` sits behind; the second call site is a `SCOPE_FUNCTIONS` (`apply`/`let`/…) fallback inside the same function |
| `Indexer::resolve_locations` | indexer/resolution.rs:704 | gated by its own `allow_rg` flag; its own further callers were **not traced in this pass** — flagged below as unresolved |
| background completion-cache warming | indexer/apply.rs:1249-1250 | **not interactive and not per-keystroke either** — runs inside `tokio::spawn` + `spawn_blocking`, throttled by a 4-permit semaphore, triggered after a file is applied to the index, one call per referenced type name. A genuinely third traffic shape this document's two-class `LatencyClass` doesn't have a good answer for yet (see Open questions). |

None of the `lookup.rs`/`resolution.rs`/`apply.rs` rows were in the original doc's caller table at
all — that table only looked at direct callers of `resolve_qualified`/`resolve_chain` within
`resolve.rs`, not at the wider set of things that reach `resolve_symbol` (both forms) through
`indexer/`. `resolve_qualified` genuinely does have only two *direct, syntactic* callers within
`resolve.rs` (`resolve_symbol_with_io`, `resolve_member_only` — that part of the original claim
holds under review), but that fact says nothing about how many *distinct request shapes* ultimately
reach the shared `resolve_symbol`/`resolve_qualified`/`resolve_chain` graph, which is the number
that actually matters for latency-class safety.

**A claimed second per-keystroke path was investigated and does *not* exist on this call graph —
correcting an over-read, not confirming it.** The review that produced this list also flagged
`indexer/infer/sig.rs:631` (inside `find_fun_signature_with_receiver`, which does call
`idx.resolve_symbol(&receiver_type.outer, None, uri)`) as "reached from
`features/call_arg_diagnostics.rs:134`." Traced directly: `call_arg_diagnostics.rs:134` calls
`resolve_call_signature` (sig.rs:1252), which dispatches to sig.rs's **own**, unrelated
`resolve_qualified`/`resolve_unqualified` (sig.rs:990, sig.rs:1138) — functions that read
`idx.definitions`/`idx.jar_definitions` directly and never call `resolve_symbol` at all (see Minor
point 9 below on the naming collision). `find_fun_signature_with_receiver` is a *different* free
function, called instead from `features/completion.rs:436`, `features/completion_context.rs:152`,
`features/signature_help.rs:32,41`, and `features/traits_impl.rs:158` (the `SignatureIndex` trait
impl). So the claim's literal citation doesn't hold — but the underlying concern was still
under-scoped in the other direction: completion and signature-help are themselves fired on nearly
every keystroke inside a call's argument list, and neither was in the original doc's caller table
either. They're added to the table above via `find_fun_signature_with_receiver`'s call to
`idx.resolve_symbol` — a fifth kind of caller (per-keystroke-adjacent, but currently unclassified
in this design) beyond the diagnostics/goto-def/hover split the original doc assumed was
exhaustive.

**Net effect on scope:** the "real migration surface" is not "three entry points inside
`resolve.rs`." It is: `resolve_symbol` (free fn + facade method) must take a threaded budget
parameter; every one of the rows in the table above becomes a place that either (a) reborrows an
already-threaded budget (the resolve.rs-internal rows) or (b) must decide, for the first time, what
`LatencyClass` it is (the `lookup.rs`/`resolution.rs`/`apply.rs`/`completion*`/`signature_help.rs`
rows). Classifying (b) correctly is now part of this document's job, not a follow-up — seven
external files, not one caller.

The 9 confirmed zero-budget sites (`cache_backed_only` / `budget = 0usize`), all currently hit
regardless of `io` — the original doc's table found 8 and missed a second, distinct site inside a
function it had already identified:

| Site | Enclosing function |
|---|---|
| resolve.rs:1032-1033 | `resolvable_via_default_import` |
| resolve.rs:1185-1186 | `receiver_provides_member` (JAR-definitions promotion) |
| resolve.rs:1210-1218 | `receiver_provides_member`, **second site** — passes a literal `0` as `walk_hierarchy`'s `sidecar_budget: &mut usize` argument (comment: "Zero sidecar budget: same diagnostics/keystroke-path, no-blocking-IPC intent as this function's other two promote-before-read calls above"). Missed by the original doc, which listed only the first site in this function. No further sites were found in this function or elsewhere; the audit re-ran a full-file scan for `cache_backed_only`/`0usize`-as-budget/literal-`0`-into-`sidecar_budget` patterns and found no 10th. |
| resolve.rs:1245-1247 | `resolve_extension_in_scope` (called directly by `resolve_qualified`'s uppercase branch **and** as the leaf closure inside `resolve_extension_via_supertype_hierarchy`'s `walk_hierarchy_breadth_first`) |
| resolve.rs:1337-1339 | `implicit_receiver_extension_match` |
| resolve.rs:1580-1582 | `resolve_qualified` itself |
| resolve.rs:1990-1991 | `resolve_via_imports` |
| resolve.rs:2108-2109 | `resolve_same_package` |
| resolve.rs:2160-2161 | `find_symbol_in_package` (called from `resolve_star_imports`) |

The `resolve_extension_in_scope` site is the one behind the confirmed Moneta bug: the supertype
walk (`resolve_extension_via_supertype_hierarchy`) already spends its own real
`MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` budget to promote each **ancestor class's** JAR (so
it can keep walking up), but the leaf check "does *this* ancestor's JAR declare the extension" —
`resolve_extension_in_scope`'s own promotion — is zero-budget independent of that outer walk's
budget. A right ancestor can be found and still yield nothing.

### WHAT

`LatencyClass` is still the caller-facing vocabulary — every *true external* caller (the "(b)" rows
in the table above) states one explicitly, never derived from `io`. What changed after review is
what "stating it" means mechanically: it is no longer "call a shared function that decides for
itself whether it's an entry point." Instead:

- `resolve_symbol` (free fn and facade method), `resolve_qualified`, `resolve_chain`, and every
  function in the 9-site table above take a new required parameter, `sidecar_budget: &mut usize`.
  There is no default and no way to call them without one — this is what makes Problem A structurally
  unrepresentable rather than merely undocumented.
- `LatencyClass::initial_sidecar_budget()` (below) may only be called by a *true external* caller —
  a function that does not itself receive `sidecar_budget` as a parameter. Every resolve.rs-internal
  call (the four `resolve_qualified`-internal `resolve_symbol` calls, `implicit_receiver_member_match`'s
  loop, `resolve_chain`'s internal helpers) reborrows the parameter it was already given; none of them
  call `initial_sidecar_budget()`. This invariant is mechanically checkable during implementation: grep
  the diff for `initial_sidecar_budget()` call sites and confirm the count matches the number of true
  external callers, with none inside `resolve.rs`'s own re-entrant helpers.

```rust
/// Which latency budget a resolution call may spend on blocking sidecar IPC
/// (JAR-promotion round trips), independent of `ResolveIo`. `ResolveIo::Full`
/// is shared by every caller of `resolve_symbol`/`resolve_qualified`/
/// `resolve_chain` — one-shot interactive callers (goto-def, hover),
/// completion/signature-help (per-keystroke-adjacent), the per-keystroke
/// nullable-dot-call diagnostic (`resolve_member_only`), and background
/// index-completion warming — so `io` alone cannot carry this signal and must
/// never be read for this decision. `resolve_symbol` itself now REQUIRES a
/// threaded `sidecar_budget: &mut usize`, so only a true external caller
/// (never a function that itself received that parameter) may call
/// `initial_sidecar_budget()` to mint one.
#[derive(Clone, Copy)]
pub(crate) enum LatencyClass {
    /// Per-keystroke / bulk-scan: zero blocking sidecar IPC. Cache-backed
    /// promotions (an already-fresh jar-symbol-cache entry) are still free —
    /// see `promote_candidates_bounded`'s own cache-freshness check.
    Keystroke,
    /// One-shot interactive request: may spend up to
    /// `MAX_SYNC_JAR_PROMOTIONS_PER_INTERACTIVE_RESOLUTION` blocking round
    /// trips for THIS call (shared across every promotion site the call
    /// touches, not reset per site — mirrors `MAX_SYNC_JAR_PROMOTIONS_PER_COMPLETION`'s
    /// request-wide counter, not `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK`'s
    /// per-walk one).
    Interactive,
}

impl LatencyClass {
    fn initial_sidecar_budget(self) -> usize {
        match self {
            LatencyClass::Keystroke => 0,
            LatencyClass::Interactive => MAX_SYNC_JAR_PROMOTIONS_PER_INTERACTIVE_RESOLUTION,
        }
    }
}

/// Matches `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK`'s value — no
/// evidence yet that a different cap is warranted; revisit against the
/// Moneta ground-truth harness once real budget is actually granted (see
/// Non-goals — this document lands the mechanism, not the flip).
const MAX_SYNC_JAR_PROMOTIONS_PER_INTERACTIVE_RESOLUTION: usize = 3;
```

`LatencyClass` lives beside `ResolveIo` in the spine module (see Gap 3) — same visibility, same
kind of policy-enum, but a genuinely separate axis. It is **not** a new `ResolveIo` variant: only
`Full` needs this axis (the other four variants are already zero-budget by construction — `NoRg`/
`IndexOnly` forbid `rg`/hierarchy outright, and `ScopedOnly`/`HierarchyAmbiguitySafe` are used only
by `walk_hierarchy`'s own recursion, which already threads its own explicit `sidecar_budget`
parameter, precedented below), so folding it into `ResolveIo` would force the other four variants
to carry a field they never use.

### HOW

Precedent already exists in this exact codebase for "a sidecar-promotion cap becomes an explicit
parameter, not a hardcoded/derived constant": `hierarchy.rs::walk_hierarchy`'s
`sidecar_budget: &mut usize` parameter was deliberately pulled out of a hardcoded constant for
this identical reason (see `docs/superpowers/specs/2026-07-19-cst-navigation-design.md`: *"the
cap exists to protect interactive, keystroke-latency-sensitive callers... it is not a claim that 3
is universally correct"*), and `complete.rs`'s `MAX_SYNC_JAR_PROMOTIONS_PER_COMPLETION` is already
a request-wide counter (`jar_promotion_attempts`) shared across every candidate in one completion
request rather than reset per candidate. This document applies the same two ideas —
caller-stated budget, request-wide counter — to `resolve_qualified`/`resolve_chain`.

1. Widen `resolve_symbol` (free fn **and** the `Indexer::resolve_symbol` facade method),
   `resolve_qualified`, and `resolve_chain` to take `sidecar_budget: &mut usize` (plain `usize`,
   matching `jar.rs`'s existing parameter shape exactly — no new type crosses into `jar.rs`; that
   file's signatures do not change). Every one of the 9 sites in the table above replaces its local
   `let mut cache_backed_only = 0usize;` (or literal `0` argument) with the threaded parameter (a
   reborrow at each nested call, including the four `resolve_qualified`-internal `resolve_symbol`
   calls and `implicit_receiver_member_match`'s loop — Problem A's fix). This is signature-only and
   behavior-preserving for every *existing* caller once step 2 below has each of them still pass 0
   — mirrors the walk_hierarchy 6b-hardening plan's own "every existing caller passes the same
   value, reproducing today's cap exactly" migration shape. Because `resolve_symbol` itself now
   takes the parameter, this step's blast radius is every caller in the Problem B table, not just
   the resolve.rs-internal ones — a materially larger, but mechanical, signature change.
2. Every *true external* caller — the "(b)" rows in the Problem B table, i.e. every place that is
   not itself receiving `sidecar_budget` from another function in this graph — materializes its own
   fresh `let mut sidecar_budget = latency_class.initial_sidecar_budget();` once per request, and
   passes `&mut sidecar_budget` down:
   - `resolve_member_only` passes `LatencyClass::Keystroke` explicitly — **zero behavior change**,
     since every site was already hardcoded to 0.
   - `resolve_symbol`'s own external callers (`features/definition.rs`, `features/hover.rs`, via
     `resolve_callee_definition`) pass `LatencyClass::Interactive` — this is the line that, combined
     with the nonzero constant, is what actually closes the Moneta gap. Per Non-goals, landing this
     document's mechanism does not require flipping these call sites in the same change, but the
     mechanism is what makes flipping them *safe* to do next.
   - `resolve_symbol_index_only`, `resolve_symbol_no_rg`, `resolve_symbol_scoped_only`,
     `resolve_symbol_hierarchy_ambiguity_safe`, `resolve_type_index_only` all pass
     `LatencyClass::Keystroke` unconditionally — consistent with what their own `ResolveIo`
     variants already forbid (rg/hierarchy/cold-index), so there is no case where they would ever
     want a nonzero budget.
   - `indexer/lookup.rs`'s `find_definition`/`find_definition_qualified_with_io` and
     `indexer/resolution.rs`'s `resolve_locations`: classification not settled by this document —
     both sit on paths that *look* interactive (goto-definition) but weren't traced back to their
     own ultimate callers in this pass. Defaulting them to `LatencyClass::Keystroke` (today's
     behavior, zero risk) is the safe migration-step-2 choice; re-classifying any of them to
     `Interactive` is deferred alongside the `resolve_symbol`/`resolve_callee_definition` flip in
     step 3, and needs its own caller trace first.
   - `indexer/apply.rs`'s background completion-cache warming (line 1249-1250): also defaults to
     `LatencyClass::Keystroke`. It is not literally per-keystroke, but it is not a synchronous
     interactive request either — see Open questions for whether a third `LatencyClass` variant is
     warranted for this shape. `Keystroke` is the conservative, zero-behavior-change default either
     way.
   - `find_fun_signature_with_receiver`'s callers (`features/completion.rs`,
     `features/completion_context.rs`, `features/signature_help.rs`, `features/traits_impl.rs`):
     also default to `LatencyClass::Keystroke` — completion and signature-help both fire on
     effectively every keystroke inside a call's parens, so they belong with the diagnostics path,
     not the goto-def/hover one, until proven otherwise.
3. `resolve_qualified`'s own internal re-entrant calls to `resolve_symbol` (four sites) and
   `implicit_receiver_member_match`'s loop reborrow the parameter they were already given — they
   are function parameters now, not local variables, so there is nothing to "materialize" at these
   sites; this is what makes Problem A impossible to reintroduce by accident. One budget counter per
   request, not per call site: initialized once at the true external caller, threaded by `&mut usize`
   through every nested call for that one request. This avoids the multiplicative risk of re-granting
   a fresh budget at each of the 9 sites independently, which would let one request spend up to 9×
   the intended cap.

## Gap 2: type-safe member/extension precedence

### WHY

`with_supertype_extension_fallback` (resolve.rs:2295-2311):

```rust
fn with_supertype_extension_fallback(...) -> Vec<Location> {
    let supertype_ext_locs = resolve_extension_via_supertype_hierarchy(...);
    if supertype_ext_locs.is_empty() {
        return member_locs;
    }
    let mut combined = member_locs;
    combined.extend(supertype_ext_locs);
    combined
}
```

Kotlin's own overload-resolution rule — a real member (declared or inherited) always outranks a
same-named extension, independent of discovery order — is encoded here only by which `.extend()`
call happens second, backed by a doc comment. AGENTS.md is explicit that "which order branches
run" is exactly the class of fact that should be a type, not a comment: nothing here stops a
future edit from swapping the two lines, and nothing in the return type (`Vec<Location>`) tells a
caller that the list is secretly two ranked tiers rather than an unordered set of candidates.

**Revised after review — this struct only covers 2 of 3 real precedence tiers in
`resolve_qualified`, and both gaps are confirmed real, not just theoretical:**

1. **A third, earlier tier: `resolve_from_class_hierarchy_scoped` (resolve.rs:1553-1562).** Inside
   `resolve_qualified`'s uppercase-root branch, when the anchor class's own body doesn't declare
   `name`, the code first tries `resolve_from_class_hierarchy_scoped` (inherited member from a
   superclass) and returns immediately if that finds anything — *before* ever calling
   `resolve_extension_via_supertype_hierarchy` (resolve.rs:1566-1575) for the ancestor-extension
   fallback. That's Kotlin's same "member beats extension" rule, restated one level up the
   hierarchy, and it is *still* encoded only by which `if !....is_empty() { return ... }` runs
   first — `MemberExtensionCandidates` doesn't see this branch at all, because it never reaches
   `with_supertype_extension_fallback`. This document does not extend the struct to cover it: doing
   so would mean threading `MemberExtensionCandidates` through a different code shape (early-return
   pairs, not a single combine-then-return), a larger change than this pass's scope. Scoped out
   explicitly, not silently dropped: a future pass should either fold this branch into the same
   type or accept that inherited-member-vs-ancestor-extension precedence stays a bare early-return
   pair for now.
2. **resolve.rs:1566-1575 bypasses the struct even for the two tiers it does model.** When
   `resolve_from_class_hierarchy_scoped` finds nothing, this branch calls
   `resolve_extension_via_supertype_hierarchy` directly and returns its raw `Vec<Location>` —
   never constructing `MemberExtensionCandidates` at all. This is not a bug: `member` is provably
   empty on this path (the hierarchy check immediately above already returned if it weren't), so
   "extension-only" is exactly what the struct's own semantics would produce here too. It's flagged
   because a reader diffing this file for "does the codebase actually use the new type
   consistently" would find a third call site that computes the identical two-tier precedence by
   hand instead of through the type — left as bare `Vec<Location>` deliberately, since routing an
   already-known-empty `member` field through the struct only to immediately unwrap the extension
   side back out would add a construction with no informational content.

### WHAT

Not a `Resolution<T>`-style outcome enum (the CST doc's own pattern for this class of problem):
member-empty/extension-nonempty, member-nonempty/extension-empty, and both-nonempty are all real,
simultaneously-possible states here, not three mutually exclusive outcomes — forcing them into a
sum type would misrepresent the actual domain shape the CST doc's own "illegal states
unrepresentable" rule warns against doing wrong. The honest model is a **struct** with two named,
separately-documented fields, collapsed to a `Vec<Location>` in exactly one place:

```rust
/// The two-tier candidate set for a receiver-qualified name lookup, ranked
/// by Kotlin's own member-over-extension precedence: a real member always
/// outranks a same-named extension. Replaces a flattened `Vec<Location>`
/// built by `.extend()`-ing the extension tier onto the member tier, where
/// the precedence lived only in call order and a comment (AGENTS.md:
/// "which order branches run" should be a type).
///
/// Does not model the earlier inherited-member-vs-ancestor-extension tier at
/// resolve.rs:1553-1575 (`resolve_from_class_hierarchy_scoped` /
/// `resolve_extension_via_supertype_hierarchy`'s direct-return branch) —
/// see Gap 2 WHY, item 1. Scoped out of this pass deliberately.
struct MemberExtensionCandidates {
    /// Declared-in-class or inherited members (`find_all_names_scoped_to_container`,
    /// `find_name_in_uri`). Always ranked ahead of `extension_fallback`.
    member: Vec<Location>,
    /// The supertype-walk extension fallback
    /// (`resolve_extension_via_supertype_hierarchy`). `Option`, not `Vec` —
    /// `resolve_extension_via_supertype_hierarchy` already collapses to at
    /// most one match before returning (resolve.rs:2372:
    /// `matches.into_iter().next().into_iter().collect()`, with a comment
    /// explaining a same-level sibling tie is deliberately resolved to
    /// "take just the first" rather than surfaced as ambiguous) — a `Vec`
    /// here would misrepresent settled single-location behavior as still
    /// ambiguous. Never outranks a present member — Kotlin's own rule, an
    /// extension can never shadow a member of the same name.
    extension_fallback: Option<Location>,
}

impl MemberExtensionCandidates {
    /// Every current call site's behavior: concatenate member-then-extension
    /// into the flat `Vec<Location>` `resolve_qualified`'s return type still
    /// needs. The ONE seam where the ranked type collapses back into an
    /// unranked list — a future shape-aware caller (arity filtering) should
    /// match `member`/`extension_fallback` directly instead of re-deriving
    /// the tier boundary from a flattened Vec's order.
    fn into_precedence_ordered(self) -> Vec<Location> {
        let mut locations = self.member;
        locations.extend(self.extension_fallback);
        locations
    }
}
```

**No real consumer today — landing as internal plumbing, not a speculative public type.** Checked
`features/definition.rs`'s shape-filter code (the obvious candidate, since it already reasons about
member-vs-extension arity): it calls `shape_filter_locations(index, shape, locs)` on the *already
flattened* `Vec<Location>` `find_definition_qualified` returns — it has no visibility into which
locations came from `member` vs `extension_fallback`, and doesn't need it today, because it filters
by call-shape (arity) uniformly across the flattened list rather than by tier. So
`MemberExtensionCandidates` has zero consumers that read its two fields separately as of this
document; its only current job is documentation-as-a-type for `with_supertype_extension_fallback`'s
own internal ordering; `into_precedence_ordered()` is the only thing anything calls. That's an
acceptable reason to land it (it still kills the "swap the two `.extend()` lines by accident" risk,
which was Gap 2's actual motivating bug class), but it should land honestly scoped that way rather
than implying a shape-aware caller is coming in this same change — none is designed here.

### HOW

`with_supertype_extension_fallback` returns `MemberExtensionCandidates` instead of `Vec<Location>`,
and its own body changes `extension_fallback` from `resolve_extension_via_supertype_hierarchy(...)`
(a `Vec<Location>`) to `.into_iter().next()` on that same call — collapsing at the point of
construction instead of relying on the callee's own already-collapsing return shape, so the
`Option` is honest even if `resolve_extension_via_supertype_hierarchy`'s own collapsing behavior
ever changes independently.

Its two call sites are **not symmetric — corrected from the original doc, which claimed both
"currently do nothing but `return with_supertype_extension_fallback(...)` immediately."** That's
true of resolve.rs:1536 (a bare `return with_supertype_extension_fallback(...);`), but
resolve.rs:1679 is the `Ok` arm of a `match Url::parse(resolved_uri)`, whose `Err(_)` arm returns
`locs: Vec<Location>` directly — a different value with a different type once
`with_supertype_extension_fallback` stops returning `Vec<Location>`. Both arms must still produce
the same type for the `match` to compile, so this call site becomes:

```rust
match Url::parse(resolved_uri) {
    Ok(parsed_uri) => {
        with_supertype_extension_fallback(indexer, locs, &current_type_base, &parsed_uri, name, from_uri)
            .into_precedence_ordered()
    }
    Err(_) => locs,
}
```

— i.e. `.into_precedence_ordered()` goes on the `Ok` arm specifically, not appended generically
after the whole `match` (there is no `with_supertype_extension_fallback(...)` call outside the `Ok`
arm to append it to). Once that's in place both arms are `Vec<Location>` again and the `match`
type-checks exactly as before; this is still zero behavior change, just a more precise description
than "pure signature change" — the type unification is real work at this specific call site, small
but not nothing. Both sites get a decoy regression test (member-present-and-arity-compatible case
must still return the member location first). This is Category (A) in the qualified-resolution
doc's own migration taxonomy: "refactor-onto-primitive... no intended output change." It also
directly sets up (without doing the work of) a future shape-aware caller — one that tries `member`
first, checks arity, and only reaches for `extension_fallback` on a miss — without that caller
having to linear-scan a flattened `Vec` and guess where the tier boundary was.

## Gap 3: file split — module boundaries

### Principle

`resolve.rs`'s own module doc comment already gives an authoritative six-step taxonomy (local →
imports → same-package → star-imports → extension functions → project-wide `rg`). The split
organizes around that existing taxonomy rather than inventing a new one, plus the cross-cutting
helper families the qualified-resolution doc's reuse inventory already names. `resolver/mod.rs`
already treats `hierarchy.rs`, `find.rs`, `import_edit.rs`, `complete.rs` as flat siblings of
`resolve.rs` — the split adds more siblings at that same flat level rather than nesting a new
directory under `resolve.rs`, matching the existing convention exactly.

Borrowed from the CST doc's structural rule, adapted for the string domain: **`resolve.rs`
becomes the spine, with zero unrelated logic** — every helper that isn't part of the top-level
dispatch chain moves to a named sibling file whose name states its one responsibility, the same
discipline the CST doc applies to `mod.rs`-as-catalogue.

### Proposed modules

| File | Contents | Rationale |
|---|---|---|
| `resolver/resolve.rs` (kept, ~350 lines) | `ResolveIo`, `LatencyClass` (new), `resolve_symbol`, `resolve_symbol_index_only`, `resolve_symbol_with_io`, `resolve_symbol_no_rg`, `resolve_symbol_hierarchy_ambiguity_safe`, `resolve_symbol_scoped_only`, `resolve_type_index_only`, `resolve_type_index_only_simple`, `resolve_callee_definition`, `resolve_chain`, `resolve_local`, `resolve_in_scope_strict`, `rg_location_satisfies_call_shape`, `ensure_file_data`, `fqns_for_name`, the `impl Indexer` facade block | The spine: every public entry point plus the two orthogonal policy enums. `resolve_local` stays here (not with `resolve_qualified`) — confirmed via its two call sites (resolve.rs:278 inside `resolve_chain`, resolve.rs:1081 inside `resolve_in_scope_strict`), both spine-internal, never called from `resolve_qualified`. |
| `resolver/qualified.rs` (new, ~300 lines) | `resolve_qualified`, `MemberExtensionCandidates` (new), `with_supertype_extension_fallback`, `resolve_extension_via_supertype_hierarchy`, `resolve_from_class_hierarchy`, `resolve_from_class_hierarchy_scoped` | Step 0 (dot-qualified access) plus its tightly-coupled `this`/`super`/uppercase-qualifier hierarchy strategy and the gap-2 type. |
| `resolver/extension.rs` (new, ~250 lines) | `resolve_extension_in_scope`, `receiver_provides_member`, `resolve_implicit_receiver_callee`, `implicit_receiver_extension_match`, `implicit_receiver_member_match` | Step 5 (extension functions) plus implicit-receiver-callee resolution — both answer "does this receiver provide this name via an extension/member without an explicit qualifier." |
| `resolver/imports.rs` (new, ~300 lines) | `resolve_via_imports`, `has_explicit_import`, `is_default_import_package`, `is_default_import_type`, `resolvable_via_default_import`, `KOTLIN_DEFAULT_IMPORT_PACKAGES` | Step 2 (explicit imports), including the default-import special case. |
| `resolver/package_scope.rs` (new, ~200 lines) | `resolve_same_package`, `symbols_in_package`, `find_symbol_in_package`, `resolve_star_imports`, `find_in_star_imports` | Steps 3+4 (same-package, star-imports) — already share the "scan a package's indexed files" shape and call into each other (`find_symbol_in_package`). |
| `resolver/tie_break.rs` (new, ~250 lines) | `ambiguity_safe_tail_with_denylist`, `default_kotlin_import_tie_break`, `module_scoped_tie_break`, `ModuleScopedOutcome`, `import_package_tie_break`, `owning_module_dependencies`, `candidate_gradle_meta`, `is_denylisted_package_prefix`, `DENYLISTED_PACKAGE_PREFIXES` | Step 6's global-defs-tail disambiguation cluster — already a cohesive unit named by prior PR work (`import_package_tie_break` is PR #310's own fix name per project memory). |
| `resolver/package.rs` (new, ~120 lines) | `jar_symbol_package`, `location_package`, `import_package_absent_from_source_roots`, `package_dir_in_source_roots`, `rg_in_package_dir` | Package-membership + rg-scoping primitives shared across imports/same-package/star/tie-break (confirmed: `jar_symbol_package` alone is called from four different strategy files). |
| `resolver/container.rs` (new, ~150 lines) | `pos_tuple`, `range_encloses`, `enclosing_container_chain`, `import_container_chain`, `resolve_companion_member` | Range-containment primitives. `range_encloses`/`resolve_companion_member` are the exact precedent the qualified-resolution doc's Primitive 1 cites for `find_name_scoped_to_container` ("`resolve_companion_member` already re-fetches it this way") — this groups them explicitly beside that reuse, rather than leaving them scattered through the spine. |
| `resolver/platform_types.rs` (new, ~150 lines) | `is_stdlib`, `resolve_kotlin_builtin_type_platform_equivalent`, `KOTLIN_BUILTIN_TYPE_PLATFORM_EQUIVALENTS` | Self-contained static-data mapping; called by the chain, never calls back into it. |

Net: `resolve.rs` shrinks from 2673 to roughly 350 lines; eight new siblings average 150-300 lines
each, in the same size band as the existing `hierarchy.rs`/`find.rs`. `resolve_qualified` remains
the single largest function (~275 lines) even after the split — this split fixes *file* size and
gives each concern a name; it does not (and does not need to) shrink `resolve_qualified`'s own
body, which is Gap 2's job, not Gap 3's.

**Verification note (open, not guessed):** the eleven groupings above were built by directly
tracing call sites for `resolve_local`, `rg_location_satisfies_call_shape`, `jar_symbol_package`,
`enclosing_container_chain`/`import_container_chain`/`range_encloses`/`resolve_companion_member`
via `search_for_pattern`, and are held with high confidence. A handful of smaller single-caller
helpers not individually re-verified here (e.g. `pos_tuple`) are assigned by co-location with
their one obvious caller; per AGENTS.md's own mandate, run `find_referencing_symbols` on each
function immediately before moving it, during implementation — not a re-guess, a final check.

### Mutual dependency between `resolve.rs` and `qualified.rs`

`resolve_symbol_with_io` (spine) calls `resolve_qualified` (qualified.rs) for the dot-qualified
case; `resolve_qualified`'s uppercase branch calls `resolve_symbol`/`resolve_symbol_index_only`
(spine) to resolve the qualifier root itself. This is a genuine two-way call relationship between
the two files, not a layering violation — it already exists today within one file, and
`hierarchy.rs`/`find.rs` already call each other as siblings under `resolver/`.

### Visibility audit — revised; "no `pub(crate)` boundary needs to change" was wrong

The original doc's claim above (no visibility changes needed beyond `mod.rs`'s existing re-exports)
was checked against the actual `fn` signatures in `resolve.rs` and is false: every function moving
out of `resolve.rs` that is currently a bare, module-private `fn` and is called from a *different*
proposed module needs `pub(super)` (all siblings hang directly off `resolver/`, so `pub(super)` —
visible to `resolver/mod.rs` and everything under it — is the right level, not `pub(crate)`).
Since these functions are private today, Rust's own privacy rules guarantee every one of their
current call sites is inside `resolve.rs` — so auditing "does this function's caller live in a
different proposed module" is a closed, checkable question, not a guess. Methodology: for each
private `fn` in the Gap-3 table, `search_for_pattern` on its call sites within `resolve.rs`, then
check which proposed module the enclosing function of each call site belongs to. Confirmed
cross-module private functions (8, not the "0" the original doc implied):

| Function (current file) | Currently | Moves to | Called from (different module) | Needs |
|---|---|---|---|---|
| `resolve_qualified` (resolve.rs:1443) | `fn` (private) | `qualified.rs` | `resolve_symbol_with_io` (resolve.rs:162, spine) **and** the `impl Indexer::resolve_qualified` facade (resolve.rs:2670, spine) | `pub(super)` |
| `resolve_extension_in_scope` (resolve.rs:1236) | `fn` (private) | `extension.rs` | `resolve_qualified` (qualified.rs:1472, 1708) and `resolve_extension_via_supertype_hierarchy` (qualified.rs:2364, as a closure) | `pub(super)` |
| `resolve_companion_member` (resolve.rs:1846) | `fn` (private) | `container.rs` | `resolve_qualified` (qualified.rs:1499) | `pub(super)` |
| `find_in_star_imports` (resolve.rs:847) | `fn` (private) | `package_scope.rs` | `resolve_via_imports` (imports.rs) and `resolve_in_scope_strict` (resolve.rs:1142, spine) | `pub(super)` |
| `location_package` (resolve.rs:830) | `fn` (private) | `package.rs` | `module_scoped_tie_break`, `import_package_tie_break`, `is_denylisted_package_prefix` (all `tie_break.rs`) and `resolve_same_package` (`package_scope.rs`) | `pub(super)` |
| `import_package_absent_from_source_roots` (resolve.rs:2469) | `fn` (private) | `package.rs` | `resolve_via_imports` (`imports.rs`) | `pub(super)` |
| `package_dir_in_source_roots` (resolve.rs:2440) | `fn` (private) | `package.rs` | `resolve_via_imports` (`imports.rs`) — same-file caller `import_package_absent_from_source_roots` stays fine too once both are `pub(super)` | `pub(super)` |
| `rg_in_package_dir` (resolve.rs:2379) | `fn` (private) | `package.rs` | `resolve_star_imports` (`package_scope.rs`) | `pub(super)` |

One named example from the review turned out **not** to need a change: `range_encloses`
(resolve.rs:1834) is already `pub(crate)`, so moving it to `container.rs` needs no visibility edit
— confirmed by direct signature read, not assumed. `is_denylisted_package_prefix`,
`owning_module_dependencies`, and `candidate_gradle_meta` were also checked and stay private —
their only call sites are same-file (`tie_break.rs`) after the split.

This audit covers the functions with confirmed cross-module call sites; it is not a symbol-by-symbol
pass over every one of the ~58 top-level items (that remains the per-item `find_referencing_symbols`
check the Migration section already calls for during implementation). **Real estimate: 8 functions
need `fn` → `pub(super)` widening**, concentrated exactly at the module seams the table already
describes as having cross-file relationships (`qualified.rs` ↔ `extension.rs`, `qualified.rs` ↔
`container.rs`, `qualified.rs` ↔ spine, `package.rs` ↔ {`imports.rs`, `package_scope.rs`,
`tie_break.rs`}) — not zero, and not scattered randomly, which is itself a useful sanity check that
the module boundaries in the table are drawn in sensible places.

## Testing artifacts — `src/resolver/tests.rs` (9219 lines)

**Not mentioned anywhere in the original doc.** `tests.rs` is declared in `mod.rs` as
`#[cfg(test)] mod tests;` — a *sibling* of `resolve.rs` under `resolver/`, not a submodule nested
inside it — and its own top of file (`src/resolver/tests.rs:1-2`) does:

```rust
use super::shared_fixture_tests::gradle_cache_jar_uri;
use super::*;
```

`use super::*` glob-imports everything `resolver/mod.rs` itself re-exports at its top level —
`tests.rs` never reaches into `resolve.rs` (or any other sibling) directly; every function or
constant it calls by bare name must already be one of `mod.rs`'s re-exports (either the
unconditional `pub(crate) use resolve::{...}` block, or one of the `#[cfg(test)]`-gated ones).
Confirmed by direct read of `mod.rs`, three of those `#[cfg(test)]`-only re-exports point at
functions this split moves out of `resolve.rs`:

```rust
#[cfg(test)]
pub(crate) use resolve::resolve_kotlin_builtin_type_platform_equivalent;  // → moves to platform_types.rs
#[cfg(test)]
pub(crate) use resolve::resolve_symbol;                                   // stays in resolve.rs (spine)
#[cfg(test)]
pub(crate) use resolve::resolve_symbol_index_only;                        // stays in resolve.rs (spine)
```

Only the first of these three actually needs an edit (`resolve::` → `platform_types::` in that one
`use` line) — `resolve_symbol`/`resolve_symbol_index_only` are unaffected since both stay in the
spine file per the Gap-3 table. But the *general* consequence is broader than that one line: because
`tests.rs` sees the resolver module's surface exclusively through `mod.rs`'s flat re-export block,
**every already-`pub(crate)` item that the split relocates to a new sibling file needs its `mod.rs`
`use` path updated to point at the new file** (mechanical, one line per moved re-export — the
non-test `pub(crate) use resolve::{ensure_file_data, find_symbol_in_package, fqns_for_name,
receiver_provides_member, resolve_callee_definition, resolve_implicit_receiver_callee,
resolve_in_scope_strict, resolve_symbol_hierarchy_ambiguity_safe, resolve_symbol_no_rg,
resolve_symbol_scoped_only};` block at mod.rs:33-37 needs the same treatment for whichever of those
names move — `find_symbol_in_package` moves to `package_scope.rs`, `receiver_provides_member` and
`resolve_implicit_receiver_callee` move to `extension.rs`, per the Gap-3 table). This is on top of
the 8 `pub(super)` widenings above, which are a *separate* concern (private → cross-module-visible)
from these re-export path fixes (already-`pub(crate)` → re-exported from a different path).

**Churn estimate:** roughly a dozen `mod.rs` line edits (3 already-cfg(test) re-export paths + ~9
non-test re-export paths, based on the Gap-3 table's own module assignments for the currently
`pub(crate)` items) — mechanical, caught immediately by `cargo test --bin kmp-lsp` failing to
compile if missed, not a silent runtime gap. **Tests themselves do not move** — per Migration step
4 ("move-don't-rewrite"), `tests.rs` stays one 9219-line file, unsplit, testing the resolver
module's behavior as a whole rather than being partitioned to mirror the new file boundaries;
splitting it (e.g. into `qualified_tests.rs`, `extension_tests.rs`) is a legitimate follow-up this
document does not attempt, consistent with Gap 3's own "moves, not rewrites" discipline. The
`#[cfg(test)]`-only functions re-exported through `mod.rs` (the three listed above) are themselves
evidence that `tests.rs` already treats the whole resolver module as one flat test surface rather
than mirroring file boundaries — splitting the file split's own siblings is not a natural
consequence of the module split and shouldn't be read as implied by it.

## Relation to existing designs (consistency check)

- **`unified-resolution-handler.md`**: `ResolveIo` is untouched in shape and scope — still exactly
  the IO/subprocess policy it was designed as. `LatencyClass` is deliberately a new, separate enum
  rather than a `ResolveIo` variant, precisely because this document's job is to stop conflating
  the two axes the way `Full` currently does.
- **CST resolution unification doc**: this document stays entirely inside the string domain
  (`resolver/`); it touches nothing under `indexer/infer/`. It reuses that doc's structural rules
  (mod.rs/spine-as-catalogue-with-zero-logic, type-driven correctness, construction discipline)
  as stated repo-wide conventions (AGENTS.md states them project-wide, not CST-specific) — it does
  not reuse `CstQuery`, `Resolution<T>`, or any CST-domain type. Gap 2 explicitly diverges from
  that doc's `Resolution<T>`-enum pattern in favor of a struct, with the reasoning spelled out
  above (the domain shape here is two independent optional lists, not an exclusive outcome).
- **Qualified-resolution unification doc**: that doc's Primitive 1 (`find_name_scoped_to_container`,
  `range_encloses`, `resolve_companion_member` in `resolver/find.rs`) is the direct precedent for
  this document's `container.rs` grouping — not a coincidence; `container.rs` groups exactly the
  primitives that doc's own reuse-inventory table already names as reused, adjacent code. This
  document does not touch that doc's Primitive 2 (CST-domain `Url`-threading through
  `InferDeps::find_field_type`) at all — different domain, different file, out of scope here.

## Reuse inventory (existing types/patterns — do not reinvent)

| Type / pattern the design needs | Status | Location | Decision |
|---|---|---|---|
| `sidecar_budget: &mut usize` parameter shape | exists | `hierarchy.rs::walk_hierarchy`, `jar.rs::promote_candidates_bounded` | Reuse the exact shape for `LatencyClass`'s underlying counter — no new numeric type crosses into `jar.rs` |
| Request-wide promotion counter | exists | `complete.rs::MAX_SYNC_JAR_PROMOTIONS_PER_COMPLETION` / `jar_promotion_attempts` | Precedent for Gap 1's "one counter per request, not per site" rule |
| `MAX_SYNC_JAR_PROMOTIONS_PER_*` naming convention | exists | `hierarchy.rs`, `complete.rs` | `MAX_SYNC_JAR_PROMOTIONS_PER_INTERACTIVE_RESOLUTION` follows it |
| `range_encloses` / `resolve_companion_member` | exists | `resolve.rs:1834`, `resolve.rs:1846` | Move into `container.rs`, unchanged — already the qualified-resolution doc's own reuse precedent |
| `find_name_scoped_to_container` | exists | `resolver/find.rs` | Not moved — stays in `find.rs`, which `qualified.rs`/`resolve.rs` continue to call via `super::find::` |
| `ResolveIo` | exists | `resolve.rs:83` | Unchanged; stays in the new spine file |
| `Resolution<T>` outcome-enum pattern | exists (CST domain) | CST unification doc | Explicitly **not** reused for Gap 2 — see Relation to existing designs |

**Naming collision — disambiguating `resolve_qualified` (Minor point 9).** There are two entirely
unrelated functions named `resolve_qualified` in this codebase:

1. `resolver::resolve::resolve_qualified` (resolve.rs:1443) — this document's Gap 1/Gap 2/Gap 3
   subject, moving to `resolver/qualified.rs`.
2. `indexer::infer::sig::resolve_qualified` (sig.rs:990) — a private, file-local homonym in the
   call-argument-diagnostics signature-lookup module, called only from `sig.rs:1254` inside
   `resolve_call_signature`, itself `call_arg_diagnostics.rs`'s per-keystroke entry point. It reads
   `idx.definitions`/`idx.jar_definitions` directly and never calls `resolver::resolve::resolve_symbol`
   or this document's `resolve_qualified` at all — a fully independent implementation with its own,
   separate zero-budget JAR-promotion sites (`cache_backed_only` at sig.rs:1011, sig.rs:1051,
   sig.rs:1163), untouched by this document's Gap 1 mechanism.

This document's every reference to "`resolve_qualified`" means (1) only. A future
`find_referencing_symbols("resolve_qualified")` will surface both — check the file path before
trusting the result. `call_arg_diagnostics.rs` itself remains a real, separate per-keystroke
diagnostic surface (correctly a concern for latency-sensitive JAR promotion in general) — it is
just not part of *this* document's call graph, since it never reaches `resolver::resolve_symbol`.
Whether sig.rs's own resolve_qualified/resolve_unqualified deserve the same `LatencyClass`-style
treatment is a separate, un-scoped follow-up.

Net: **one new enum** (`LatencyClass`), **one new struct** (`MemberExtensionCandidates`), **one
new constant** (`MAX_SYNC_JAR_PROMOTIONS_PER_INTERACTIVE_RESOLUTION`), **eight new files** (moves,
not new logic), **zero changes to `jar.rs`'s public signatures**, **8 `pub(super)` visibility
widenings** (see Visibility audit above — not zero, corrected from the original doc), **~12
`mod.rs` re-export path edits** (see Testing artifacts above).

## Migration (incremental, mirrors the qualified-resolution doc's own step style)

1. **(A, behavior-preserving) Land `MemberExtensionCandidates`.** Change
   `with_supertype_extension_fallback`'s return type; update its two call sites to call
   `.into_precedence_ordered()`. Decoy regression test: member present + arity-compatible still
   wins; member present + arity-incompatible + extension present still surfaces the extension
   (today's existing shape-aware-caller behavior, unchanged).
2. **(A, behavior-preserving) Land `LatencyClass` + thread `sidecar_budget`.** Widen
   `resolve_symbol` (free fn and facade method), `resolve_qualified`, `resolve_chain`, and all 9
   downstream sites; every *true external* caller — the full Problem-B table in Gap 1, not just the
   resolve.rs-internal ones — passes `LatencyClass::Keystroke` (reproducing today's hardcoded-zero
   exactly), and every resolve.rs-internal re-entrant call (the four `resolve_qualified`-internal
   `resolve_symbol` calls, `implicit_receiver_member_match`'s loop) reborrows instead of
   materializing. Green on `cargo test --bin kmp-lsp` with zero assertion changes — this step alone
   changes no behavior. This step's diff touches `indexer/lookup.rs`, `indexer/resolution.rs`,
   `indexer/apply.rs`, and the `find_fun_signature_with_receiver` call sites
   (`features/completion.rs`, `features/completion_context.rs`, `features/signature_help.rs`,
   `features/traits_impl.rs`) in addition to `resolve.rs` itself — a materially wider diff than the
   original doc's "8 sites in one file," worth calling out to reviewers up front rather than
   discovering mid-review.
3. **(B, migration-is-the-fix, deferred per Non-goals) Flip `resolve_symbol`'s true interactive
   callers (`features/definition.rs`, `features/hover.rs`, via `resolve_callee_definition`) to
   `LatencyClass::Interactive`.** RED-then-GREEN: a decoy test in the Moneta
   `navController.navigate`-shape (an extension only reachable via a never-materialized ancestor
   JAR) should fail before this step and pass after. Not required to land alongside steps 1-2, but
   unlocked by them. Re-classifying `indexer/lookup.rs`/`indexer/resolution.rs`'s callers (left at
   `Keystroke` in step 2 pending their own caller trace — see Gap 1 WHY) is a separate follow-up,
   not bundled into this step.
4. **(mechanical, file moves only) Execute the Gap-3 split**, one sibling file per commit,
   move-don't-rewrite, `cargo test --bin kmp-lsp` green after each move. Run
   `find_referencing_symbols` per moved item first (per AGENTS.md), not just per file, to catch
   any single-caller helper this document mis-assigned in the table above. Each move also needs:
   (a) the `pub(super)` widening from the Visibility audit if the item is one of the 8, and (b) the
   corresponding `mod.rs` re-export path fix if the item is one of the ~12 from Testing artifacts.

## Testing & verification

- `cargo test --bin kmp-lsp` (binary-only crate — `--lib` runs 0 tests) green after every step.
  Focused loops while iterating: `-- resolver`, `-- indexer_tests`, `-- nullable_call`.
- Step 2's decoy: assert `resolve_member_only` still spends zero blocking sidecar round trips —
  a counter/mock on `promote_candidates_bounded`'s sidecar-lock path, or (simpler) a fixture with
  a cold, never-materialized JAR and an assertion that resolution against it from
  `resolve_member_only` returns empty rather than blocking.
- Step 3's decoy is the actual Moneta-shaped ground-truth check this document exists to unblock —
  run against the real corpus per the resolution-accuracy benchmark's own harness, not only a
  hand-written fixture (per project memory: `resolve-accuracy-benchmark` findings have repeatedly
  only shown up at real-corpus scale).
- Gap-3 split: no behavior change is intended at any step; the full existing `resolver`/`indexer`
  test suite is the net, same as the CST doc's own "existing suites are the behaviour net" rule.

## Risks

- **Threading `sidecar_budget` through `resolve_symbol` itself touches a wider call graph than
  `resolve_chain`/`resolve_qualified` alone** — revised after review: not just
  imports/same-package/star-imports/receiver-provides-member/default-import on the spine, but every
  external caller in the Gap 1 Problem-B table (`indexer/lookup.rs`, `indexer/resolution.rs`,
  `indexer/apply.rs`'s background indexing, and `find_fun_signature_with_receiver`'s four callers).
  Mitigation: it is still a pure signature-widening change, mechanically identical at all 9 sites,
  and every *existing* caller's behavior is unchanged by construction (Migration step 2) — the same
  risk profile the walk_hierarchy 6b-hardening plan already accepted for a structurally identical
  change. What is **not** true, and the original doc's mitigation overclaimed: that
  `resolve_symbol`/`resolve_callee_definition` are "the only two `Full`-driving spine entry points."
  They are the only two *interactive-navigation* callers classified `Interactive` by this document
  (step 3); the wider caller set in step 2 is real and left at `Keystroke` deliberately, not because
  it was proven safe to ignore. `indexer/apply.rs`'s background completion-cache warming in
  particular is a traffic shape (throttled background work, not per-keystroke and not a blocking
  user request) this document's two-class model doesn't cleanly fit — flagged in Open questions,
  not resolved here.
- **`resolve_qualified` at ~275 lines remains the largest single function post-split.** Gap 3
  intentionally does not attempt to shrink it further — doing so safely would mean redesigning its
  internal branching (a larger, riskier change than a file-boundary move), and its size is a
  single-function-complexity problem, not a file-navigability one. Flagged, not fixed, here.
- **A phased landing (steps 1-2 without step 3) leaves the confirmed Moneta bug still open** after
  this document's own changes land. Accepted per Non-goals — the mechanism and the flip are
  separable, and shipping the mechanism first, behavior-preserving and fully tested, is lower risk
  than shipping both in one change.

## Open questions

- **Is `3` the right `MAX_SYNC_JAR_PROMOTIONS_PER_INTERACTIVE_RESOLUTION`?** Reused from
  `MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK` for consistency, but that constant was tuned for a
  hierarchy walk's per-ancestor promotion pattern, not `resolve_qualified`'s. Should be revisited
  against real latency measurement once Migration step 3 actually grants it.
- **Should `resolve_symbol_index_only`'s bulk-scan callers ever want a nonzero, bulk-appropriate
  budget** (distinct from both `Keystroke` and `Interactive`)? Today it is folded into `Keystroke`
  (zero), matching its current zero-budget behavior exactly, but the resolution-accuracy
  benchmark's own doc comment already flags this path as "expects most bare/local references to
  miss" for *volume*, not latency-per-call, reasons — a third `LatencyClass::BulkScan` variant with
  a small nonzero, throughput-tuned budget might independently improve benchmark recall. Not
  designed here; flagged because it's a natural extension of the same mechanism.
- **What `LatencyClass` should `indexer/apply.rs`'s background completion-cache warming get?**
  Defaulted to `Keystroke` in Migration step 2 (safe, zero-behavior-change), but it's a genuinely
  third traffic shape — throttled (4-permit semaphore), asynchronous background work triggered by
  indexing, not a synchronous request from any LSP client action. A `LatencyClass::Background`
  variant with its own small budget (distinct from both `Keystroke`'s 0 and `Interactive`'s 3) might
  be the right long-term answer, but this document doesn't design it — flagged as unresolved, not
  silently folded into "the same as per-keystroke diagnostics" by assumption.
- **`resolve_symbol`'s budget is a raw attempt COUNT (`sidecar_budget: usize`), not a wall-clock
  deadline or a cancellation token.** Nothing in this document (or `walk_hierarchy`'s existing
  `sidecar_budget` precedent) bounds *time* — three cold JAR materializations on the
  `LatencyClass::Interactive` path (Migration step 3, once flipped) can still add up to a
  multi-second stall if each sidecar round trip is slow, independent of the count being "only 3."
  This project's own memory already records a prior multi-second hover/inlay stall regression from
  an earlier unbudgeted-promotion bug (see `perf-by-name-scan-cherry-pick` memory) — the failure
  mode this document is trying to prevent is not hypothetical. No timeout, cancellation, or
  async-yield story exists anywhere in this design for the JAR-promotion round trips themselves;
  budget only bounds *how many* blocking calls happen, not *how long* any one of them is allowed to
  take. Left as an explicitly unresolved open question, not a silent gap — a future pass should
  either add a wall-clock cap alongside the count, or establish (with real measurement, not
  assumption) that 3 sequential sidecar round trips are always fast enough in practice not to need
  one.
- **`jar.rs` (1633 lines) and `resolver/infer.rs` (2167 lines) are comparable in size to
  pre-split `resolve.rs`; `complete.rs` (2346 lines) is even larger.** Out of scope per Non-goals —
  none of the three is touched by this document, and this document does not claim continuity with
  either prior unification doc's own stated direction on this point (neither the CST doc nor the
  qualified-resolution doc scopes `infer.rs`/`complete.rs` restructuring as their own next step
  either; this is this document's own observation, not an inherited mandate). The same "spine +
  named siblings" approach likely applies to all three, leaving an asymmetric result — two
  comparably-sized files restructured, two left untouched — which is an acceptable, explicitly
  out-of-scope-for-this-pass outcome rather than an oversight, stated plainly rather than left to
  imply the split is now "done" project-wide.
- **Should `container.rs` also absorb `find_name_scoped_to_container` from `find.rs`?** Left in
  `find.rs` here to avoid touching a file this document doesn't otherwise need to change, but the
  qualified-resolution doc's own reuse-inventory table treats it and `range_encloses` as one
  family — worth a second look during implementation once `container.rs` exists and the seam is
  visible in a real diff, rather than deciding it speculatively here.
