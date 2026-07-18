# Kotlin 2.4 language coverage baseline

## Result

Kotlin `2.4` and compiler `v2.4.10` are the initial language boundary for the
kmp-lsp coverage matrix. The matrix describes the current contract directly;
it does not reconstruct the release-by-release migration from Kotlin 1.9.

The baseline contains 2,145 requirements:

| Classification / status | Requirements |
|---|---:|
| exact / active | 632 |
| exact / ignored | 385 |
| heuristic / active | 27 |
| heuristic / ignored | 57 |
| out-of-scope / excluded | 1,044 |
| **Total** | **2,145** |

Status totals are 659 active, 442 ignored, and 1,044 excluded. The 442 ignored
requirements have 448 unique executable tests. The complete matrix has 1,109
unique primary test links.

## Source boundaries

| Purpose | Repository / release | Revision | Source boundary |
|---|---|---|---|
| Normative Kotlin/Core source | `Kotlin/kotlin-spec` / `1.9-rfc+0.1` | `2f7aa0524ec27e788dfacd550f144809f2e0254c` | `docs/src/md`, 20 Kotlin/Core documents |
| Kotlin language target | `JetBrains/kotlin` / `v2.4.10` | `5687445832cd835b4509b9fbc264cdf1a8201093` | compiler sources and test data cited by current KL requirements |
| Current Language Guide | `JetBrains/kotlin-web-site` | `7c270c2ac320fbee4884927f056b89d32f2a002e` | 49 local topics under the `Language guide` TOC subtree |

The Kotlin 1.9 specification remains the normative source for the 2,105
Kotlin/Core requirements because no separate Kotlin 2.4 specification exists.
Those citations do not make Kotlin 1.9 a supported baseline. Forty additional
KL requirements capture current behavior established by the pinned Kotlin
2.4.10 compiler sources and test data.

Preview compiler releases, including `v2.4.20-Beta1`, are outside this stable
baseline.

## Matrix layout

The tracked matrix uses 22 TOML files:

- `tests/kotlin_spec/coverage.toml` defines the Kotlin 2.4 target, aggregate
  counts, source identities, and the Language Guide topic list;
- 20 `tests/kotlin_spec/coverage/kotlin.core/*.toml` files retain the
  source-oriented Kotlin/Core requirements;
- `tests/kotlin_spec/coverage/kotlin.language.toml` contains all 40 current KL
  requirements not defined by Kotlin/Core.

KS and KL identifiers are stable traceability keys. Historical previous IDs,
per-release changelog audit IDs, migration status, stabilization history,
preview dispositions, and the standalone documentation claim ledger are not
part of the current baseline.

The Rust validator checks aggregate and per-source counts, unique requirement
IDs, requirement metadata, primary test ownership, active/ignored status,
Language Guide membership, Kotlin 2.4 compiler citations, and complete tracing
of every `ks_*` and `kl_*` test.

## Known unsupported Kotlin 2.4 behavior

Ignored exact tests expose unsupported behavior rather than removing it from
the matrix:

- named context parameters and explicit context arguments do not parse cleanly;
- collection literals lack expected-type companion factory validation;
- companion blocks, companion extensions, the `@all` use-site target,
  multi-dollar interpolation, and `when` guards are not recognized by the
  production grammar;
- context-sensitive resolution does not use expected enum, sealed,
  companion-bearing, call-argument, or annotation-argument types;
- exhaustiveness diagnostics do not fully model generic sealed bounds, empty
  bounded types, data-flow narrowing, shared triangle leaves, or inline-lambda
  control-flow facts;
- several K2 diagnostic rules remain absent, including empty-left-hand-side
  class literals, enum-entry callable references, deprecated enum-entry
  diagnostics, invalid named lambda arguments, and expression-body return
  restrictions;
- experimental name-based and square-bracket destructuring forms remain
  unsupported.

Supported current behavior stays active, including local and inherited nested
type aliases, explicit backing-field navigation, underscore local declarations,
root-package import rules, package modifier rejection, and Kotlin 2.4
smart-casted `when` subject exhaustiveness.

## Verification

The self-contained matrix check is:

```sh
cargo test coverage_matrix_has_valid_traceability_entries --quiet
```

The following ignored tests validate citations when the pinned read-only source
checkouts are present:

```sh
cargo test coverage_matrix_matches_pinned_kotlin_spec_checkout -- --ignored
cargo test language_requirements_match_pinned_kotlin_checkout -- --ignored
cargo test documentation_citations_match_pinned_kotlin_web_site_checkout -- --ignored
```

Repository verification also includes:

```sh
cargo fmt --check
cargo test
cargo clippy -- -D warnings
cargo clippy --all-targets -- -D warnings
git diff --check
```
