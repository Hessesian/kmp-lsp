# Producer-detection CST follow-up — replace hand-rolled text scanning with indexed `SymbolEntry.detail`

> **2026-09-22, follow-up to PR #324** (branch `worktree-inferred-receiver-refs`). User's own review
> of the merged-quality branch: "there's plenty of hand parsing what should be CST based, why wasn't
> that approach used?" — a fair catch. `declared_member_name_returning` and its three shape-scanners
> (`kotlin_function_returning`, `kotlin_property_typed`, `java_method_returning`, `src/rg.rs`) do
> `std::fs::read_to_string` + a manual per-line character scan to detect "does this declaration's
> return/declared type equal `owner_class`" — exactly the "string parsing where the CST should be
> the source of structure" pattern AGENTS.md and `docs/superpowers/specs/2026-06-30-cst-resolution-unification-design.md`
> warn against. This plan replaces it with already-indexed, already-CST-extracted data.

## What already exists (checked before writing this, per the user's explicit ask)

- `SymbolEntry.detail` (`src/types.rs:216-221`) — a short signature string computed ONCE at parse
  time via `extract_detail_from_node`/`extract_detail`/`extract_detail_from_lines` (`src/parser.rs:1642-1700ish`).
  `extract_detail_from_node` takes text from the declaration node's start byte up to its body's start
  byte (or falls back to line-range text for bodyless declarations), joining multi-line text with
  spaces. This means `detail` ALREADY correctly handles: multi-line signatures (joined), extension
  receivers (`"fun Foo.openBody(): Body"`), leading type params (`"fun <T> openBody(): Body"`), and
  is populated for both Kotlin and Java declarations (the extractor is language-agnostic, called from
  both parsing paths) — every shape-miss the PR #324 final review flagged as an undocumented gap in
  the hand-rolled scanner is handled for free by this already-built data.
- `Indexer::files: DashMap<String, Arc<FileData>>` (`src/indexer.rs:163`), keyed by the file's full
  URI string (`Url::from_file_path(path)?.as_str()` — same conversion used throughout `indexer.rs`,
  e.g. `indexer.rs:456,527,652`). `FileData.symbols: Vec<SymbolEntry>` (`src/types.rs:450`) is the
  per-file symbol enumeration this task needs — no new walk required.
- `extract_extension_receiver`/`extract_extension_receiver_full` (`src/parser.rs:1230,1250`) — existing
  helpers that parse a `detail` string's extension-receiver prefix. Confirms `detail`-string parsing
  (not raw-source parsing) is this codebase's established pattern for this exact kind of question.
- **Missing, confirmed by grep** (`extract_return_type`, `return_type_from_detail`, or similar — none
  found in `src/parser.rs` or `src/types.rs`): no existing helper extracts a declaration's return/
  declared type FROM a `detail` string. This is the one genuinely new, small piece.

## Scope

Only hop 1.5 (declaration/producer detection) changes. Hop 2 (searching the workspace for callers of
the discovered producer names) stays rg-based — that is a legitimate "does an arbitrary file mention
this bare word" text search over files whose content isn't necessarily indexed/parsed at query time,
not a declaration-structure parse; it was never the hand-parsing the user flagged.

## Task 1: replace `declared_member_name_returning`'s file-reading + line-scanning with indexed `SymbolEntry.detail` lookup

Files: `src/rg.rs`, `src/rg_tests.rs` only.

1. Write `declared_type_from_detail(detail: &str) -> Option<&str>` (or a closely-named equivalent —
   exact naming is your call, AGENTS.md rules apply: no abbreviated names, full words) that extracts
   the trailing type token from an already-CST-extracted `detail` string:
   - A function-shaped detail (`"fun openBody(): Body"`, `"fun openBody(): List<Body>"`, `"fun Foo.openBody(): Body"`)
     — find the LAST top-level `": "` after the closing `)` of the parameter list (not inside nested
     generics/parens), take the type token after it.
   - A property-shaped detail (`"val isOnline: Boolean"`, `"var count: Int"`) — find the `": "` after
     the property name, take the type token after it.
   - A Java method detail (whatever shape `extract_detail_from_node` actually produces for Java — READ
     a real example first via a quick unit test or by tracing the Java parsing call sites at
     `src/parser.rs` before assuming Kotlin's `"fun ...): Type"` shape applies; Java's return type comes
     BEFORE the method name, not after — this needs its own case, do not assume the Kotlin shape covers it).
   - Returns `None` for anything that isn't a function/property/method shape (e.g. `"class Foo"`,
     `"init(...)"` — reuse `SymbolEntry.kind` to gate this rather than sniffing the string, since you
     already have the typed `SymbolKind` from the `SymbolEntry` this detail came from — prefer the type
     you already have over re-deriving it from a string).
   - This function's job is ONLY to extract the type token from an already-normalized string — it
     does no CST/tree-sitter work itself and needs none, because `extract_detail_from_node` already
     did the real CST-bounded extraction at parse time. Keep it a small, pure string function; do not
     over-build it into a parser.
2. Reuse the EXISTING dotted-segment matching logic added during PR #324's fix round
   (`type_annotation_matches_owner` or whatever it's named now — read the current code, it was added
   in the fix-round commit, name may not match this description exactly) to compare the extracted type
   token against `owner_class` — do not duplicate that matching logic, call it.
3. Replace `producer_scoped_candidate_files`'s hop-1.5 body: instead of
   `std::fs::read_to_string` + `content.lines()` + the three shape-scanner functions
   (`kotlin_function_returning`, `kotlin_property_typed`, `java_method_returning` — DELETE these three
   once nothing calls them; verify via grep, per AGENTS.md's zero-references-before-deleting rule,
   Serena being unavailable this worktree per the standing session note) —
   for each hop-1 file: convert its path to the `index.files` URI-string key (same conversion pattern
   as `indexer.rs`'s existing call sites — read one to copy the exact idiom), look it up in
   `request`'s `Indexer` (check what's actually reachable from `RgSearchRequest`/the calling context —
   it may not currently carry an `&Indexer` reference; if it doesn't, thread one through, or find
   whatever this repo's convention is for stateless-rg-code-that-needs-index-access — check how other
   rg.rs code that already touches `index_candidates`/`files_importing_nested` gets its `&Indexer`),
   iterate `file_data.symbols`, call your new `declared_type_from_detail` on each symbol's `detail`,
   keep symbols whose extracted type dotted-matches `owner_class`, collect their names (same output
   shape hop 1.5 produced before — a set of producer member names, capped at
   `MAX_OWNER_PRODUCING_MEMBER_NAMES` exactly as today).
4. A hop-1 file not present in `index.files` (not yet indexed) yields no producer names from that
   file — this is a safe degrade (falls toward `NoProducerFound`/`SkippedTooBroad`, never wider than
   today), matching the existing floor. Do not add a filesystem-read fallback for this case — that
   would reintroduce the exact problem being removed.
5. Remove the now-dead `std::fs::read_to_string` call and any now-unused imports.
6. Tests: update/replace the 6 existing `declared_member_name_returning`-shaped unit tests in
   `src/rg_tests.rs` to exercise the new function(s) against the SAME shapes they covered before
   (Kotlin fun, Kotlin val, Java method, generic wrapper, parameter-type negative, inferred-local-`val`
   negative) — PLUS add the shapes the old text-scanner silently missed and the final review flagged
   as undocumented gaps, now that they should work for free: a multi-line function signature
   (return type on a line after the closing paren), an extension-receiver function
   (`fun Foo.openBody(): Body`), and a leading-type-param function (`fun <T> openBody(): Body`). Each
   new positive test should assert the fix actually finds the producer (not just that it doesn't
   crash) — construct it so it would have failed under the OLD line-scanning implementation, the way
   PR #324's own tests proved their fixes with genuine red/green evidence, not asserted claims.
7. Also update/replace `producer_scoped_candidate_files`'s existing integration-level tests in
   `src/features/references_tests.rs` if any assert on the OLD file-reading behavior directly (most
   likely they test observable outcomes — found/rejected reference locations — which should be
   unaffected; only touch these if something in them specifically depends on the removed
   implementation detail).

## Global Constraints

- `src/features/references_verify.rs` untouched, as always.
- No behavior change to what gets found vs. not found for any EXISTING passing test — this is an
  internal-mechanism swap (text scan → indexed lookup) that should be a superset of the old
  detection power (strictly fixes the documented gaps), not a narrowing. If any existing test's
  expected result would need to change, treat that as a signal to re-examine your approach, not as
  an acceptable side effect — flag it explicitly in your report rather than silently adjusting the
  test.
- `cargo fmt`, `cargo test --bin kmp-lsp`, `cargo clippy --all-targets -- -D warnings` clean.
- AGENTS.md style rules apply (no abbreviated names, enum/type over comment, no `and` in names, every
  fix has a test, zero-references check before deleting the three old scanner functions).
- Serena's `activate_project` is broken for this worktree this session (known, unrelated) — use plain
  Read/Grep/Edit.
- This updates the ALREADY-OPEN PR #324 (branch `worktree-inferred-receiver-refs`) — commit on top of
  the existing branch tip, do not create a new branch.

## Verification

After the implementer + task review are clean: re-run the real-world Moneta measurement from the
original plan (`isOnline` find-references against `/home/ocel/Work/Moneta/android`, using the same
harness as before) and confirm it still finds the same references (9, per PR #324's own measurement)
— this proves the index-based rework didn't regress the actual real-world bug this whole effort exists
to fix, not just the synthetic tests.
