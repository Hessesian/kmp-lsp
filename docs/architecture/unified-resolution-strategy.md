# Unified Resolution Strategy

Status: **proposed** (2026-06-17). Captures the architectural direction agreed after a
series of per-consumer resolution fixes. Not yet implemented — see "Migration".

## The problem

Every language feature re-implements the same core question — *"given a name (+ maybe a
receiver) at a call site, which symbol(s) does it bind to?"* — each with its own candidate
set, its own reachability filter, and its own overload policy. Fixes land one consumer at a
time; the same two gaps get re-patched in each.

| Consumer | Entry point | Candidate source | Reachability filter | Overload policy |
|---|---|---|---|---|
| go-to-def | `resolver::resolve::resolve_symbol_inner` | definitions → imports → same-pkg → star → hierarchy → rg | `resolve_via_imports` (per-symbol pkg via `jar_symbol_packages`; previously a blanket `jar:` bypass) | returns all |
| call-arg diagnostics | `indexer::infer::sig::resolve_call_signature` | `definitions` + `jar_definitions` | `is_import_reachable` (**fails open for `jar:` URIs** — jar files aren't in `idx.files`) + library `required`-clamp | arity envelope → `Overloaded` skip |
| type inference (hover/inlay/completion) | `indexer::infer::chain::resolve_call_expr_type`, `resolver::infer::find_fun_return_type_by_name` | **`definitions` only** (ignores `jar_definitions`!) | **none** (picks first match across all packages) | picks first |
| hover / signature help | `indexer::infer::sig::find_fun_signature_full` | definitions → rg/on-demand → jar | `resolve_symbol_no_rg` | first |
| completion (dot) | `resolver::complete::resolve_dot_receiver_type` | text heuristics (`infer_lines`) → `infer_receiver_type` → CST fallback | n/a (resolves receiver type, then members) | n/a |

Each session-fix bridged the *same* gaps in a different row.

## Two root causes

1. **Two symbol universes at different fidelity.** Workspace + sources-JAR symbols live in
   `definitions` / `files`; compiled-JAR symbols live in `jar_definitions` / `jar_files`.
   Jar symbols historically lacked real packages, real param counts, and return types — so
   any consumer that wanted to filter or read them had to special-case. (This is *why* the
   paths diverged: uniform filtering wasn't possible.) Enriching jar symbols is the
   precondition for unification, and is mostly done now (see "Done this session").

2. **`qualified` collapses overloads.** `indexer.qualified: DashMap<String, Location>` maps an
   FQN to a *single* location. `remember` has 5 public overloads sharing
   `androidx.compose.runtime.remember`, so the one O(1) "correct" path can only keep one —
   pushing consumers onto unfiltered short-name scans (which then match unrelated jars).

## Done this session (the enabling data-model work — was per-consumer patching)

- JAR symbols now carry **real `(required, total)` param counts** parsed from the sidecar
  `detail` (`jar::params_from_detail`); jar cache bumped (v8).
- JAR symbols now carry **real per-symbol package + `top_level`** (sidecar `SymbolEntry.pkg`
  / `top_level`), stored in the `jar_symbol_packages` side table; correct FQNs registered in
  `qualified` (`pkg.name` for top-level, `pkg.Container.name` for members).
- `resolve_via_imports` filters jars by **real package** (replaced the blanket `jar:` bypass).
- call-arg diagnostics: library `required`-clamp (sidecar emits no default markers, so trust
  `total`, not `required`); suppress while JAR indexing in flight; republish on `jar_done`.
- type inference: `remember`/`rememberSaveable` infer their **trailing-lambda result**;
  **constructor fallback** (`Foo(...)` → `Foo`); **CST variable-init fallback** so completion
  resolves `val x = remember { Foo() }`.

All of these are thin bridges over the same two gaps. Unification turns "N patches" into
"1 filter to get right, N thin projections."

## Target shape

One core that does candidate enumeration + reachability filtering and returns a rich record:

```rust
struct ResolvedSymbol {
    location: Location,
    kind: SymbolKind,
    package: String,
    source_set: SourceSet,        // Main / Test / Library
    params: (u8, u8),             // (required, total)
    return_type: Option<String>,
    type_params: Vec<String>,
    detail: String,
    deprecated: bool,
}

enum IoPolicy { NoIo, AllowIo }   // diagnostics = NoIo (keystroke latency); hover = AllowIo

fn resolve(
    name: &str,
    receiver: Option<&ReceiverType>,
    caller_uri: &Url,
    io: IoPolicy,
) -> Vec<ResolvedSymbol>;
```

Each feature becomes a thin projection:
- **go-def** → `.map(|s| s.location)`
- **call-arg diag** → arity envelope over `.params` (→ `Overloaded` when distinct)
- **type inference** → best `.return_type` (+ generic subst, + lambda-result special cases)
- **hover / sig-help** → best `.detail`
- **completion (dot)** → members of the receiver type

### What must NOT be merged
- **IO policy genuinely differs** — diagnostics/sig-help/completion must stay rg/disk-free for
  keystroke latency; hover/lambda-inference may block. So `resolve` takes `IoPolicy`; the
  candidate/filter core is shared, the *reach* differs. (This is a legitimate reason some
  paths split today.)
- **Language-semantic special cases stay as thin layers on top**: scope functions
  (`apply`/`run`/…), lambda-result functions (`remember`), generic type-arg substitution,
  constructor inference. These are real semantics, not duplication.

## Migration (incremental, test-anchored — no big-bang)

In a ~1350-test codebase the only safe path is one consumer at a time:

1. **Land the data-model precondition** (mostly uncommitted now): per-symbol jar package +
   params + FQNs; make `find_fun_return_type_by_name` consult `jar_definitions` *and* filter
   by import reachability (currently it does neither — the `remember`→`RealVariable` bug).
2. **Build `resolve_core` alongside** the existing paths; anchor it with the *union* of the
   existing resolution tests before porting anything.
3. **Port one consumer at a time**, smallest first (go-def's import step → diagnostics →
   type-inference), deleting each bespoke filter as its consumer moves. Keep tests green.
4. **Promote `qualified` to `Vec<Location>`** so overloads survive — removes a whole class of
   short-name-scan fallbacks on its own.

## Open follow-ups (tracked)

- **`find_fun_return_type_by_name` package filtering** — still ignores `jar_definitions` and
  does no reachability filtering; worked around only for `remember` via the lambda
  short-circuit. Other cross-jar name collisions can still infer a wrong return type.
- **Cache schema-fingerprint guard** — auto-invalidate caches on struct-layout drift (catches
  the "forgot to bump `CACHE_VERSION`" footgun); keep explicit versions for *semantic* bumps
  (a layout hash can't catch a meaning change like `params_from_detail`). Agreed as its own PR.
- **`qualified` → `Vec<Location>`** to represent overloads.

## Debugging recipe: ground-truth harness

The reliable way to see what *actually* resolves (jars + sources-jars matter and unit tests
can't reproduce them): a temporary `#[ignore]` test that indexes the real sample project.

```rust
#[tokio::test] #[ignore]
async fn ground_truth() {
    let root = std::path::Path::new("/home/ocel/Work/samples/nowinandroid");
    let idx = Arc::new(Indexer::new());
    Arc::clone(&idx).index_workspace_full(root, Arc::new(NoopReporter)).await;
    let jars = crate::indexer::jar::scan_gradle_jars(None);
    if let Ok(mut g) = idx.jar_sidecar.lock() { crate::indexer::jar::index_jars(&idx, &jars, &mut g); }
    crate::indexer::jar::index_sources_jars(&idx, None, None); // populates `definitions` (Library) — needed to repro type-infer bugs
    // … call resolve_symbol / infer_expr_type / infer_receiver_type and eprintln! …
}
```

Run: `cargo test --bin kmp-lsp -- ground_truth --ignored --nocapture`.

Gotchas:
- **Test builds don't launch the sidecar** (`#[cfg(not(test))]`), so `index_jars` only works
  off a *warm* jar cache. Warm it first with the installed CLI: `kmp-lsp find <name> --root <proj>`.
- After bumping a cache version, the first warm run re-scans all jars (slow, minutes).
- `definitions["remember"]` etc. is only populated after `index_sources_jars` — omit it and
  type-inference bugs won't reproduce.

## Addendum (2026-09-14): root cause #2 recurred independently, in a second registry

A `resolution-accuracy` Gap-cluster investigation (real Android corpus,
`/home/ocel/Work/Moneta/android`) found the *identical* "collapses overloads" defect this
doc named as root cause #2 — but in a completely separate registry this doc never
mentions: `extension_by_receiver: DashMap<String, Vec<ExtensionEntry>>` and its readers in
`src/resolver/extension.rs` and `src/resolver/qualified.rs`. The map's own value type is
already `Vec<ExtensionEntry>` — plural, correct — but every consumer function that reads it
still narrows to one candidate before returning, by construction:

- `resolve_extension_in_scope` (`extension.rs:69`) — `return vec![Location{..}]` on the
  *first* in-scope entry, inside the loop, despite a `-> Vec<Location>` signature that
  promises otherwise.
- `jar_extension_for_type_root` (`qualified.rs:505-542`) — the identical pattern, on the
  last-resort path taken for exactly the built-in/uncompiled receivers this bug class hits
  hardest.
- `implicit_receiver_extension_match` (`extension.rs:144-168`) — same first-match narrowing,
  on a third, separate call path.
- `QualifiedCandidates`'s own fields (`qualified.rs`) — `own_type_extension` /
  `supertype_extension` were typed `Option<Location>`, so even a caller that *did* get
  several locations from one of the functions above had nowhere to put more than one.

**Why this doc's target architecture (`Vec<ResolvedSymbol>` core) didn't already prevent
it:** it was never built here. The CST-resolution-unification effort
(`docs/superpowers/specs/2026-06-30-cst-resolution-unification-design.md`) that *did* land
explicitly scoped this territory OUT: *"String path (`resolver/resolve.rs`, `complete.rs`,
string `infer_*`)... intentionally heuristic; NOT unified (deep-analysis later)."*
`extension.rs` and `qualified.rs` are string-path resolver code — the exact files that
"later" never reached. A subsequent, narrower pass into the same file
(`docs/superpowers/specs/2026-08-24-qualified-resolution-unification-design.md`, "5 instances
of a same-named-sibling-leak bug") fixed a *different* defect sitting in the same file and
stopped there. And **PR #304** (2026-09-02, `find_all_names_scoped_to_container` returns
every same-name candidate instead of `.find()`'s first) fixed this *exact* defect shape for
the **member** tier only — the fix was never swept to its structural sibling, the extension
tier, which has now been shown to carry the identical bug in at least four places.

**The pattern to take from this for any future sweep:**

1. **A root cause named once is not fixed once it's fixed in one place.** When a defect
   shape is found and fixed (PR #304's member-tier fix), the correct follow-up is not "close
   the ticket" but "grep every sibling function solving the same kind of problem and check
   each one individually" — `resolve_extension_in_scope`, `jar_extension_for_type_root`, and
   `implicit_receiver_extension_match` all do the conceptual job "given a name and a
   receiver-ish key, return every matching declaration" and all three independently got it
   wrong the same way, undetected, for over a month. A checklist item ("does this function's
   return type promise more than one candidate, and does its body ever actually produce
   more than one?") applied to every `-> Vec<T>`-returning resolution function would have
   caught all four sites in one pass instead of one Gap-cluster investigation at a time.
2. **A "scoped out for now" boundary needs an expiry, not just a label.** The CST-unification
   doc's exclusion of the string path was reasonable in June (a real architectural
   decision — heuristic resolution has to work cold, without a synced project, which the CST
   path can't do). But "deep-analysis later" with no owner and no date is how a real,
   root-cause-named defect survives three subsequent unification passes that all worked in
   adjacent code without ever being pointed at it.
3. **Independently-derived registry keys drift.** A second, smaller instance of the same
   underlying problem (no single chokepoint) turned up in the same investigation: the
   `extension_by_receiver` key is computed independently at 3+ sites — source-side
   (`parser::extension_receiver_from_decl`) and JAR-side (`jar.rs`, twice, Tier 1 and Tier
   2) — each hand-rolling its own generics/nullability/package-prefix stripping, with no
   shared function forcing agreement. They now disagree (source strips a trailing `?`, JAR
   doesn't), and nothing short of manually diffing all derivation sites would have surfaced
   it. Any registry with more than one writer needs its key-derivation logic to live in
   exactly one function, called by every writer — not independently reimplemented per
   writer "because they're slightly different call sites."

None of this changes root cause #2's original prescription (`Vec<ResolvedSymbol>`
everywhere, one core) — if anything it's a second, independent data point for it. It *does*
mean the migration plan above should not assume "the parts we haven't unified yet are just
old code we haven't gotten to" — some of them are old code a design doc *deliberately*
routed around, and that routing decision is exactly where a structurally-identical bug will
keep re-appearing until the boundary itself is revisited, not just patched from outside it.
