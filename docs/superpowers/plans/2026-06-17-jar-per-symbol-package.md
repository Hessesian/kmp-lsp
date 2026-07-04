# JAR Per-Symbol Package Resolution — Implementation Plan

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax. This plan spans the Kotlin sidecar **and** the Rust crate; the sidecar must be rebuilt and redeployed before the Rust-side behaviour can be validated end-to-end.

**Goal:** Make every JAR symbol carry its **real package** (and a top-level flag), so go-to-definition / import resolution for a common name like `remember` lands on the correct public declaration instead of a random internal impl from an unrelated jar.

**Architecture:** The Kotlin sidecar already knows each class's fully-qualified internal name (`klass.name`, e.g. `androidx/compose/runtime/ComposablesKt`) but discards the package. We emit `pkg` + `top_level` per `SymbolEntry`, store the package per Rust `SymbolEntry`, build the correct `qualified` FQN (`pkg.name` for top-level, `pkg.Container.name` for members), and replace the blunt `jar:` filter bypass in `resolve_via_imports` with a real per-symbol package filter. This retires the "one inferred package per multi-package jar" hack.

**Tech Stack:** Kotlin (kotlinx-metadata/ASM sidecar, Gradle shadowJar), Rust (serde, DashMap, tower-lsp).

---

## Root cause (verified via harness on nowinandroid + 754 jars)

`resolve_symbol("remember")` returns **13 locations** across compose-runtime, the Kotlin compiler, gradle plugin, KSP, intellij. Two defects in `build_jar_file_data` (jar.rs:741-798):

1. **One inferred package per jar** — derived from the first class-like symbol's detail. Wrong for multi-package jars; useless for the import filter, which is why `resolve_via_imports` (resolve.rs:700) just does `if uri.starts_with("jar:") return true` and lets every same-named symbol through.
2. **Wrong FQN for top-level functions** — built as `pkg.ComposablesKt.remember` (facade used as container), so `qualified["androidx.compose.runtime.remember"]` *misses* and resolution falls through to the unfiltered short-name path.

## File structure

- `java-sidecar/.../model/SymbolEntry.kt` — add `pkg`, `topLevel` fields.
- `java-sidecar/.../KotlinClassIndexer.kt` — capture package from class internal name; set fields in `entriesFromClass` / `entriesFromPackage` / `JavaClassVisitor`.
- `src/sidecar.rs` — add `pkg`, `top_level` to `SidecarSymbol`.
- `src/types.rs` — add `package` to Rust `SymbolEntry`.
- `src/indexer/jar.rs` — store per-symbol package; build correct `qualified` FQN; bump nothing here.
- `src/indexer/jar_cache.rs` — bump `JAR_CACHE_VERSION` (SidecarSymbol bincode layout changed).
- `src/resolver/resolve.rs` — replace the `jar:` bypass with a per-symbol package filter.

---

## Task 1 — Sidecar emits per-symbol package + top-level flag

**Files:**
- Modify: `java-sidecar/src/main/kotlin/io/github/hessesian/jarindexer/model/SymbolEntry.kt`
- Modify: `java-sidecar/src/main/kotlin/io/github/hessesian/jarindexer/KotlinClassIndexer.kt`
- Test: `java-sidecar/src/test/kotlin/io/github/hessesian/jarindexer/IndexerTest.kt`

- [ ] **Step 1: Add fields to the Kotlin model.** In `SymbolEntry.kt`, after `deprecated`:

```kotlin
    /** Fully-qualified package of the declaring class, e.g. "androidx.compose.runtime". Empty if default package. */
    val pkg: String = "",
    /** True for top-level declarations (top-level fun/val, or a class/interface/object itself). */
    @SerialName("top_level")
    val topLevel: Boolean = false,
```

- [ ] **Step 2: Capture the package in `ClassMetadataVisitor`.** Add `var packageName: String = ""` and in `visit(...)` set:

```kotlin
        packageName = if (name.contains('/')) name.substringBeforeLast('/').replace('/', '.') else ""
        simpleClassName = name.substringAfterLast('/')
```

- [ ] **Step 3: Thread the package through `indexClassBytes`.** Pass `visitor.packageName` into `entriesFromClass` / `entriesFromPackage` (and the Java path). Update signatures:
  - `entriesFromClass(klass, dep, pkg)` — the class entry gets `pkg = pkg, topLevel = true`; each function/property gets `pkg = pkg, topLevel = false`.
  - `entriesFromPackage(pkg, containerName, dep, pkgName)` — every emitted fun/property gets `pkg = pkgName, topLevel = true` (file-facade members are top-level).
  - `JavaClassVisitor` — set `pkg` from the class internal name; class entry `topLevel = true`, methods `topLevel = false`.

- [ ] **Step 4: Add a test** in `IndexerTest.kt` asserting a top-level function (e.g. compile a tiny class or use an existing fixture) has the expected `pkg` and `topLevel = true`, and a class member has `topLevel = false`.

- [ ] **Step 5: Build the fat jar.** Run: `cd java-sidecar && ./gradlew shadowJar`. Expected: `build/libs/java-sidecar.jar` produced (classifier+version stripped).

- [ ] **Step 6: Deploy to the local test rig.** Copy `java-sidecar/build/libs/java-sidecar.jar` → `~/.cargo/bin/kmp-jar-indexer.jar` (the native `.bak` stays renamed; discovery uses the jar). See `[[sidecar-local-test-rig]]`.

---

## Task 2 — Rust deserialization

**Files:**
- Modify: `src/sidecar.rs:42`

- [ ] **Step 1: Add fields to `SidecarSymbol`** after `deprecated`:

```rust
    /// Fully-qualified package of the declaring class, e.g. "androidx.compose.runtime".
    #[serde(default)]
    pub pkg: String,
    /// True for top-level declarations (top-level fun/val, or a class/interface/object itself).
    #[serde(default)]
    pub top_level: bool,
```

- [ ] **Step 2: Compile-check** (`cargo check --bin kmp-lsp`). `#[serde(default)]` keeps old caches/JSON loadable.

---

## Task 3 — Store per-symbol package on the Rust `SymbolEntry`

**Files:**
- Modify: `src/types.rs:138` (struct), `src/indexer/jar.rs:700` (construction)
- Test: `src/indexer/jar_tests.rs`

- [ ] **Step 1: Add field to `SymbolEntry`** after `deprecated`:

```rust
    /// Fully-qualified package of the declaring type for JAR-indexed symbols
    /// (e.g. "androidx.compose.runtime"). Empty for source-indexed symbols
    /// (their package lives on `FileData.package`).
    #[serde(default)]
    pub package: String,
```

- [ ] **Step 2: Fix every `SymbolEntry { … }` literal** that now misses `package`. Source-path construction sites set `package: String::new()`. The JAR site (jar.rs:700) sets `package: sym.pkg.clone()`. Run `cargo check` and fix each reported literal.

- [ ] **Step 3 (jar.rs): build the correct `qualified` FQN per symbol** using `sym.top_level`. Replace the per-jar `package` inference block (jar.rs:741-798) so the FQN is:
  - top-level → `format!("{}.{}", sym.pkg, sym.name)` (skip if `sym.pkg` empty)
  - member (`!top_level`, `container = Some(c)`) → `format!("{}.{}.{}", sym.pkg, c, sym.name)`

  Keep the existing per-jar `package` inference only as the `FileData.package` fallback (harmless; or set `FileData.package = None`). Insert each FQN into `indexer.qualified`.

- [ ] **Step 4: Test** `jar_symbol_registers_correct_top_level_fqn`: inject sidecar symbols (top-level `remember` with `pkg = "androidx.compose.runtime", top_level = true`, plus a member `Foo.remember`), then assert `idx.qualified.get("androidx.compose.runtime.remember")` resolves to the top-level one and `…Foo.remember` to the member.

---

## Task 4 — Replace the `jar:` filter bypass with a real package filter

**Files:**
- Modify: `src/resolver/resolve.rs:692-714` (`resolve_via_imports` step ii)
- Test: `src/resolver/tests.rs`

- [ ] **Step 1: Add a helper** to fetch a JAR location's symbol package:

```rust
/// Package of the JAR symbol at `loc` (matches by synthetic range), if any.
fn jar_symbol_package(indexer: &Indexer, loc: &Location) -> Option<String> {
    let data = indexer.jar_files.get(loc.uri.as_str())?;
    data.symbols
        .iter()
        .find(|s| s.range == loc.range && !s.package.is_empty())
        .map(|s| s.package.clone())
}
```

- [ ] **Step 2: Replace the bypass.** In the step-ii filter, change the `if loc.uri.as_str().starts_with("jar:") { return true; }` branch to compare the symbol's real package to `expected_pkg` (accept exact or prefix, mirroring the source branch); fall back to `true` only when the symbol package is unknown (older cache):

```rust
                    if loc.uri.as_str().starts_with("jar:") {
                        return match jar_symbol_package(indexer, loc) {
                            Some(p) => p == expected_pkg || p.starts_with(&format!("{expected_pkg}.")),
                            None => true, // no package info (pre-v8 cache) → keep, don't regress
                        };
                    }
```

- [ ] **Step 3: Test** `import_resolves_jar_symbol_to_correct_package`: inject two jar `remember` symbols (one `pkg = androidx.compose.runtime`, one `pkg = org.jetbrains.kotlin`), a caller importing `androidx.compose.runtime.remember`; assert resolution returns only the compose one.

---

## Task 5 — Bump BOTH cache versions

bincode is positional and not new-field-tolerant (despite `#[serde(default)]`), so
adding a field to a cached struct breaks old caches and **requires** a version bump.
Two caches are affected:

**Files:**
- Modify: `src/indexer/jar_cache.rs:26` — `SidecarSymbol` (jar cache) gained `pkg` + `top_level`.
- Modify: `src/indexer/cache.rs` (or wherever `CACHE_VERSION` lives) — `SymbolEntry` (workspace cache) gained `package`.

- [ ] **Step 1: Bump `JAR_CACHE_VERSION`** to the next integer (8 if PR #169's bump to 7 has merged) + `// vN: SidecarSymbol gained pkg + top_level` comment.
- [ ] **Step 2: Bump the workspace `CACHE_VERSION` 28 → 29** + `// v29: SymbolEntry gained package` comment. (Locate via `grep -rn "CACHE_VERSION" src/`.)

---

## Task 6 — End-to-end validation

- [ ] **Step 1: Full Rust suite + clippy** — `cargo test --bin kmp-lsp` and `cargo clippy --bin kmp-lsp`, both clean.
- [ ] **Step 2: Ground-truth harness** (temporary `#[ignore]` test, as used during diagnosis): index nowinandroid + jars, assert `resolve_symbol("remember")` from a caller returns only `androidx.compose.runtime` locations (the 5 public overloads / a single top-level via `qualified`), with zero kotlin-compiler/gradle/ksp hits. Remove the harness before committing.
- [ ] **Step 3: `cargo install --path .`** and confirm in helix: gd on `remember` lands in compose runtime.
- [ ] **Step 4: Commit** to a new branch stacked appropriately; open PR.

---

## Out of scope / follow-ups
- `is_import_reachable` (sig.rs) still fails open for `jar:` URIs (jar files aren't in `idx.files`); the call-arg library clamp already keeps diagnostics correct, so per-symbol-package reachability for the diagnostic path is a separate optional improvement.
- `qualified` remains `DashMap<String, Location>` (single). Overloaded top-level functions collapse to one location for go-def — acceptable (lands on a correct overload). Returning *all* overloads would require `Vec<Location>` and is deferred.
