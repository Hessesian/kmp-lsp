# Reindex Actor-Gate — Design

Status: **adopted (grep rule), token design deferred** — the pre-push grep rule (§2a) is
implemented and verified (`.git/hooks/pre-push`, check 5). The capability-token mechanism
originally proposed here (§2b) was rubber-ducked, critiqued, and — after a second Fable pass that
genuinely re-evaluated rather than defended it — downgraded to a documented fallback, not built.
See §2b for the revised reasoning.

## 1. Problem restatement (verified against current code)

`Indexer::index_workspace_full`/`index_workspace_prioritized`/`reset_index_state` are `pub(crate)`
(`src/indexer/scan.rs:686,710`, `src/indexer.rs:739`) — visible to the whole binary crate. Nothing
in the type system distinguishes "the actor's own scan coordinator calling this" from "some
other module reaching for it directly." That distinction matters because calling
`index_workspace_full`/`index_workspace_prioritized` without going through
`ScanHandler::handle_reindex` (`src/workspace/scan_handler.rs:121-158`) skips five Tier-1/
materialization fields being cleared (`jar_qualified`, `jar_bare_names`,
`jar_extension_receivers`, `materialized`, `materialization_failed`, cleared at
`scan_handler.rs:141-145`) and skips `spawn_jar_indexing()` (`scan_handler.rs:157`, the only
thing that ever re-crawls JAR dependencies). A JAR added after LSP startup then stays invisible
to completion forever, no matter how many reindexes run.

Confirmed call sites, read directly (not assumed from the task description):

- **`src/backend/commands.rs:150-159`** (`send_reindex_event`, shared by `handle_reindex_command`
  and `handle_clear_cache_command`) and **`src/backend/git_watcher.rs:86-117`**
  (`spawn_git_head_watcher`) — both now correctly do `self.event_tx.send(Event::Reindex).await`
  only. Both carry a doc comment pointing at `index_workspace_full`'s doc comment for why direct
  calls are wrong. **These are the two call sites the gate must make it impossible to regress.**
- **`src/workspace/scan_handler.rs:302-345`** (`execute_scan`, spawned from `enqueue_scan`) —
  calls `indexer.reset_index_state()` (line 322, only when `reset_before_scan` is set),
  `index_workspace_prioritized` (line 328), and `index_workspace_full` (line 333). This is the
  **one legitimate direct-call site** — it's the code that pairs the scan with the JAR-state
  bookkeeping in `handle_reindex`/`handle_change_root` above it.
- **`src/indexer/apply.rs:647-656`** (`apply_workspace_result`) calls `self.reset_index_state()`
  unconditionally as step one of "apply scan results." Traced the call graph:
  `apply_workspace_result` is only ever invoked from `finalize_workspace_scan`
  (`src/indexer/scan.rs:894-914`), which is called from inside `index_workspace_impl`'s callers
  (`index_workspace_full`, `index_workspace_prioritized`, and `run_pending_reindex`) — i.e. it
  already runs, unconditionally, as an internal step of *every* scan regardless of who triggered
  it. **This is a different concern from the actor-bypass bug and is confirmed out of scope**:
  gating it would not close any hole the method-level gate (below) doesn't already close, and it
  would break every unit test that builds a bare `Indexer` and drives it directly
  (`src/indexer/jar_tests.rs:270,915`, `src/indexer/live_tree_tests.rs:151`,
  `src/indexer/apply_tests.rs:687`).
- **`src/main.rs:190-212`** (`--index-only`) and **`src/cli/run.rs:165-183`**
  (`build_index_inner`, reached from `build_index`, used by every CLI subcommand) both build a
  fresh, standalone `Arc<Indexer>` with no actor, no `event_tx`, and no `Backend` — the process
  indexes once and exits/returns. Confirmed no long-lived session exists in either path (no
  `Actor`/`ScanHandler` construction anywhere in `src/main.rs` or `src/cli/*`). **Legitimately
  fine to keep calling this directly** — but seen below, they need a *narrower* fix so that
  giving them an escape hatch doesn't just recreate the bug under a new name.
- **Exhaustive grep** (`reset_index_state|index_workspace_full|index_workspace_prioritized|
  index_workspace\(`) turned up no other production call sites. Test-only callers:
  `src/indexer/scan_tests.rs:135,171,218` call `index_workspace_full` directly (3 sites, all
  inside `mod tests` nested in `scan.rs` via `#[path]`, i.e. a descendant of `indexer::scan`);
  `src/indexer_tests.rs:1949` only *mentions* `index_workspace_full` in a doc comment — the test
  itself drives a real `Actor` via `Event::Initialize`, so it isn't a direct-call site at all
  (comment is stale, not a bug). No test calls `index_workspace_prioritized` directly.
- **Aside, not part of this design**: `src/indexer/scan.rs:5` still lists
  `` [`Indexer::index_workspace`] `` — "normal LSP startup (bounded)" — as an entry point in the
  module doc comment. No such method exists (`grep -n "fn index_workspace\b"` finds nothing);
  normal startup actually goes through `index_workspace_prioritized` with an empty
  `initial_paths` (`scan_handler.rs:109-118`, `handle_initialize`). Worth a one-line fix
  (see §2b migration Task 2, if that plan is ever executed) but it isn't the bug this design
  addresses, and not worth a standalone doc-only PR on its own.

## 2a. Adopted mechanism: pre-push grep rule

Implemented in `.git/hooks/pre-push` as check 5, following the same awk-based per-file-skip
pattern the existing unwrap/expect check (check 3) already uses (chosen over the flatter
`grep -v` pattern checks 2/4 use, since that pattern's file-exclusion silently fails to work
per-line — verified while designing this rule, not assumed):

```bash
echo -n "  • Direct index_workspace_full/prioritized calls... "
REINDEX_BYPASS=$(git diff main..HEAD -- '*.rs' | awk '
    /^diff --git/ { skip = (/scan_handler\.rs/ || /indexer\/scan\.rs/ || /tests\.rs/ || /\/tests\//) }
    /^(---|[+][+][+])/ { next }
    !skip && /^\+.*\.index_workspace_full\(|^\+.*\.index_workspace_prioritized\(|^\+.*Indexer::index_workspace_full\(|^\+.*Indexer::index_workspace_prioritized\(/ { print }
' || true)
if [[ -n "$REINDEX_BYPASS" ]]; then
    echo "❌"; echo "$REINDEX_BYPASS" | head -5 | sed 's/^/    /'; VIOLATIONS=1
else
    echo "✅"
fi
```

Blocks any added line calling `.index_workspace_full(`/`.index_workspace_prioritized(` (dot-call
or fully-qualified `Indexer::` form) outside `scan_handler.rs`, `indexer/scan.rs` itself, and test
files — the same shape as the bug that already happened once in `backend/commands.rs` and
`backend/git_watcher.rs`.

**Verified both directions, not just reasoned about:**
- Ran against the real (already-fixed) combined branch diff — zero output, no false positive
  against `scan_handler.rs`'s legitimate calls or the routed-through-the-actor fix.
- Added a decoy commit reintroducing the exact original bug (a direct
  `self.indexer.clone().index_workspace_full(...)` call inside
  `backend/commands.rs::handle_reindex_command`), ran the hook, confirmed it printed the decoy
  line and blocked the push, then reverted the decoy (`git reset --hard HEAD~1`) before it ever
  reached a real commit.

### Why this instead of the token mechanism in §2b

A second-pass Fable review (asked to critique its own earlier §2b recommendation, not defend it)
found the token design didn't hold up as well as it first appeared, once these three things were
checked directly against the code rather than reasoned about in the abstract:

1. **The claimed "one new coupling" was measured incompletely.** The original grep only checked
   `use crate::workspace` in `indexer/*.rs` (correctly zero). It never checked `use crate::cli`.
   `indexer/jar.rs:25` already imports `crate::cli::extract_sources`, and six files under
   `cli/*.rs` already import `indexer::*` — `indexer` and `cli` are already bidirectionally
   coupled in production. The `StandaloneIndexGate` half of the token design was adding a marker
   type to a boundary that's already thoroughly tangled, not a clean one.
2. **Neither mechanism protects the actual failure mode, only a file boundary.** A future method
   added *inside* `scan_handler.rs` that constructs a fresh gate token (or, for the grep rule,
   simply lives in the excluded file) and calls `index_workspace_full` directly — skipping the
   Tier-1 clearing and `spawn_jar_indexing()` call that make `handle_reindex` correct — passes
   either gate cleanly. Both approaches encode "this call must textually originate outside file
   X," not "this call must go through the correct sequence." The token design's implied
   completeness advantage over a grep rule doesn't hold up under this specific failure mode.
3. **Cost asymmetry, compared honestly.** The full token design: 2 new marker types, a 4-way
   method split (`_body` + 3 gated wrappers), a visibility bump (`cli::run` module), a new
   `build_index_only` function the design itself flagged as introducing dedup debt against the
   near-duplicate `build_index_inner`, across 6 files. The grep rule: one block in an existing
   file, matching an idiom this repo already uses four times.

The one place the token design still has a real edge: this bug is **silent** (no crash, no
failing test — unlike the loud `unwrap()` panics the same hook already guards), and the hook
itself is local, untracked, not run in CI, and bypassable with `--no-verify`. That's a genuine
asymmetry against the other conventions this hook enforces, and the honest reason to keep §2b
on file rather than delete it — see the trigger conditions at the end of §2b.

## 2b. Mechanism considered and deferred — token design

Kept as a fallback design, not built. Revisit only if either: (a) the grep rule in §2a is
demonstrated to have actually been evaded in practice (a real push slipped a bypass through, or
`--no-verify` was used carelessly on this exact class of change), or (b) this project gains
contributors or CI infrastructure where the local, untracked hook can no longer be relied on to
be present for every contributor. If that trigger fires, build **only** the `ReindexGate` half
below (the `indexer↔workspace` edge, which is genuinely new and protects the bug that actually
occurred) — **not** `StandaloneIndexGate`/the CLI half, which §1 already found unnecessary (the
CLI path has no live-session/JAR-staleness concern to begin with) and which item 1 above shows was
adding a marker type to an already-tangled boundary for no protective benefit.

### Why plain visibility can't do this

`backend::commands`, `workspace::scan_handler`, and `indexer::scan` are three sibling subtrees
under the crate root (`main.rs`: `mod backend; mod indexer; mod workspace;` — none is nested in
another). `pub(in path)` requires `path` to be an *ancestor* of the item's defining module
(rustc: "visibilities can only be restricted to ancestor modules"). There is no `path` that is
an ancestor of `indexer::scan` (where the methods live) and also names a scope that contains
`workspace::scan_handler` but excludes `backend::commands` — both are equally "not
`indexer::scan`, not each other's ancestor." Restructuring the module tree so one is nested
inside another was considered and rejected: `ScanHandler` owns actor-only coordination state
(`scan_queue`, `jar_indexing_in_progress`, generation checks) that has no reason to live under
`indexer`, and moving `backend` under `workspace` (or vice versa) to make visibility work would
be a much larger, unrelated restructure for a narrowly-scoped bug. **Verified this limitation is
real**, not assumed — see the two rustc runs below.

### The trick that does work: constructor privacy is a different axis than module-path visibility

A struct's *type* can be `pub(crate)` (nameable everywhere) while its *constructor* is
inaccessible outside the defining module, by giving it a private field:

```rust
pub(crate) struct ReindexGate(());   // field `()` has no `pub` — private
```

Privacy for struct fields follows "defining module + its descendants," same as any other private
item — but this is orthogonal to which *module path* can be named. Concretely verified with a
throwaway crate mirroring this codebase's module shape:

```
error[E0603]: tuple struct constructor `Gate` is private
  --> src/main.rs:16:33
   |
16 |         let g = crate::trusted::Gate(());
   |                                 ^^^^ private tuple struct constructor
```

— a foreign module can name `Gate` as a type but cannot produce a value of it. This is the gate.
(Also verified separately that a private, non-`pub(crate)` `mod run;` inside `cli/mod.rs` blocks
even *naming* the type from outside `cli` — `error[E0603]: module 'run' is private` — which is
why one visibility bump is needed in the migration below; not the same axis as the field trick,
but easy to conflate, so both were checked independently by compiling real snippets rather than
reasoning from the reference alone.)

### Design (revised scope): one token, `ReindexGate`, defined in `src/workspace/scan_handler.rs`

**`StandaloneIndexGate`/CLI gating dropped entirely, not deferred alongside this** — §1 already
found the CLI path (`main.rs --index-only`, `cli/run.rs`) has no live-session/JAR-staleness
concern at all (no actor, no long-running process to go stale), so gating it protects against
nothing. Only `ReindexGate` remains, narrowest correct scope (the file, not a broader `workspace`
module — `scan_handler_tests.rs` is nested inside it via the same `#[path]` pattern as
`scan.rs`/`scan_tests.rs`, so its own tests keep working for free):

```rust
/// Proof the caller is the actor's scan coordinator — the only code path that
/// pairs a workspace scan with the Tier-1 JAR-state clearing and re-crawl a
/// live-session reindex requires (see `handle_reindex` above). The private
/// field means only code physically inside this module can construct one:
/// `backend::commands`/`backend::git_watcher` can name `ReindexGate` (it's
/// `pub(crate)`) but can never produce a value of it, so they cannot call
/// `Indexer::index_workspace_full`/`index_workspace_prioritized` without
/// going through `Event::Reindex`, at compile time.
pub(crate) struct ReindexGate(());
```

**Gated methods** (`src/indexer/scan.rs`), the shared scan body factored into a private
`impl` helper both wrap (the private helper — `index_workspace_impl` — already exists at
`scan.rs:943` and is already un-gated/module-private today, so this reuses an existing seam
rather than inventing one):

```rust
impl Indexer {
    /// Actor-coordinated full reindex of workspace source files. Callable only
    /// from `workspace::scan_handler` — see `ReindexGate`. CLI callers
    /// (`--index-only`, `cli/run.rs`) keep calling today's un-gated
    /// `index_workspace_full` unchanged — no actor exists in that process, so
    /// there is nothing to bypass (confirmed in §1; not gated per the revised
    /// scope above).
    pub(crate) async fn index_workspace_full<R: ProgressReporter + 'static>(
        self: Arc<Self>,
        root: &Path,
        reporter: Arc<R>,
        _gate: crate::workspace::scan_handler::ReindexGate,
    ) {
        self.index_workspace_full_body(root, reporter).await
    }

    /// Test-only equivalent of `index_workspace_full`, minus the gate — `cfg(test)`
    /// code never ships in a release binary, so this cannot become a production
    /// bypass vector. Used by `scan_tests.rs` in place of constructing a real
    /// `ReindexGate` it has no business holding.
    #[cfg(test)]
    pub(crate) async fn index_workspace_full_for_test<R: ProgressReporter + 'static>(
        self: Arc<Self>,
        root: &Path,
        reporter: Arc<R>,
    ) {
        self.index_workspace_full_body(root, reporter).await
    }

    // Renamed from the current `index_workspace_full` body verbatim — this is
    // "the source-file-scan half of a reindex," not "the reindex operation";
    // `_full` in the old name read as "the complete thing," which is exactly
    // the naming trap that let the original bug happen unnoticed.
    async fn index_workspace_full_body<R: ProgressReporter + 'static>(
        self: Arc<Self>,
        root: &Path,
        reporter: Arc<R>,
    ) {
        /* unchanged body of today's index_workspace_full */
    }

    /// Prioritized indexing — actor-only (only `ScanHandler::execute_scan` and
    /// its own tests call this; no CLI variant needed).
    pub(crate) async fn index_workspace_prioritized<R: ProgressReporter + 'static>(
        self: Arc<Self>,
        root: &Path,
        initial_paths: Vec<PathBuf>,
        reporter: Arc<R>,
        _gate: crate::workspace::scan_handler::ReindexGate,
    ) {
        /* unchanged body */
    }
}
```

Note this still requires a CLI-facing question if ever built: since `index_workspace_full` itself
becomes gated, `main.rs`/`cli/run.rs` would need *some* un-gated entry point. The cleanest option
consistent with "drop CLI gating, not add a second gate" is to give `index_workspace_full_body`
itself a `pub(crate)` visibility restricted to `indexer`'s own submodules plus a thin, un-gated
`pub(crate) fn index_workspace_standalone(...)` wrapper defined in `indexer/scan.rs` (not in
`cli/run.rs` — no new marker type, no cross-module coupling, just a second un-gated wrapper next
to the gated one). Left as an implementation detail for whoever picks this up if the trigger in
§2b's header ever fires, not fully specified here since it's a small decision, not a design one.

`reset_index_state` (`src/indexer.rs:739`) stays exactly as it is today — un-gated, `pub(crate)`
— per §1's finding that its one production call site (`apply.rs`) is already-unconditional
internal plumbing, and its several test call sites need direct access to a bare `Indexer` with no
actor in the picture at all.

### One new coupling (revised claim — verified in both directions this time)

`indexer/scan.rs` currently has zero dependency on `workspace` (`grep -rn "use crate::workspace"
src/indexer.rs src/indexer/*.rs` returns nothing) — this edge is genuinely clean today, and naming
`crate::workspace::scan_handler::ReindexGate` as a parameter type would be a real, novel back-edge
on it (`workspace` already depends on `indexer`, the normal direction; this adds the reverse).

This is *not* the same situation as `indexer↔cli`, which the original version of this document
claimed was equally clean — **that claim was wrong**, caught during the rubber-duck/Fable-critique
pass: `indexer/jar.rs:25` already imports `crate::cli::extract_sources`, and six files under
`cli/*.rs` already import `indexer::*`. That boundary is already bidirectionally tangled, which is
part of why the CLI half of this design was dropped rather than built — a marker type doesn't
protect a boundary that's already open in both directions, and the CLI path didn't need protecting
in the first place (§1).

The `indexer↔workspace` coupling `ReindexGate` would add is a bare marker type with no methods
and no behavior; it costs one `use` line, is invisible to control flow, and (via the `#[cfg(test)]`
variant) disappears entirely from release builds for test call sites.

### Alternatives considered and rejected

1. **Unsealed marker trait (`trait ReindexAuthority {}`), each caller implements it for their own
   type, methods take `impl ReindexAuthority`.** Rejected: this is *not* a gate. Because trait
   impl coherence in Rust is crate-scoped, not module-scoped, `backend::commands` could define
   `struct Whatever; impl ReindexAuthority for Whatever {}` locally and pass `Whatever` in —
   zero-cost, fully legal, completely defeats the point. Sealing the trait (private supertrait)
   just relocates the exact same ancestor-path problem the whole design exists to solve, so it
   buys nothing over the concrete-type approach and adds a layer of indirection.
2. **Push the JAR-clearing/re-crawl side effect down into `Indexer` itself** (following this
   codebase's own precedent, `docs/agent-reference.md` §16, "side effects belong at the write
   site, not scattered at call sites" — e.g. `ScanHandler::set_root` bundling the generation
   bump). Rejected for this specific case: the side effect needs `jar_indexing_in_progress`,
   `configured_jar_paths`, and `scan_queue`-based generation coordination, all of which are
   actor-only state that lives on `ScanHandler`, not `Indexer` — moving it down would mean
   duplicating actor coordination primitives into the lower-level type, a much bigger change than
   this bug warrants. (§16's pattern is the right shape for *within-module* forgetfulness; this
   bug is cross-module bypass, which needs a boundary, not a bundling.)
3. **Just rename `index_workspace_full` and rely on that alone (no gate).** This is the doc
   comment's current mitigation. Renaming genuinely helps (a name that doesn't read as "the whole
   reindex" removes the specific psychological trap that caused the original bug) and is folded
   into this design (`index_workspace_full_body`), but by itself it's still only a convention —
   the task asks for a compile error, and renaming alone doesn't produce one.

### §2b migration plan (only if the trigger conditions at the top of §2b fire)

- [ ] **Task 1 — Add `ReindexGate` to `src/workspace/scan_handler.rs`.**
  Add the private-field marker type near the top of the file. Update the two call sites inside
  `execute_scan` (`scan_handler.rs:328,333`) to pass `ReindexGate(())`. No other file in
  `workspace/` needs to change — `handle_reindex`, `handle_change_root`, etc. never call the
  indexer methods directly, only `enqueue_scan`/`execute_scan` do.

- [ ] **Task 2 — Split `index_workspace_full` in `src/indexer/scan.rs`.**
  Rename the current method body to a new private `index_workspace_full_body` (verbatim, no
  logic change). Add two wrappers: `index_workspace_full` (gated, `ReindexGate`) and `#[cfg(test)]
  index_workspace_full_for_test` (ungated). Add the `_gate: ReindexGate` parameter to
  `index_workspace_prioritized`. Decide at implementation time whether `main.rs`/`cli/run.rs` get
  a second un-gated `index_workspace_standalone` wrapper defined in this same file (see the note
  in §2b's Design section above) — re-verify against the code at that time rather than trusting
  this plan's snapshot of `main.rs`/`cli/run.rs`'s current shape. Update the module doc comment at
  the top of the file (`scan.rs:4-8`) to list the new wrapper(s), and drop the stale
  `` [`Indexer::index_workspace`] `` bullet (§1 aside) — replace it with a note that bounded
  startup indexing is `index_workspace_prioritized` with empty `initial_paths`.

- [ ] **Task 3 — Update `src/indexer/scan_tests.rs`.**
  Change the 3 call sites (`scan_tests.rs:135,171,218`) from `.index_workspace_full(&workspace,
  Arc::new(NoopReporter))` to `.index_workspace_full_for_test(&workspace,
  Arc::new(NoopReporter))`. No other test file needs changes (confirmed in §1: no other test
  calls these two methods directly).

- [ ] **Task 4 — Confirm `apply.rs`, `reset_index_state`, and `main.rs`/`cli/run.rs` need no
  gating change** (re-verify §1's conclusions still hold, don't just trust this snapshot).

- [ ] **Task 5 — `cargo build && cargo test` clean**, then run the decoy check below before
  merging.

### §2b verification plan (decoy check, same shape as §2a's — only relevant if §2b is ever built)

**Positive check**: `cargo build`/`cargo test` succeed after Tasks 1-4.

**Negative check** — a decoy commit, reverted after confirming the failure, never merged: add a
direct, ungated `self.indexer.clone().index_workspace_full(&root, reporter).await` call inside
`backend/commands.rs`. Expect first `error[E0061]: this function takes 3 arguments but 2 arguments
were supplied`; then, attempting to supply a gate honestly
(`crate::workspace::scan_handler::ReindexGate(())`), expect `error[E0603]: tuple struct
constructor 'ReindexGate' is private` (wording confirmed against a real rustc build of an
equivalent minimal snippet during the original design pass). The second error is the actual proof
— it shows there is no syntactically valid way for `backend::commands` to obtain a `ReindexGate`,
not merely that one call was missing an argument. Revert before committing anything else.

## Out of scope

- **The token design (§2b) itself is deferred, not built** — see the trigger conditions at its
  header. §2a (the grep rule) is what's actually adopted and implemented.
- `StandaloneIndexGate`/CLI gating — dropped from the design entirely (not deferred alongside
  `ReindexGate`): §1 found the CLI path has no live-session/JAR-staleness concern, and the
  rubber-duck/Fable-critique pass found the `indexer↔cli` boundary this would have gated is
  already bidirectionally coupled in production, so a marker type there would protect nothing.
- `reset_index_state` gating (§1 — confirmed orthogonal, internal scan-pipeline plumbing, used by
  every scan regardless of caller).
- Unifying `main.rs`'s `--index-only` behavior with `cli::run`'s `index` subcommand's near-
  duplicate source-path-collection logic — a real dedup candidate noticed while tracing call
  sites for §1, but unrelated to this bug.
- Fixing the stale `` [`Indexer::index_workspace`] `` module-doc bullet beyond what's folded into
  §2b Task 2 if that plan is ever executed — no functional impact, just documentation drift.
- Any restructuring of the `indexer`/`workspace`/`backend`/`cli` module tree (nesting one inside
  another) to make `pub(in path)` visibility work instead of the token trick — considered in §2b
  and rejected as disproportionate to this bug.
