use std::path::PathBuf;

/// Returns the current user's home directory.
///
/// Wraps the deprecated `std::env::home_dir` in a single place so callsites
/// don't need to repeat the `#[allow(deprecated)]` annotation.
#[allow(deprecated)]
pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::home_dir()
}

/// Shared recursion-depth cap for hand-rolled recursive CST descents
/// (`node.children()` walked via plain Rust recursion rather than an
/// iterative `TreeCursor` loop).
///
/// Real Kotlin/Java syntax — even unusually deeply nested Compose UI —
/// bottoms out at a few dozen levels; measured against the actual
/// `nowinandroid` sample, a realistic file (including a malformed,
/// mid-edit buffer) never exceeds ~20. This cap is generous relative to
/// that (over an order of magnitude of headroom) while still sitting far
/// below the few-thousand-frame threshold that overflows an 8 MiB stack —
/// see the recursive descents in `features::call_arg_diagnostics`,
/// `features::fill_when`, `features::missing_import_diagnostics`,
/// `features::nullable_call_diagnostics`, and `indexer::cst_folding`,
/// each of which independently stack-overflows on a pathological input
/// (e.g. a single expression with tens of thousands of chained
/// operators/segments, or an unclosed-brace file with tens of thousands of
/// trailing lines) with no guard at all before this constant was
/// introduced.
///
/// Not a correctness bound in the usual sense: once hit, callers simply
/// stop descending into that subtree (silently under-reporting deeper
/// diagnostics) rather than erroring — the same trade-off `resolve_chain`'s
/// hierarchy walk already makes with its own `max_depth` parameter.
pub(crate) const MAX_CST_DESCENT_DEPTH: usize = 512;
