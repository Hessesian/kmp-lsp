# ViewBinding Navigation — Design & Implementation Plan

> Status: **v1 implemented** on branch `feature/viewbinding-navigation`.
> All product and architecture decisions below were settled in a design session
> with the maintainer; this document remains the normative reference for
> behavior and naming. The implementation plan at the end records the original
> PR sequence used to land v1.

## Goal

Navigate between Android layout XML files (`res/layout*/*.xml`) and their
usage in Kotlin through generated ViewBinding classes — without requiring a
Gradle model, and degrading gracefully when the project has not been built.

| LSP request | From | To |
|---|---|---|
| `textDocument/definition` | type/import `FooBarBinding` (Kotlin) | start of the layout XML file(s) |
| `textDocument/implementation` | type/import `FooBarBinding` (Kotlin) | the generated `FooBarBinding.java` class |
| `textDocument/implementation` | tag `<TextView>` / `<com.x.Custom>` (XML) | the view class definition |
| `textDocument/references` | `binding.field` (Kotlin) | all Kotlin usages of the field |
| `textDocument/definition` | `binding.field` (Kotlin) | the `@+id/field` declaration in the XML |
| `textDocument/references` | `@+id/field` (XML) | all Kotlin usages of the field |
| `textDocument/hover` | `binding.field` (Kotlin) | `val field: <ViewType>` |

### Name-mapping conventions

These are the AGP ViewBinding conventions; kmp-lsp mirrors them exactly:

- **Layout file → class:** `foo_bar.xml` → `FooBarBinding`
  (snake_case → PascalCase, `Binding` suffix appended).
- **View id → field:** `@+id/foo_bar` → `fooBar`
  (snake_case → camelCase).
- **Package:** generated classes live in `<applicationId>.databinding`. The
  package name is never derived by kmp-lsp — it is read from the `package`
  declaration of discovered generated files.

### Out of scope (v1)

- **Data binding** (`<layout>` root tag, binding expressions).
- **Kotlin synthetics** (`kotlinx.android.synthetic.*`) — deprecated upstream.
- **Pure-Java bindings with no layout XML present** — a generated
  `*Binding.java` with no matching layout in the workspace behaves like any
  other indexed Java class; no remapping applies.
- **Intra-XML `@id/...` references** (e.g. `layout_constraintTop_toBottomOf`).
- **`R.id` resolution** in Kotlin code.

### Follow-up TODOs (recorded, not v1)

1. **`R.id` / R class support** — resolve `R.id.foo_bar` in Kotlin to the
   `@+id/foo_bar` declaration, and `R.layout.foo_bar` to the layout file.
   Builds directly on the layout side index introduced here.
2. **Intra-XML references** — `@id/x` usages inside constraint attributes as
   reference results for `@+id/x`.

---

## Product behavior

### Definition: Binding type → layout file(s)

`textDocument/definition` on a `FooBarBinding` type usage or on its
`import com.app.databinding.FooBarBinding` line returns the start of every
layout variant file that generates the class, using an LSP location array.
The default variant (`res/layout/foo_bar.xml`) is ordered first, qualifier
variants (`res/layout-land/`, `res/layout-sw600dp/`, …) after it.

### Implementation: Binding type → generated Java

`textDocument/implementation` on the same positions returns the location of
the generated `FooBarBinding.java` class — the raw Java file under `build/`.
Definition answers "what does this represent"; implementation answers "what
is the actual class the compiler sees".

### Implementation: XML tag → view class

`textDocument/implementation` on a layout XML tag resolves the view class:

- **Fully-qualified tags** (`<com.example.CustomView>`) resolve through the
  existing qualified index.
- **Bare tags** (`<TextView>`) are probed against the standard framework
  prefixes `android.widget.`, `android.view.`, `android.webkit.` — in that
  order — against the index. There is **no hardcoded tag→FQN map**. This
  naturally works when Android SDK sources are configured (kmp-lsp already
  detects them via `detect_android_sdk_source_paths` in
  `src/workspace_json.rs`) and returns nothing when they are not.

### Definition: `binding.field` → `@+id` declaration

Definition on a binding field access targets the `@+id/foo_bar` attribute
position in the layout XML. When several layout variants declare the id, the
declaration in **every variant that declares it** is returned (default
variant first).

For `<include android:id="@+id/header" layout="@layout/view_header"/>`, the
`<include>` tag **is** the id declaration — definition on `binding.header`
targets it. The generated field's type is the included layout's Binding class
(`ViewHeaderBinding`), so chained access `binding.header.title` works through
normal type resolution: the field type is a Binding type like any other, and
the same remapping rules apply recursively. No special chain handling exists
or is needed.

### References: binding fields

`textDocument/references` covers **all Kotlin usages** of a binding field:
qualified (`binding.title`), explicit contextual receivers (`this`, `it`), and
bare implicit-`this` members inside `apply` / `with` / `run` / `also`.
Requesting references from the XML side (`@+id/field`) returns the same result
set.

Precision policy: **prefer false negatives over false positives.** A text
candidate whose receiver cannot be positively resolved to the matching
Binding class is dropped, not included speculatively. A candidate that a nearer
local declaration shadows (see *Contextual receiver resolution*) is likewise
excluded.

### Hover

Hover on `binding.field` renders a Kotlin-style signature synthesized from
the generated Java field:

```kotlin
val title: TextView
val subtitle: TextView?   // when the Java field carries @Nullable
```

Nullability is read from the `@Nullable` annotation on the generated field
(AGP emits it for ids that are absent in some variants). Type names are
short, not fully qualified.

### Diagnostics

Three diagnostics, all emitted on the Kotlin side. **No diagnostics are
emitted in XML files.**

| Condition | Position | Severity | Message (gist) |
|---|---|---|---|
| Layout XML exists, no generated Binding.java found | the `import *.databinding.*Binding` line | Warning | "ViewBinding class not generated — build the project" |
| Layout root tag has `tools:viewBindingIgnore="true"` | the import line | Warning | "layout opts out of ViewBinding (`tools:viewBindingIgnore`)" — distinct from the build-required message |
| a binding field usage resolves in generated Java but the id no longer exists in any live layout variant | the specific stale field usage | Information | "field comes from a stale build; id no longer in layout" |

The stale-field diagnostic covers the same usage forms as navigation:
qualified `binding.field` and bare implicit-`this` members inside
`with` / `apply` / `run` / `also`. It skips declaration names and any usage a
nearer local declaration shadows, so a local `val title` inside
`with(binding) { … }` is never reported as stale.

Definition/hover/references requests stay **silently empty** when the symbol
cannot be resolved at all — exactly like any other unresolved symbol. The
diagnostics carry the explanation for ungenerated or stale bindings; the
requests themselves do not emit warnings.

When **definition** resolves to a generated `*Binding.java` symbol but XML
remapping cannot find a target (no layout variants indexed, field id absent from
every variant, etc.), the **generated Java location is kept** rather than
returning empty. Field remap misses must not fall back to the binding *class*
layout header — only the precise Java symbol location passes through.
**Implementation** is unchanged: it always returns raw Java.

### Staleness model

Two sources of truth, deliberately asymmetric:

- **Generated Java** is the source of truth for **which fields exist and
  their types**. Hover and implementation always reflect the last build.
- **Live XML** is the source of truth for **navigation targets**. If an id
  was renamed in XML after the last build, definition on the old
  `binding.field` returns nothing (the declaration no longer exists), and the
  Info-level staleness diagnostic appears on that usage.

---

## Architecture

### XML parsing

Layout files are parsed with **tree-sitter-xml** (crate `tree-sitter-xml`,
pin `0.6.4` — the last release depending on `tree-sitter ^0.22`, matching the
workspace's `tree-sitter = "0.22"`; `0.7.0` moved to the 0.23 ABI). This is
consistent with the all-tree-sitter codebase: positions, incremental reparse,
and the existing `NodeExt` / cursor-API traversal conventions apply. XML node
kind constants live in `src/queries.rs` like every other grammar's.

XML cursor lookups convert the LSP `position.character` (a UTF-16 code-unit
offset) to a byte offset with `utf16_col_to_byte` before building the
tree-sitter `Point`, exactly as the Kotlin paths do — `Point.column` is a byte
offset, so a line with a multi-byte character before the cursor would
otherwise resolve the wrong node.

### Layout side index (new data model)

Generated Binding.java files are parsed by the **existing Java indexer**
(`parse_java` in `src/parser.rs`) — their symbols land in
`Indexer.definitions` / `Indexer.qualified` as usual, which is why
implementation and hover come mostly for free.

The only new storage is a **layout side index**: one entry per layout XML
file, holding

- module root (see pairing below),
- layout name (`foo_bar`) and variant qualifier (`""`, `land`, `sw600dp`, …),
- root tag name and its range,
- the `tools:viewBindingIgnore` flag,
- declared ids (`@+id/...`) with the view tag name and attribute position,
- `<include>` entries (id, included layout name, tag range).

**Binding-class ↔ layout links are computed at query time** from the
name + module-root convention. They are not stored; there is nothing to
invalidate when either side changes.

A second small side index tracks discovered generated binding files per
module root (class name → generated file URI + mtime), so the query-time link
and the diagnostics can ask "is there a generated class for this layout?"
without re-scanning `build/`.

### Layout variant handling

All `res/layout*` qualifier directories are indexed. The path check is
anchored as a **directory segment** equal to `layout` or matching
`layout-<qualifier>` — a sibling directory like `res/layouts_backup/` does
not match. The parent of the `layout*` segment must be a `res*` directory
segment.

### Generated-file discovery

Generated bindings are found by scanning the module's `build/` directory for
files named `*Binding.java` whose `package` declaration ends in
`.databinding`. AGP output paths are **not hardcoded** — they move between
AGP versions. When multiple build variants (debug/release) contain generated
copies of the same class, the **most recently modified** file wins.

### Module pairing

Module roots are derived purely from path structure:

- A layout at `<X>/src/<sourceset>/res*/layout*/foo_bar.xml` belongs to
  module root `<X>`.
- A generated file at `<X>/build/generated/.../FooBarBinding.java` belongs to
  module root `<X>`.

Layout and binding pair by **layout name + module root**. No Gradle model,
no manifest parsing, no namespace resolution.

### Indexing lifecycle (hybrid)

- **Proactive:** `res/layout*/*.xml` files join the normal workspace scan.
  The crawler (`find_source_files` in `src/indexer/discover.rs`) gains a
  layout-XML branch. `SOURCE_EXTENSIONS` in `src/rg.rs` is **not** extended —
  it feeds `rg`/`fd` reference searches and the resolver fallback, and XML
  must not pollute those. Layout discovery is a separate filter.
- **Lazy + additive:** generated Binding.java discovery runs per module,
  triggered by (a) a `*.databinding.*Binding` import seen while indexing a
  Kotlin file, or (b) a layout XML being indexed. It reuses the additive
  path established by `Indexer::index_source_paths` (`src/indexer/apply.rs`),
  which builds on the unconstrained crawl
  (`find_source_files_unconstrained`) that does **not** exclude `build/`.
  Discovery is idempotent and additive — it never triggers a full rescan.

### Request routing: post-resolution remap

There is **one choke point**, and it sits *after* resolution. The normal
pipeline (`find_definition` in `src/features/definition.rs`,
`find_implementation` in `src/features/implementation.rs`, the resolver in
`src/resolver/resolve.rs`) runs completely unchanged and resolves binding
types and fields to the **real generated Java symbols**.

If — and only if — a resolved location lands inside a generated
`*Binding.java` (identified via the binding side index: under `build/`,
package `*.databinding`), the result is remapped:

| Resolved Java symbol | `definition` returns | `implementation` returns |
|---|---|---|
| the Binding class itself | layout file start, all variants, default first | the raw Java class location |
| an id-backed field | the `@+id` position in every declaring variant | raw Java location |
| an `<include>`-backed field | the `<include>` tag | raw Java location |
| `rootView` field / `getRoot()` | the XML root tag | raw Java location |

**Remap miss (definition only):** when remapping cannot produce XML targets for
a resolved generated-Java symbol, the original Java location is returned
(see *Diagnostics* above for the silently-empty vs Java-fallback split).

Hover uses the Java symbol's type info untouched (plus the Kotlin-style
rendering described above).

One special case: in generated `bind()` bodies the root view's field is
assigned without an `R.id` (AGP emits `rootView` directly). When remapping
that field, the target is recovered **by field name** from the layout side
index rather than by id lookup.

### Contextual receiver resolution

Explicit contextual receivers — `this.field` / `it.field` inside
`apply` / `with` / `run` / `also` — reuse the existing inference machinery in
`src/indexer/infer/` (`RECEIVER_THIS_FNS` in `lambda.rs`, `it_this.rs`,
`receiver.rs`, `cst_lambda.rs`). Binding fields are ordinary Java symbols in
the index and the remap is post-resolution, so those forms fall out for free.

**Bare implicit-`this` members** (`with(binding) { title }`, no `this.`
prefix) are handled explicitly in `CursorContext::build`
(`src/backend/cursor.rs`): a bare lowercase identifier inside a receiver lambda
is treated as `this.<name>` only after ruling out closer bindings.

**Local shadowing is normative.** Kotlin resolves a bare name to the nearest
in-scope `val`/`var`, function parameter, or lambda parameter *before* an
implicit receiver, so a local declaration that shadows a binding field wins.
`Indexer::name_shadowed_by_local_declaration` (`src/indexer/scope.rs`) performs
this CST scope walk, bounded by the enclosing function and by type bodies
(class/object/enum members and top-level declarations never shadow an inner
receiver-lambda member). Navigation, reference verification, and the staleness
diagnostic all consult it, so a shadowed local is never mistaken for a binding
field.

### Freshness

Freshness has two halves, delivered by two different mechanisms:

- **Layout XML** (`**/res/layout*/*.xml`) — handled by handler-side routing
  in `Backend::did_change_watched_files` (`src/backend/mod.rs`):
  create/change reindexes the layout into the side index; delete removes the
  entry. Layout files are tracked source, so client-native file watching
  delivers these events reliably.
- **Generated Binding.java** (`**/build/**/databinding/*Binding.java`) —
  handler-side routing in `did_change_watched_files` is kept as a
  **best-effort** path, but the authoritative mechanism is a **server-side
  watcher** (v1 component): editors' native file watchers typically skip
  gitignored paths, and `build/` is gitignored, so `didChangeWatchedFiles`
  events for generated files usually never arrive — handler routing alone
  would leave the build-required diagnostic stuck after a build.

The server-side watcher mirrors the existing `git_watcher` pattern
(`spawn_git_head_watcher` in `src/backend/git_watcher.rs`: a spawned
background task with a debounced poll loop, triggering reindex work on
change). It polls `<module>/build/**/databinding/` directories, **registered
lazily** — only for modules where additive binding indexing has already run.
When generated `*Binding.java` files appear or change, it re-triggers the
additive binding discovery for that module, so the build-required diagnostic
**self-clears** after a build without a manual reindex. See PR 3 in the
implementation plan.

**Registration hand-off.** The real watcher handle is installed in
`initialized`, but the workspace scan kicked off during `initialize` already
runs binding discovery against the earlier **noop** handle, whose registrations
are dropped. To close that gap, `set_databinding_watcher_handle`
(`src/viewbinding/discovery.rs`) re-registers every already-discovered
module root against the incoming handle, so modules discovered before the
watcher was installed are still polled.

> **Reality note — watcher registration.** The design session originally
> assumed registering the two watch patterns via dynamic
> `client/registerCapability`. kmp-lsp deliberately does **no** dynamic
> capability registration: tower-lsp 0.20 has a panic race in
> `pending.wait()` (documented in `Backend::initialized`,
> `src/backend/mod.rs`). Clients that natively watch files (Zed, Helix) send
> `workspace/didChangeWatchedFiles` without registration; the handler-side
> routing above builds on that, and the server-side watcher covers the
> gitignored `build/` blind spot.

### Caching

The layout and binding side indexes are persisted in the existing disk cache
(`src/indexer/cache.rs`) so warm starts keep navigation instant. This bumps
`CACHE_VERSION` (currently 29); new fields carry `#[serde(default)]` per the
schema-change rule.

---

## Implementation plan

Six stacked PRs, each independently mergeable, each leaving `cargo test` and
`cargo clippy -- -D warnings` green. Every PR includes its own tests (unit
tests in companion `*_tests.rs` files; feature-level tests with temp dirs and
real files; binary integration tests where noted). Integration fixtures
**pre-bake** generated Binding.java files under a fake module `build/`
directory — no real Gradle build runs in CI.

### PR 1 — Layout XML indexing: tree-sitter-xml + layout side index

The foundation: parse layout files, populate the side index, keep it fresh.
No user-visible LSP behavior changes yet (all consumers come later), but the
index is fully queryable and tested.

**Scope**

- Add `tree-sitter-xml = "0.6.4"` to `Cargo.toml`.
- XML node-kind constants in `src/queries.rs` (new XML section).
- Module `src/viewbinding/layout.rs` (+ `layout_tests.rs`):
  - `struct LayoutFileData { module_root: PathBuf, layout_name: String, variant_qualifier: String, root_tag: TagLocation, view_binding_ignore: bool, view_ids: Vec<LayoutViewId>, includes: Vec<LayoutInclude> }`
  - `struct LayoutViewId { id: String, tag_name: String, id_attribute_range: Range }`
  - `struct LayoutInclude { id: Option<String>, included_layout_name: String, tag_range: Range }`
  - `fn parse_layout_xml(content: &str) -> ParsedLayout` — pure function,
    tree-sitter-xml cursor traversal via `NodeExt` helpers.
  - `fn layout_path_components(path: &Path) -> Option<LayoutPathComponents>` —
    the anchored `res*/layout*` segment check plus module-root derivation
    (`<X>/src/<sourceset>/res*/layout*/name.xml` → module root `<X>`,
    layout name, variant qualifier).
- New field on `Indexer`: `viewbinding: ViewBindingState` containing
  `layouts: DashMap<String, Arc<LayoutFileData>>`
  (URI → data), plus read accessors and a
  `fn index_layout_content(&self, uri: &Url, content: &str)` /
  `fn remove_layout(&self, uri: &Url)` write pair on `Indexer`.
- Crawler: layout-file discovery alongside `find_source_files` in
  `src/indexer/discover.rs` (separate function, e.g. `find_layout_files`;
  `SOURCE_EXTENSIONS` untouched). Wire into the scan orchestration
  (`index_workspace_impl` / `index_workspace_prioritized` in
  `src/indexer/scan.rs`) so layouts are indexed during the normal proactive
  scan.
- Watcher: route `res/layout*/*.xml` create/change/delete in
  `Backend::did_change_watched_files` to `index_layout_content` /
  `remove_layout`.
- Cache: persist `layouts` in the disk cache; bump `CACHE_VERSION`;
  `#[serde(default)]` on new fields.

**Tests**

- `parse_layout_xml`: ids with positions, root tag, `viewBindingIgnore`
  true/false/absent, `<include>` with and without id, malformed XML (no
  panic, best-effort result).
- `layout_path_components`: default variant, qualifier variants, rejection of
  `res/layouts_backup/`, rejection of non-`res` parents, module-root
  derivation for nested modules.
- Discovery test in `discover_tests.rs`: layout files found, `build/` still
  excluded from the constrained crawl.
- Watcher test: change updates the side index entry; delete removes it.
- Cache round-trip test: layouts survive save/load.

### PR 2 — Generated-binding discovery + module pairing

Find generated `*Binding.java` files, index them through the normal Java
pipeline, and record the layout↔binding pairing inputs. Depends on PR 1 (uses
module-root derivation and the layout side index as a trigger).

**Scope**

- Module `src/viewbinding/discovery.rs` (+ tests):
  - `struct GeneratedBindingEntry { class_name: String, file_uri: String, modified_at: SystemTime }`
  - `fn discover_generated_bindings(module_root: &Path) -> Vec<GeneratedBindingEntry>` —
    walks `<module_root>/build/` for `*Binding.java`, verifies the `package`
    declaration ends in `.databinding`, prefers most-recent mtime per class
    name across build variants.
  - `fn module_root_for_generated_file(path: &Path) -> Option<PathBuf>` —
    the `build/` segment counterpart of PR 1's layout derivation.
- New field on `Indexer`: `generated_bindings: DashMap<PathBuf, Arc<ModuleBindings>>`
  (module root → discovered entries), with an additive
  `fn index_generated_bindings(&self, module_root: &Path)` that discovers and
  feeds each file through the existing `index_content` (Java parse — symbols
  land in `definitions` / `qualified` as usual).
- Triggers:
  - Kotlin indexing (`src/indexer/apply.rs`): when a file's imports contain
    `*.databinding.*Binding`, enqueue additive discovery for the importing
    file's module. Follow the deferred pattern — no blocking inside the apply
    path (precedent: `src/indexer/enrich.rs` background worker,
    `index_source_paths` additive flow).
  - Layout indexing (PR 1 path): a layout joining the index enqueues
    discovery for its module root.
- Query-time link helpers (pure `&self` reads):
  - `fn binding_class_name_for_layout(layout_name: &str) -> String` and the
    inverse `fn layout_name_for_binding_class(class_name: &str) -> Option<String>`
    (snake_case ↔ PascalCase mapping).
  - `fn layouts_for_binding_class(&self, class_name: &str, module_root: &Path) -> Vec<Arc<LayoutFileData>>`
    (default variant first).
  - `fn is_generated_binding_uri(&self, uri: &str) -> bool` — the remap
    predicate used by PR 4.
- Watcher (best-effort half): route `build/**/databinding/*Binding.java`
  events arriving via `did_change_watched_files` to additive re-discovery for
  the owning module. The authoritative server-side watcher is PR 3.
- Cache: persist `generated_bindings`; `CACHE_VERSION` bump if not already
  covered by PR 1's bump landing in the same release.

**Tests**

- Discovery against a temp-dir fake module: finds nested AGP-style paths
  without hardcoding them, rejects `*Binding.java` outside `.databinding`
  packages, picks the newer of debug/release copies.
- Name mapping round-trip: `foo_bar` ↔ `FooBarBinding`, single-word layouts,
  digits in names.
- Import-triggered additive indexing: indexing a Kotlin file with a
  databinding import makes the generated class resolvable; a competing
  non-generated class named `FooBarBinding` in normal sources is not
  misclassified (`is_generated_binding_uri` false).
- Watcher test: touching a fixture Binding.java under `build/` re-indexes it.

### PR 3 — Server-side databinding watcher

Editors' native watchers skip gitignored paths, and `build/` is gitignored —
so without this watcher a build would neither refresh the binding index nor
(later) clear the build-required diagnostic. Small, self-contained PR on top
of PR 2's `index_generated_bindings`. Placed here rather than inside PR 2 to
keep PR 2 reviewable; placed before the diagnostics PR so that self-clearing
is already proven infrastructure by the time diagnostics land.

> **Reality note.** `git_watcher.rs` has no companion test file, so there is
> no test precedent to copy; this PR sets it, using temp dirs plus the
> `poll_until` helper pattern from `src/workspace/actor_tests.rs`.

**Scope**

- Module `src/viewbinding/watcher.rs`
  (+ `watcher_tests.rs`):
  - `fn spawn_databinding_watcher(indexer: Arc<Indexer>) -> DatabindingWatcherHandle` —
    spawns a tokio task with a debounced poll loop (same shape as
    `spawn_git_head_watcher` in `src/backend/git_watcher.rs`, 2-second
    interval). Each tick scans registered module roots for `*Binding.java`
    files under a `databinding` directory segment, compares mtimes against a
    per-module snapshot, and calls `index_generated_bindings` when changes
    are detected.
  - `struct DatabindingWatcherHandle { watched_module_roots: Mutex<HashSet<PathBuf>>, … }`
    with `fn watch_module(&self, module_root: &Path)` — idempotent; registers
    the module for polling. First-build subtlety: `build/` may not exist yet
    (that is exactly the self-clear scenario) — polling simply finds nothing
    until it appears.
  - After a re-discovery completes, open-file diagnostics are re-published
    via the existing `republish_open_file_diagnostics`
    (`src/workspace/document_handler.rs`) so PR 6's diagnostics self-clear
    with no further wiring.
- Lazy registration at the write site (rule: side effects live in the write
  helper, not at call sites): `index_generated_bindings` (PR 2) calls
  `watch_module` on its first run per module. No caller can forget to
  register the watch.
- **No new dependencies** — the poll loop matches the established
  `git_watcher` pattern.

**Tests**

- `watch_module` idempotence: second call for the same module adds no
  duplicate registration, triggers no duplicate re-discovery.
- End-to-end trigger: write a fixture `FooBarBinding.java` under
  `<temp module>/build/generated/…/databinding/` after the watcher is
  registered; `poll_until` the class is resolvable in the index.
- Filtering: writes to `build/tmp/whatever.txt` and to a `*Binding.java`
  outside any `databinding` segment trigger nothing.
- First-build case: register while `build/` is absent, then create
  `build/…/databinding/FooBarBinding.java` — still triggers.
- Debounce: a burst of rapid writes coalesces into one re-discovery
  (assert via parse/discovery counters, `parse_count` precedent).

### PR 4 — Post-resolution remap: definition + implementation

The user-visible navigation payoff. Depends on PR 2 (PR 3 is not required
for navigation, only for freshness).

**Scope**

- Module `src/viewbinding/navigation.rs` (+ tests):
  - `fn remap_generated_binding_locations(index: &…, locations: Vec<Location>) -> Vec<Location>` —
    the single choke point. For each location inside a generated Binding.java
    (via `is_generated_binding_uri`): class → layout file start(s); id-backed
    field → `@+id` positions in every declaring variant; `<include>`-backed
    field → `<include>` tag; `rootView` / `getRoot` → XML root tag (recovered
    by field name, covering the `bind()` root-field special case). Non-binding
    locations pass through untouched.
  - Applied in the definition path only (`find_definition` results in
    `src/features/definition.rs` / its backend adapter);
    `find_implementation` (`src/features/implementation.rs`) deliberately
    returns the raw Java location.
- XML-side requests: the backend currently only tracks Kotlin/Java/Swift
  documents. Add minimal XML document handling (didOpen/didChange routing in
  `src/workspace/document_handler.rs` / `file_change_handler.rs` feeding
  `index_layout_content`) and position lookup into the layout side index:
  - `textDocument/implementation` on a view tag → fully-qualified tags via
    the qualified index; bare tags probed with `android.widget.`,
    `android.view.`, `android.webkit.` prefixes. Empty result when SDK
    sources are not indexed.
  - `textDocument/definition` on an `@+id` attribute → the same id's
    declaration across variants (self + siblings), enabling variant hopping.
- Chained include access (`binding.header.title`): no code — covered by a
  test proving the field's declared type `ViewHeaderBinding` resolves through
  the normal pipeline and remaps recursively.

**Tests**

- Feature tests with pre-baked fixtures (layout XML + fake generated Java
  under `build/`): definition on import/type → layout file(s), variant
  ordering (default first); definition on `binding.field` → `@+id` in all
  declaring variants; `binding.header` → `<include>` tag; `getRoot()` → root
  tag; implementation on the type → raw Java.
- Competing-definition tests: a hand-written class named `FooBarBinding`
  outside `build/` is never remapped; a layout in module A does not pair with
  a generated class in module B.
- Contextual receivers: `with(binding) { title }`, `binding.apply { title }`,
  `it.title` — definition remaps identically (exercising decision that no new
  inference work is needed).
- XML-side: implementation on `<TextView>` with SDK sources indexed
  (fixture-simulated) and without (empty, no error); fully-qualified custom
  view tag.
- Binary integration test (`tests/lsp_smoke.rs` pattern: fixture files +
  `workspace.json` with `{"sourcePaths":[]}` + LSP JSON-RPC) covering
  definition Kotlin→XML end-to-end.

### PR 5 — Hover nullability + receiver-verified references

Depends on PR 4 (uses the same binding identification; references reuse the
remap plumbing for the XML entry point).

**Scope**

- Hover: when the resolved symbol is a generated binding field, render
  Kotlin-style `val field: TextView` / `val field: TextView?`. Read
  `@Nullable` from the generated Java field at parse time (extend the Java
  extraction in `src/parser.rs` — `extract_java` — to record the annotation
  on the field's `SymbolEntry.detail`, or read it lazily from the file in the
  hover path; decide by measuring parse cost). Rendering slots into the
  existing pipeline: `compute_hover` (`src/features/hover.rs`) →
  `resolve_symbol_info` (`src/indexer/resolution.rs`) →
  `format_symbol_hover` (`src/backend/format.rs`). Short type names.
- References, two-stage:
  1. Candidate gathering: existing machinery —
     `rg_find_references` (`src/rg.rs`) gathers `.fieldName` text candidates,
     import-graph narrowing and live-buffer injection as today
     (`find_references_with_qualifier` in `src/features/references.rs`).
  2. Receiver verification: for each candidate, resolve the receiver's type
     through existing `resolve_qualified` / variable-type inference
     (`src/resolver/resolve.rs`, `src/indexer/infer/`); keep only receivers
     resolving to the matching Binding class. Unresolvable receivers are
     dropped (false-negative bias).
- XML-side references: `textDocument/references` on `@+id/field` maps id →
  field name via the naming convention, then runs the same verified pipeline.
- Generated-file URIs are excluded from reference results (consistent with
  the existing Library/sourcePaths exclusion in `references.rs`).

**Tests**

- Hover: non-nullable and `@Nullable` fields; include-typed field hovers as
  `val header: ViewHeaderBinding`; short names asserted, FQNs asserted absent.
- References: qualified usages found; `apply`/`with`/`it` usages found; a
  **misleading competitor** — another class with an identically-named `title`
  property — is not included (the regression-proving test); unresolvable
  receivers dropped.
- XML → references round-trip equals Kotlin-side results.

### PR 6 — Diagnostics: build-required, viewBindingIgnore, staleness

Depends on PRs 1–3 (needs both side indexes, and the PR 3 watcher for
self-clearing); independent of PRs 4–5 at the code level, sequenced last
because it is pure polish on top of proven navigation.

**Scope**

- Module `src/viewbinding/diagnostics.rs` (+ tests):
  - `fn viewbinding_import_diagnostics(index: &…, uri: &Url) -> Vec<Diagnostic>` —
    for each `*.databinding.*Binding` import in the file: if the paired
    layout exists but no generated class is discovered → Warning
    "ViewBinding class not generated — build the project"; if the layout sets
    `tools:viewBindingIgnore="true"` → the distinct opt-out Warning. No
    layout and no generated class → nothing (plain unresolved import,
    existing behavior).
  - `fn stale_binding_field_diagnostics(index: &…, uri: &Url, document: &LiveDoc) -> Vec<Diagnostic>` —
    Information severity on each `binding.field` usage whose field exists in
    the generated Java but whose id is absent from every live layout variant.
- Wire both into the two diagnostic publication sites
  (`src/workspace/document_handler.rs` and
  `src/workspace/file_change_handler.rs`, alongside `call_arg_diagnostics` /
  `nullable_dot_call_diagnostics` / `missing_package_diagnostic`), inside the
  existing `spawn_blocking` blocks and behind the same
  `indexing_in_progress` suppression.
- Self-clearing: no new code — the PR 3 server-side watcher re-discovery
  (plus PR 2's best-effort handler routing) already re-publishes open-file
  diagnostics via `republish_open_file_diagnostics`, which clears the
  build-required diagnostic after a build. Covered by a test.

**Tests**

- Import diagnostics: layout present + no generated class → build-required;
  `viewBindingIgnore` layout → opt-out message (and **not** build-required);
  generated class present → no diagnostic; import of a non-existent binding
  with no layout → no new diagnostic.
- Staleness: field in generated Java, id removed from all variants → Info on
  the usage position; id present in one variant only → no diagnostic; XML
  side stays diagnostic-free.
- Self-clear integration test: fixture starts without generated file
  (diagnostic present), file is written + watcher event delivered →
  diagnostic gone on re-publish.

### Dependency order

```
PR 1 (layout index)  →  PR 2 (binding discovery)  →  PR 3 (server-side databinding watcher)
                                                  →  PR 4 (remap: definition/implementation)
                                                  →  PR 5 (hover + references)
                                                  →  PR 6 (diagnostics)
```

PRs 3–6 all depend on PR 2. PR 5 additionally reuses PR 4's XML-side
document handling, and PR 6 relies on PR 3 for diagnostic self-clearing, so
the merge order is 1 → 2 → 3 → 4 → 5 → 6 per the standard stacked-PR
workflow (merge base first, rebase dependent, `cargo build && cargo test` on
main after each merge).
