# Changelog

## Unreleased

### Features

- **Go-to-definition into library source actually opens the file** — jumping into a dependency that ships a `*-sources.jar` now opens the real source. The relevant entry is extracted on demand to a read-only file under `~/.cache/kmp-lsp/jar-sources/` and returned as a `file://` location the editor can open; previously these resolved to non-openable `jar:` URIs (so hover/completion worked, but the jump didn't). Compiled-only jars with no sources still show the signature.
- **`jarPaths` — index compiled jars without a build system** — for projects with no Gradle cache (Make/Bazel/manual builds), point the indexer directly at compiled `.jar`/`.aar` dependencies. Set `jarPaths` in `workspace.json` (`"jarPaths": ["<WORKSPACE>/libs", "/opt/deps/foo.jar"]` — files or directories, `<WORKSPACE>` and relative paths supported) or via LSP `initializationOptions.indexingOptions.jarPaths`. Indexed in addition to the Gradle cache; hover docs are read from a sibling `*-sources.jar` when one is present.
- **Accurate library resolution via real per-symbol packages** — JAR symbols now carry their true package (emitted by the sidecar), so go-to-definition, hover, completion, and call-arg diagnostics bind to the *imported* overload instead of an arbitrary same-named symbol from an unrelated jar (e.g. compose `remember`, `stringResource`). Library hover JavaDoc/KDoc now renders for annotated and generic declarations (`@Composable`, `remember`, `stringResource`, …).

### Bug fixes

- **Qualified member-method arg diagnostics** — `receiver.method()` calls into a member defined in another file/package are now arg-count-checked (previously skipped because the method's file was wrongly gated by import reachability).
- **JAR indexing no longer stuck "loading"** — a cancelled or coalesced background JAR scan can no longer leave open-file diagnostics suppressed indefinitely.
- **Generic angle brackets in hover docs** — `List<String>` / `Map<K, V>` in KDoc/JavaDoc are no longer stripped as if they were HTML tags.

## 0.24.0

### Features

- **Deprecated & internal completion filtering** — `@Deprecated` library symbols are now hidden from completion (e.g. kotlinx-coroutines' `launch(context: Job)` / `launch(context: NonCancellable)` guidance shims). `@Deprecated` workspace symbols are kept but marked with the LSP `Deprecated` tag and sorted to the bottom. `internal` members of libraries — inaccessible from your module — are also hidden. Deprecation is captured both from sources JARs (tree-sitter) and the compiled-JAR sidecar (reads the `@Deprecated` annotation from bytecode via ASM, matched to declarations by JVM signature).
- **Completion overload dedup** — overloads of the same extension collapse to a single entry per name (plus its `name { }` trailing-lambda form), matching IDE behaviour. Eliminates duplicate `launch` / `launch { }` rows caused by version skew across cached library versions and by sources-vs-compiled JAR copies of the same function.
- **Precise find-references for JAR/library symbol usages** — invoking `textDocument/references` on a usage of a JAR-defined symbol (e.g. Compose's `remember`) now scopes the search to workspace files that import the symbol's declaring package/type, instead of an unscoped codebase-wide search. An unrelated workspace `fun remember()` in another package is no longer returned as a false positive.

### Bug fixes

- **Indexing deadlock on reindex** — `reset_index_state` could self-deadlock when an extension receiver carried both library and workspace entries: it inserted into a `DashMap` while iterating it, taking a re-entrant lock on the same shard. This manifested as a stuck progress bar and frozen completions on large workspaces. Inserts are now deferred until the iterator is dropped.
- **Workspace cache clobbered by a near-empty save** — a save firing during a reindex's reset window (notably the JAR-indexing pass) could overwrite the full on-disk cache with a 3-file one, forcing a cold rescan on every subsequent startup. The JAR pass no longer writes the workspace cache, and `save_cache` refuses to shrink a populated cache unless the caller is the authoritative scan finalizer. The `scan.lock` sentinel was removed — atomic cache writes already guarantee on-disk integrity.
- **Stale bare-name cache after apply** — a completion request arriving mid-apply could leave the bare-name cache reflecting a partial index; the final rebuild is now forced to run against the complete symbol set.

## 0.23.0

### Features

- **`textDocument/onTypeFormatting`** — pressing Enter now triggers smart indentation in Kotlin files. Three cases handled: (1) Enter after `{` with an auto-paired `}` on the cursor line splits the pair into a properly indented block; (2) Enter after `{` with no closing brace on the line corrects the new line's indentation to one level deeper than the opening; (3) Enter inside `///`, `//!`, `/**`, `* `, or `//` comment lines continues the comment prefix automatically. No-op when the editor already inserted the correct prefix. Respects `FormattingOptions.insert_spaces` / `tab_size` from the client.

## 0.22.1

### Bug fixes

- **Introduce local variable on property access** — extracting a variable from a plain property access like `repo.userData` (no call parens) now correctly produces `val userData = repo.userData` instead of the broken `val userData = userData`. The CST expansion now walks to the outermost `navigation_expression` in addition to `call_expression`.
- **Rename scoped to function when cursor is on local variable reference** — renaming a local variable when the cursor was on a *reference* (not the declaration) incorrectly fell through to the global rename path, renaming same-named symbols in other classes (e.g. sealed class constructor parameters). Now correctly detects the local declaration in scope and limits the rename to the enclosing function body.

## 0.22.0

### Features

- **`kmp-lsp check <file|dir>…`** — new subcommand for instant tree-sitter syntax validation without an LSP session or index. Accepts files and directories (walks `.kt`, `.kts`, `.java`, `.swift`). Exits 1 on any error. Optional `--json` output with structured `{files_ok, files_with_errors, errors}` payload.
- **`kmp-lsp refs --exclude-imports`** — new flag that strips import-statement matches from `refs` output. Useful for common names (`Event`, `Result`, `State`) where import lines otherwise flood results.
- **`kmp-lsp diagnose` now reports syntax errors** — tree-sitter syntax errors are now surfaced alongside call-arg and `when` diagnostics instead of being silently dropped.
- **README: AI agent integration section** — top-level section with one-liner to install the Copilot skill from the repo, linking to full agent setup docs.

### Bug fixes

- **Parser: suppress `@file:[Ann1, Ann2]` comma false positive** — tree-sitter-kotlin 0.3 emits spurious ERROR nodes for commas inside `@file:[…]` bracket annotation syntax. These are now filtered at the current-line level to avoid inadvertently suppressing real errors elsewhere in the file.
- **Windows: `Path::canonicalize` UNC prefix** — `canonicalize()` on Windows returns `\\?\`-prefixed extended-length paths that confused `rg` and the rg output parser. The prefix is now stripped after canonicalization and `split_rg_fields` handles it as a fallback.

## 0.21.0

### Features

- **Sources-JAR auto-mount** — `*-sources.jar` files are now unpacked in-memory at startup directly from the Gradle module cache (`~/.gradle/caches/modules-2/files-2.1`). Go-to-definition into library source code (Compose, AndroidX, Kotlin stdlib, …) works automatically without running `kmp-lsp extract-sources`. The manual `extract-sources` step is no longer needed for most projects; it remains available as a fallback for cases where Gradle sources are not cached locally.
- **Sources-JAR parse cache** — parsed sources JARs are fingerprinted and cached on disk. On subsequent startups, unchanged JARs are loaded from the cache instead of re-parsed, making cold-start indexing significantly faster for large Gradle caches.
- **Semantic tokens: import highlighting** — `import` statements now receive a `NAMESPACE` semantic token over the full dotted path (e.g. `java.util.Scanner`, `androidx.compose.ui.Modifier`) in both Kotlin and Java files.
- **Diagnostics off async thread** — `when`-branch diagnostics and call-argument diagnostics are now computed inside `tokio::task::spawn_blocking`, keeping the async runtime free during CPU-bound work. Reduces stutter on large files.

### Bug fixes

- **Extension function diagnostics** — call-argument diagnostics now correctly resolve extension functions: receiver type is matched against the extension's first implicit parameter, preventing false-positive "missing argument" errors on extension calls.
- **Generic type inference in diagnostics** — `when` branch type member resolution no longer varies by query order; type cache always resolves with an empty branch set to eliminate key-collision instability.
- **Inlay hints: `::class` arguments** — inlay hints now infer the correct type from `SomeClass::class` literal arguments by delegating to `resolve_call_expr_type`.
- **JAR symbols in go-to-def** — JAR-indexed symbols are now reachable through all lookup paths: explicit import, qualified, and unqualified. Package is correctly extracted from sidecar detail.
- **Copy() inside receiver lambdas** — `copy()` calls inside lambda receivers are no longer incorrectly attributed to JAR symbols, preventing noise in hover and diagnostics.
- **install.sh** — rewrote with `#!/usr/bin/env bash` shebang (was `/bin/sh`), correct `gunzip` decompression for `kmp-jar-indexer`, and literal-match checksum lookup via `awk` (was `grep` with regex hazard on dots).

## 0.20.0

### Rename: kotlin-lsp → kmp-lsp

Project renamed to **kmp-lsp** (Kotlin Multiplatform Language Server) to avoid
confusion with JetBrains' official `kotlin-language-server` and to reflect the
multi-language scope (Kotlin, Java, Swift).

- Binary: `kotlin-lsp` → `kmp-lsp`
- Sidecar: `kotlin-jar-indexer` → `kmp-jar-indexer`
- Config dir: `~/.config/kotlin-lsp/` → `~/.config/kmp-lsp/`
- Cache dir: `~/.cache/kotlin-lsp/` → `~/.cache/kmp-lsp/`
- Sources dir: `~/.kotlin-lsp/sources` → `~/.kmp-lsp/sources`
- LSP commands: `kotlin-lsp/reindex` → `kmp-lsp/reindex`
- Env vars: `KOTLIN_LSP_*` → `KMP_LSP_*`
- VS Code setting: `kotlinLsp.path` → `kmpLsp.path`

**Migration steps:**
```sh
mv ~/.config/kotlin-lsp ~/.config/kmp-lsp
mv ~/.cache/kotlin-lsp ~/.cache/kmp-lsp
mv ~/.kotlin-lsp ~/.kmp-lsp
```
Update your editor config to use `kmp-lsp` as the binary name.

### Features

- **Windows support** — full cross-platform path normalisation; `x86_64-pc-windows-msvc`
  and `aarch64-pc-windows-msvc` targets added to the CI matrix; Windows `.zip` artifacts
  and platform-specific `.vsix` packages included in releases.
- **Install scripts** — `install.sh` (Linux/macOS) and `install.ps1` (Windows) automate
  binary download, checksum verification, and PATH setup.

---

## Migration: kotlin-lsp → kmp-lsp

Starting with this version the project is renamed to **kmp-lsp**.

**What changed:**
- Binary: `kotlin-lsp` → `kmp-lsp`
- Sidecar: `kotlin-jar-indexer` → `kmp-jar-indexer`
- Config: `~/.config/kotlin-lsp/` → `~/.config/kmp-lsp/`
- Cache: `~/.cache/kotlin-lsp/` → `~/.cache/kmp-lsp/`
- Sources dir: `~/.kotlin-lsp/sources` → `~/.kmp-lsp/sources`
- LSP commands: `kotlin-lsp/reindex` → `kmp-lsp/reindex`, `kotlin-lsp/clearCache` → `kmp-lsp/clearCache`
- Env vars: `KOTLIN_LSP_*` → `KMP_LSP_*`

**Migration steps:**
```sh
mv ~/.config/kotlin-lsp ~/.config/kmp-lsp
mv ~/.cache/kotlin-lsp ~/.cache/kmp-lsp
mv ~/.kotlin-lsp ~/.kmp-lsp
```
Update your editor config to use `kmp-lsp` as the binary name.

---

## 0.19.1

### Bug fixes

- **Diagnostics no longer flash** — removed the immediate empty-list clear sent on every keystroke. The debounced reindex already guards against stale diagnostics via a generation counter, so the clear was redundant and caused the flash. Fixes #152.
- **Android build fixed** — jemalloc is now disabled on Android (as it is on Windows), preventing a build crash when targeting `aarch64-linux-android`. Fixes #151.
- **VS Code extension: darwin-x64 sidecar** — the `kmp-jar-indexer` for darwin-x86_64 now correctly falls back to the aarch64 binary (which runs via Rosetta 2 on Intel Macs).

## 0.19.0

### Features

- **GraalVM native sidecar (`kmp-jar-indexer`)** — ships as a self-contained native binary (no JVM required). Indexes JAR/AAR files and returns full symbol + doc metadata in ~4 ms startup. Built via GraalVM native-image on all 4 platforms (linux-x64, linux-arm64, macOS-x64, macOS-arm64). Falls back to `java -jar` automatically if the native binary is absent but `java` is on PATH.
- **`CompletionContext` struct** — centralises all cursor-position analysis (receiver, scope, lambda/annotation context, named-arg detection) into a single `analyse()` pass. Replaces 5–6 independent text/CST walks per completion request with one. Modelled after rust-analyzer's `CompletionContext`.
- **`JarPhase` enum** — makes JAR indexing state explicit: `Unavailable → Discovering → Indexing(n/total) → Ready(count) → Failed`. Observable by all features; replaces ad-hoc boolean flags.
- **`CursorContext` struct** — unifies cursor-position analysis for `textDocument/references` and `textDocument/implementation`. Eliminates duplicated line-scan logic across backend handlers.
- **Backend decomposition** — `src/backend/mod.rs` (was 837 lines) split into 6 focused modules: `panic_guard`, `progress`, `helpers`, `capabilities`, `commands`, `init`. Each has a single reason to change.

### Distribution

- **`install.sh`** — one-liner installer (`curl -fsSL .../install.sh | sh`) that downloads the combined tarball, verifies SHA256 checksum, and installs both `kmp-lsp` + `kmp-jar-indexer` to `~/.cargo/bin`. Supports `--version` flag and `INSTALL_DIR` override.
- **`sha256sums.txt`** — every release now includes a checksum file for all artifacts.
- **cargo-binstall** — `cargo binstall kmp-lsp` supported via `[package.metadata.binstall]` in `Cargo.toml`.
- **mason.nvim registry** — `contrib/mason-registry/package.yaml` ready for submission to `mason-org/mason-registry`.
- **aqua/mise registry** — `contrib/aqua-registry/registry.yaml` ready for submission; `mise use aqua:Hessesian/kmp-lsp` installs both binaries.
- **Release CI** — new `build-sidecar` job compiles native binary on all 4 platforms. Combined tarballs (`kmp-lsp-{platform}.tar.gz`) bundle both binaries; per-binary `.gz` files provided for mason.nvim-style installs.

### Bug fixes

- **`install.sh` checksum on macOS** — uses `command -v sha256sum` / `shasum` detection instead of a broken pipeline fallback.
- **aqua-registry asset template** — fixed `{{.OS}}-{{.Arch}}` order (was reversed).

## 0.18.0

### Features

- **Named argument completion** — typing `param =` inside a call expression now shows completion items for the remaining unset named parameters of the callee. Works for same-file functions, cross-file imports, and data class primary constructors (including cross-package). Triggered when the cursor is after a `,` inside a call and the prefix contains `=` or an identifier prefix. Fixes #124.

### Bug fixes

- **Signature help survives unclosed parentheses** — `UserData(bookmarkedNewsResources = setOf(),  ` (no closing `)`) no longer silently drops signature help. A text-based fallback scans backward for the innermost unmatched `(` when the CST has no complete call node.
- **Signature help correct active param with generics** — `split(',')` was incorrectly splitting `Map<K, V>` params at the inner comma. Active-parameter index now uses depth-zero comma counting.
- **Signature help in nested calls** — cursor inside `foo(bar(` now shows `foo`'s signature via outer-call fallback when the inner callee cannot be resolved.
- **No signature help inside function definitions** — `fun greet(ha: String, ` no longer triggers spurious signature help from the text-based fallback. The CST walk now detects `function_value_parameters`, `primary_constructor`, and `formal_parameters` as definition context and suppresses the fallback.
- **Call-arg diagnostics fire for named-arg calls** — `greet(ha = "", )` with a missing required parameter is now flagged. The previous blanket "skip if any named arg" guard has been removed; arity checks are valid for named-arg call sites too.
- **References: nested type false positives eliminated** — `IntroContract.Event` references no longer appear in files that import an unrelated `Event` class from a different package. Candidate discovery now uses FQN-aware import matching via the index.
- **Java abstract/interface method declarations** — `void process(String s);` (no body, ends with `;`) is now correctly recognised as a declaration and excluded from reference results. Previously only `{`-body and `throws`/`default` forms were handled.
- **Dot-completion prefers class/object over function** when names collide.



### Bug fixes

- **Annotation completion: `@` now keeps session open** — typing `@` alone no longer returned an empty list, which caused editors (Zed, VS Code, Neovim) to close the completion session. The cross-package scan is now also triggered on empty prefix when in annotation context. Fixes #122.
- **Annotation completion: no more `@Composable()`** — accepting an annotation class no longer inserts unwanted parentheses. The snippet `is_fn` guard now checks `annotation_only` and suppresses the `($1)` suffix. Fixes #122.
- **Annotation completion: stdlib items no longer leak** — functions (`println`, `listOf`, `forEach`), live templates (`fun`, `class`), and other non-annotation stdlib items no longer appear in annotation completion lists.

## 0.17.1

### Bug fixes

- **Rename keyword guard (Java/Kotlin split)** — `KOTLIN_KEYWORDS` and `JAVA_EXTRA_KEYWORDS` are now separate sorted arrays. `.kt`/`.kts` files only block Kotlin reserved words; `.java` files block both sets. Previously, Java-only reserved words (`switch`, `void`, `int`, `static`, `new`, etc.) were incorrectly blocked as rename targets in Kotlin files.

## 0.17.0

### Features

- **Missing package declaration diagnostic** — files with no `package` statement now show a Warning diagnostic on the first line. The warning spans at least the word `package` (7 chars) so it's visible in the gutter. A `Add missing package declaration` code action is available to insert the correct package derived from the file path.
- **Java package statement** — the code action correctly emits `package foo.bar;` (with trailing semicolon) for Java files; Kotlin files get `package foo.bar` without.
- **KMP source-set roots** — package inference now recognises `jsTest`, `nativeTest`, `jsMain`, and `nativeMain` alongside the standard `commonMain`/`androidMain`/etc. roots.

### Bug fixes

- **Autocomplete single-char prefix** — typing the first character of a class name (e.g. `E` for `EditText`) no longer returns an empty list. Previously the cross-package scan was gated behind a 2-char minimum, causing the editor to close the completion session before any symbols appeared. Single-char uppercase prefixes now trigger a score-0 (starts-with) match; camel-acronym matching still requires ≥2 chars to avoid noise. Fixes #117.

## 0.15.0

### Features

- **`textDocument/implementation`** — `go to implementation` now works for interface methods and abstract functions. Finds all concrete override sites across the workspace, handling Kotlin `override fun`, Java `@Override`, and abstract class methods. Scoped by declaring class to avoid false positives from same-name methods in unrelated classes.
- **Field and property reference scoping** — `find references` for `val`/`var`/Java fields is now scoped to files that reference the declaring class, eliminating false positives from same-named fields in unrelated classes. Declaration lines in other files are filtered out; override declarations in subtypes are kept.
- **`ThisContext` enum** — `this` type inference in receiver lambdas now returns a tri-state (`Resolved`, `InsideReceiver`, `NotFound`) instead of `Option<String>`. Callers can distinguish "inside an `apply`/`run`/`with` lambda with unknown receiver type" from "not in any receiver lambda", preventing incorrect fallback to `enclosing_class_at`.
- **Generic type substitution in lambda `it`** — `it` now resolves to the concrete element type when the receiver is a generic container (e.g. `result.getOrNull()?.also { it.field }` → `it: FamilyAccount` when `result: Result<FamilyAccount>`). Works for extension functions and multi-hop chains.
- **CST-first lambda parameter resolution** — lambda parameter type inference uses the live tree-sitter CST as the primary path. Falls back to text-scan only when no live document is available.
- **`fill_when` diagnostics and code action** — detects missing branches in `when` expressions over sealed classes and enums, and offers a "Fill missing branches" code action. Handles `is` branches, object branches, Boolean exhaustion, and smart-cast type narrowing in `when`/`is` branches.
- **Missing argument diagnostics** — call expressions with too few or mismatched arguments are flagged. Handles default parameters, varargs, `@JvmOverloads`, and Java constructor overloads.
- **Synthetic enum members** — `.entries`, `.values()`, `.valueOf()`, `.name`, `.ordinal` resolve correctly in go-to-definition, hover, and completion.
- **`infer_expr_type` extended** — expression type inference now covers boolean operations, if-expressions, range literals, and single-expression function return types. Powers inlay hints for return types.
- **Async rg enrichment with debounced inlay hint refresh** — inlay hints trigger a background `rg` pass to enrich unresolved types; results are pushed to the client via debounced refresh rather than blocking the initial response.
- **Panic-safe LSP handlers** — every LSP request handler is wrapped in a `catch_unwind` boundary. Panics produce a structured crash report (file, line, backtrace fragment) logged via `RUST_LOG` instead of crashing the server.
- **`params` field on `SymbolEntry`** — function/method symbols now carry their parameter list (extracted from CST at index time), enabling accurate call-site arity checks without an rg round-trip.
- **`container` field on `SymbolEntry`** — every symbol now records its enclosing class/object name, enabling tighter scoping in cross-file resolution.

### Performance

- **Chunked library cache** — library index (`~/.kmp-lsp/sources`, Android SDK) is saved as 20 MB chunks instead of one large file, eliminating the end-of-indexing memory spike and enabling streaming load on startup.
- **Streaming library cache load** — chunks are deserialised and applied incrementally; peak RSS during warm start is now proportional to one chunk rather than the full library index.
- **jemalloc allocator** — switched to `tikv-jemallocator` on Linux/macOS for lower fragmentation on the DashMap-heavy workload; ~15–20% RSS reduction on large Android projects.
- **Signature lookup cache** — repeated `rg` calls to resolve the same function signature are deduplicated via an in-memory cache; measurable speedup on files with many call expressions.
- **Worker thread scaling** — Tokio worker threads now scale to available CPU cores.
- **fill_when subtype scan dedup** — sealed-class subtype discovery is cached for the duration of a single diagnostics pass.

### Bug fixes

- **CLI warm-start only seeing 56 files** — `resolve_root` was returning a relative path (`"."`) when invoked from the workspace directory, causing all workspace source files to be misclassified as library URIs and omitted from the on-disk cache. Warm starts now correctly see all indexed files.
- **Memory regression after cache fix** — after the root canonicalization fix, the 107 build-layout source roots (inside the workspace) were being re-indexed as library sources, doubling memory use. They are now correctly skipped when already covered by the workspace scan.
- **Named lambda parameter with receiver on previous line** — resolved to `:T` when the receiver type and the lambda were on different lines. Fixed by threading the correct UTF-16 column to the CST lookup.
- **CST lambda snapshot race** — stale live-doc snapshots could cause position mismatches in named-lambda-param CST lookup. Fixed by snapshotting `live_doc` once before position derivation.
- **`collect_signature` panic** — panicked when `start_line >= lines.len()`. Now returns `None` gracefully.
- **`forward_resolve_segments` dedup regression** — failed suffix incorrectly suppressed future resolution of the same suffix in a different chain context. Dedup gate now correctly keys on `(segment, resolved_prefix)`.
- **CST chain root type stripping** — fully qualified dotted types (e.g. `androidx.fragment.app.Fragment`) were stripped to `Fragment` too aggressively. The CST root now preserves the full type until the final resolution step.
- **Semantic diagnostics during scan** — diagnostics were published mid-scan, causing transient false positives. Diagnostics are now suppressed until the workspace reaches ready state, then republished.
- **`enclosing_class_at` false positives in receiver lambdas** — `this` inside an `apply`/`run` lambda with an unresolvable receiver type was incorrectly resolved to the enclosing class. The new `ThisContext::InsideReceiver` variant prevents this fallback.
- **Sibling-qualifier bleed in field references** — references with a dot-qualifier matching a sibling field (e.g. `account.value` picked up `value` from unrelated class) are now filtered by checking that the qualifier contains the declaring class name.

### Architecture (internal)

- **MVI workspace actor** (`src/workspace/`) — all mutable workspace state (index, live docs, workspace root, scan phase) is owned by a single `WorkspaceActor` driven by an event loop. Backend and CLI communicate via `WorkspaceHandle`. Eliminates a class of write-order races.
- **Features module** (`src/features/`) — LSP feature implementations (references, hover, definition, completion, rename, go-to-implementation, fill_when, signature_help, …) extracted from the 2000-line `backend/mod.rs` into focused per-feature modules.
- **Language abstraction** (`src/language/`) — per-language keyword sets and override-declaration detection extracted from scattered `if lang == Kotlin` blocks into a `Language` enum with per-variant impls.
- **Infer module split** (`src/indexer/infer/`) — the 1900-line `it_this.rs` split into `chain.rs` (navigation chain resolution), `cst_lambda.rs` (CST-backed lambda context), `receiver.rs` (receiver type inference), and `type_subst.rs` (generic type substitution).

## 0.14.0

- **`sourceRoots` scoping for rg searches** — `rg`-based references, definitions, and symbol searches are now scoped to the configured `sourceRoots` entries from `workspace.json` (IntelliJ/Android Studio module source roots). Searches no longer scan generated code or build output directories when source roots are configured. All callers (Backend, CLI fast mode, resolver step-5, infer) use a single central `Indexer::rg_scope_for_path` path so scoping is consistent across the board. Fixes [#78](https://github.com/Hessesian/kmp-lsp/issues/78).

## 0.13.0

- **Zed extension** — `contrib/zed-extension` registers `kmp-lsp` as a first-class Zed language server for Kotlin, Java, and Swift. Resolves the binary from `$PATH`; no symlinks or `binary.path` overrides required. Install locally with `zed --install-dev-extension contrib/zed-extension` or copy to `~/.config/zed/extensions/kmp-lsp/`.
- **`complete` CLI subcommand** — `kmp-lsp complete <file> <line> [col]` returns completion candidates as JSON (`[{label, kind, detail?, import?}]`). Flags: `--dot` (auto-place cursor after last `.` on the line), `--eol` (end of trimmed line), `--no-stdlib` (skip `~/.kmp-lsp/sources` for ~5× faster project-only completions). Useful for agent/script integration without a running LSP daemon.
- **Library cache** — `sourcePaths`-indexed files are saved to a deterministic on-disk cache (`~/.cache/kmp-lsp/library-<hash>.bin`). Subsequent restarts skip re-parsing unchanged library sources, making warm startup significantly faster on large projects with many source JARs.
- **Library visibility filtering** — symbols marked `private` or `internal` in library source files are stripped from the index. Only `public` and `protected` symbols are indexed for external libraries (inaccessible members add noise to completions and workspace symbol search).
- **Android SDK auto-detection** — the Android platform sources (`$ANDROID_HOME/sources/android-XX/`) are now indexed automatically. Detection order: `sdk.dir` in `local.properties` → `$ANDROID_HOME` → `$ANDROID_SDK_ROOT`. The highest installed API level is picked. No `sourcePaths` config or `extract-sources` needed for Android SDK classes (`Activity`, `Context`, `View`, etc.).
- **`@` completion trigger** — `@` is now a trigger character so annotation completions (`@Composable`, `@Inject`, `@Override`, …) appear immediately after typing `@`.
- **LSP smoke test suite** — `tests/lsp_smoke.rs` exercises the full server over stdio: initialization, workspace symbol, go-to-definition, hover, and inlay hints. Runs against a temp fixture without a real Android project.
- **Stack overflow fix** — `has_fun_interface_descendant` converted from recursive to iterative to prevent stack overflow on deeply nested class hierarchies.

## 0.12.1

- **Auto-include `~/.kmp-lsp/sources` in LSP server** — after running `kmp-lsp extract-sources`, extracted library sources are indexed automatically without any manual `sourcePaths` configuration in the LSP client.
- **Docs overhaul** — README restructured for progressive disclosure (VS Code Quick Start first, condensed config, detailed options moved to `docs/features.md`). `docs/editors.md` reordered with VS Code at the top including platform-specific `.vsix` install commands.

## 0.12.0

- **`extract-sources` CLI** — `kmp-lsp extract-sources` walks the Gradle cache (`~/.gradle/caches/modules-2/files-2.1`), deduplicates `*-sources.jar` by keeping the latest version per artifact, and extracts `.kt`/`.java` sources to `~/.kmp-lsp/sources`. Supports `--dry-run`, `--output`, `--gradle-home`, and optional group/artifact filter patterns. CLI commands (`find`, `refs`, `hover`, `index`) now automatically include `~/.kmp-lsp/sources` so extracted library sources are indexed without any manual configuration.
- **`sources` CLI** — `kmp-lsp sources` lists auto-discovered source roots and their origin (`workspace.json` or `build-layout`). Prints a tip to run `extract-sources` when build-layout detection is active.
- **Zero-config source root discovery** — the LSP server and CLI now auto-discover source roots from JetBrains `workspace.json` (exported by IntelliJ/Android Studio) and from standard Gradle/Maven build layouts (`src/main/kotlin`, `src/main/java`, per-module subprojects). No manual `sourcePaths` configuration needed for most Android projects.
- **Extension robustness** — fixed hang on large workspaces; `shutdown` is now non-blocking; top-level `object` declarations emit `STATIC` semantic token modifier.

## 0.11.0

- **Semantic tokens** — full `textDocument/semanticTokens/full` implementation with two-phase pipeline: Phase 1 (CST classification via tree-sitter) + Phase 2 (cross-file resolution via index). Supports Kotlin, Java, and Swift.
- **`tokens` CLI command** — `kmp-lsp tokens <file>` dumps semantic tokens (CST-only by default, 19ms). `--resolve` opts into Phase 2 cross-file resolution.
- **`tree` CLI command** — `kmp-lsp tree <file>` dumps the tree-sitter parse tree for debugging.
- **VS Code extension** — bundled extension with syntax highlighting, binary auto-discovery, and support for Kotlin, Java, and Swift files. GitHub Actions release workflow builds cross-platform binaries and packages `.vsix`.
- **Performance** — CLI `tokens` defaults to CST-only mode (19ms vs 1.1s with full index). Added `docs/performance.md` with benchmarks and profiling guide.
- **`fd` optional** — file discovery falls back to `walkdir` when `fd` is not installed.

## 0.10.0

- **CLI mode** — `kmp-lsp find|refs|hover|index` subcommands: use kmp-lsp as a standalone tool without an editor or daemon
- **Auto mode** — uses cached index when available, falls back to fast rg/fd automatically (no flag needed)
- **`--fast` flag** — pure rg/fd, zero startup cost; useful in scripts and CI
- **`--smart` flag** — builds index if missing, uses full cross-file accuracy
- **`--json` flag** — machine-readable output for piping/scripting
- **`--root` flag** — workspace root override; defaults to nearest `.git` dir or cwd
- **`--help` / `--version`** — standard CLI flags; work before or after subcommand
- **Helpful errors** — `--find` (common mistake) prints `'find' is a subcommand, not a flag`

## 0.9.4

- **Phase 12 refactoring complete** — replaced bool/tuple returns with named `struct`s for clarity (e.g., `ScanResult`, `NamedResult`); downgraded unreachable `pub` to `pub(crate)` across the binary crate; fixed bare `unwrap()` and double-ref anti-patterns; replaced blocking `std::fs::read_to_string` with `tokio::fs` in spawned tasks.
- **Hexagonal architecture cleanup** — replaced `Option<tower_lsp::Client>` in `Indexer` with `ProgressReporter` outbound port trait. `LspProgressReporter` adapter in backend sends `$/progress` notifications; `NoopReporter` used in CLI/tests. Fixes LSP violation where domain layer depended on protocol types.
- **Comprehensive codebase documentation** — 7 new markdown guides in `docs/codebase/` covering architecture, module structure, conventions, integrations, testing, and known concerns. Includes hexagonal layer breakdown, design patterns, concurrency model, and high-churn risk areas.
- **Feature contributor onboarding** — CodeTour (13-step walkthrough) at `.tours/feature-contributor-guide.tour` teaches how to implement a new LSP feature from handler to tests. Covers architecture layers, handler pattern, resolver logic, and test strategy.

## 0.9.3

- **Performance: no more file cap** — the default file limit is now unlimited. Previously the LSP mode only eagerly indexed 2000 files; larger projects (especially iOS) fell back to on-demand `rg` for deeper files. After the query/parser caching fix in 0.9.2, the per-file parse cost is low enough that indexing everything upfront is the right default. Use `KMP_LSP_MAX_FILES` env var to set a custom limit if needed.
- **Performance: cached tree-sitter queries and parsers** — `Query` objects (the compiled S-expression query automaton) are now compiled once per process via `OnceLock` and reused across all file parses. `Parser` objects are reused per worker thread via thread-local storage. Eliminates the dominant CPU cost for large iOS codebases during indexing.

## 0.9.2

- **Generic type parameter substitution** — hover, inlay hints, and completion now resolve generic type parameters to their concrete types when inside a subclass. For example, if `DashboardProductsReducer : FlowReducer<Event, Effect, State>`, then `EffectType` is shown as `Effect` in inlay hints, hover tooltips, and completion detail. Works for:
  - Enclosing class supertypes (e.g. `FlowReducer<Event, Effect, State>`)
  - Member property type hierarchies (e.g. a `val reducer: DashboardProductsReducer` in a ViewModel gives access to `FlowReducer`'s param substitution)
  - Annotated classes where the declaration line is an annotation (scans up to 5 source lines to find the actual `<TypeParams>`)
- **Hover/inlay hint consistency** — `it`/lambda param hover now uses the same import-aware resolution as go-to-definition (`resolve_symbol` → local → imports → same-package → hierarchy), fixing cases where hover showed the wrong type (e.g. a deprecated enum instead of the local data class)
- **Hover applies enclosing-class substitution** — `it`/`this` hover applies the same substitution map as inlay hints (was previously using raw inferred type)
- **`parse_type_params` fix** — now only looks for `<>` before the first `(`, avoiding false matches on constructor parameter generic types

## 0.9.1

- **CST inlay hints** — inlay hint computation replaced with a tree-sitter preorder walk; no longer scans line-by-line. `line_starts` precomputed for O(1) offset lookups; `hint_property` now uses CST initializer inference for untyped `val`/`var`.
- **Live parse trees** — each open document keeps a live tree-sitter parse tree updated on every `didChange`. CST-first paths in `lambda_params_at_col`, `enclosing_class_at`, and `find_it_element_type_in_lines_impl` use the live tree instead of backward character scans.
- **`it` inside nested lambdas no longer shows `: suspend`** — `find_as_call_arg_type` now tracks brace depth; a cursor inside `setState { it }` no longer walks out through the `{` and mis-infers the outer function's `suspend` parameter type.
- **O(1) line access in CST fast paths** — replaced `from_utf8(&doc.bytes).lines().nth(row)` (O(row)) with `live_lines` map lookups (O(1)) in scope and inference hot paths.

## 0.8.0

- **Completion relevance & ranking** — completions are now scored and sorted by match quality: exact prefix match (score 0) → camelCase acronym match (score 1, e.g. typing `CB` matches `ColumnButton`) → substring (score 2, same-file/package only). Results are capped at 150 items with `isIncomplete: true` so the client re-queries as you type, keeping the list tight. Cross-package (auto-import) symbols require a prefix of ≥ 2 characters and only include prefix/acronym matches (no substring flood). Typing after `@` restricts completions to class/annotation kinds (functions and variables are suppressed).
- **Auto-import completion** — selecting an unimported class/interface/object in completion automatically adds the correct `import` statement. Multiple classes with the same name (from different packages) appear as separate items with the package shown in the detail column. Already-imported, same-package, and star-import-covered symbols are shown without a redundant edit.
- **`sourcePaths` configuration** — index extra directories (library sources, Gradle-unpacked stubs) for hover, go-to-definition and autocomplete, while excluding them from `findReferences` and `rename`. Paths can be absolute (including `~/…`) or relative to the workspace root; no hardcoded directory excludes are applied (the user's intent is trusted). Files inside the workspace root are indexed but not excluded from findReferences.
- **`contrib/extract-sources.py`** — cross-platform Python 3 script that finds `*-sources.jar` files in the Gradle cache, deduplicates by keeping the latest version of each artifact, and extracts `.kt`/`.java` sources to `~/.kmp-lsp/sources/` for use with `sourcePaths`. Supports substring filters (e.g. `androidx.compose`), `--dry-run`, and custom `--gradle-home`/`--output` paths.

## 0.7.1

- **`ignorePatterns` configuration** — exclude directories/files from indexing via `initializationOptions`. Supports gitignore-style globs: bare patterns (e.g. `bazel-*`) match at any depth; path-scoped patterns (e.g. `third-party/**`) match relative to the workspace root. Absolute paths under the workspace root are also accepted. Applied to both `fd` and the `walkdir` fallback, and to the warm-start cached manifest so newly configured patterns take effect without clearing the cache. See [Configuration](#configuration) in the README.
- **Swift hover keyword fix** — Swift functions now correctly show `func` instead of `fun` in hover code blocks.

## 0.7.0

- **`it`/`this` type-directed inference** — when `it` or `this` is used as a call argument (named or positional), the expected parameter type is inferred from the function signature. E.g. `.send(channel = this)` → `SendChannel`, `process(it)` → `Item`
- **`this` in receiver vs regular lambdas** — `this` inside a regular `(T) -> R` lambda now correctly hints the enclosing class instead of the lambda param; only receiver lambdas `T.() -> R` and scope functions (`run`/`apply`/`also`/`let`/`with`) hint the receiver type
- **`fun interface` recognition** — fix tree-sitter not recognising `fun interface` declarations
- **Suspend lambda type inference** — correct type inference for `suspend` lambda parameters
- **Rename regression tests** — 9 tests covering 2/3/4 occurrences same-line, multi-line, substring false-positive, UTF-16 range correctness
- **Copilot extension** — remove overly restrictive `kotlin_rg` pre-hook; all `rg` queries now pass through unconditionally

## 0.6.1

- **`super.method` go-to-def** — must not fall through to an override in the current file; resolves to the parent class declaration

## 0.6.0

- **`super`/`this` go-to-def** — `super` resolves to the parent class; `this.method` resolves via the enclosing class hierarchy
- **Multi-line constructors** — go-to-def works when the constructor spans multiple lines
- **`typealias` support** — indexed and resolved in go-to-def chains
- **Cross-module resolution** — improved supertype priority indexing for cross-module hierarchies

## 0.5.0

- **Workspace pinning** — workspace set once at `initialize` from env var / `~/.config/kmp-lsp/workspace` / `rootUri`; never overridden at runtime by `did_open`
- **Removed `changeRoot` command** — one LSP instance per workspace; restart to switch projects
- **Outside-root file isolation** — files opened outside the workspace root are skipped for workspace-wide indexing
- **Tiered root auto-detection** — strong project markers (`settings.gradle.kts`, `Cargo.toml`) > `.git` > `Package.swift`; correctly handles mono-repos
- **Cold-start navigation** — `hover`, `goToDefinition`, `documentSymbol` work immediately on first file open via on-demand `index_content`
- **`rg` fallback at cold start** — `lines_for` reads from disk when file not yet indexed
- **Live indexing progress** — `WorkDoneProgress::Report` notifications every 500 ms with percentage
- **Extension tools** — `kmp_lsp_status`, `kmp_lsp_set_workspace`

## 0.4.1

- **SOLID refactoring** — pure functions, coordinator pattern, `WorkspaceIndexResult` pipeline
- **Async indexing** — concurrent file parsing with semaphore-guarded `spawn_blocking`
- **iOS indexing fixes** — non-blocking parse, deadlock prevention
- **Cache versioning** — `CACHE_VERSION` bump invalidates stale on-disk indexes
- **`--index-only` CLI mode** — headless one-shot indexing for CI/tooling

## 0.4.0

- **Swift support** — full structural indexing of `.swift` files with all LSP features; SwiftPM `.build` and Xcode `DerivedData` excluded automatically
- **Centralized parser dispatch** — `parse_by_extension()` routes `.kt`/`.java`/`.swift` to the correct tree-sitter parser
- **Dynamic file discovery** — `fd`/`rg` glob patterns and file watchers include all supported extensions

## 0.3.13

- **Inlay hints** — type hints for lambda `it`, named params, `this`, untyped `val`/`var`
- **Go-to-implementation** — transitive subtype lookup via BFS
- **Syntax diagnostics** — tree-sitter `ERROR`/`MISSING` nodes
- **Cross-file lambda resolution** — named-arg lambdas resolve parameter types from constructor signatures
- **Instant feature availability** — all features work immediately via `rg` fallback
- **Race condition fix** — semaphore permit held through `spawn_blocking`
- **Workspace symbol** — dot-qualified queries for extension functions
