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
/// stop descending into that subtree (under-reporting deeper diagnostics)
/// rather than erroring — the same trade-off `resolve_chain`'s hierarchy
/// walk already makes with its own `max_depth` parameter. Report every hit
/// through [`report_cst_depth_exceeded`] so that trade-off stays visible.
pub(crate) const MAX_CST_DESCENT_DEPTH: usize = 512;

/// How many depth-cap hits to log before going quiet. One pathological tree
/// trips the cap once per node past the limit, so an unthrottled log would
/// bury everything else; the first few carry all the diagnostic value.
const MAX_DEPTH_REPORTS: usize = 5;

static DEPTH_REPORTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Record that a recursive CST descent stopped at [`MAX_CST_DESCENT_DEPTH`].
///
/// The cap exists to convert a stack overflow into degraded output, but a
/// silent bail leaves nothing to explain *why* results went missing — and a
/// hit on ordinary-looking source is itself the signal that something is
/// wrong (a cycle, or a walker reaching far deeper than real syntax should).
/// `site` names the walker; the node's position points at the input.
pub(crate) fn report_cst_depth_exceeded(site: &str, node: tree_sitter::Node<'_>) {
    let seen = DEPTH_REPORTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if seen >= MAX_DEPTH_REPORTS {
        return;
    }
    let position = node.start_position();
    log::warn!(
        "CST descent hit the depth cap ({MAX_CST_DESCENT_DEPTH}) in {site} at \
         {}:{} (node kind `{}`) — deeper nodes were skipped. Real Kotlin/Java \
         nests a few dozen levels, so this points at either a pathologically \
         large expression or a resolution loop.{}",
        position.row + 1,
        position.column + 1,
        node.kind(),
        if seen + 1 == MAX_DEPTH_REPORTS {
            " Further reports suppressed."
        } else {
            ""
        },
    );
}
