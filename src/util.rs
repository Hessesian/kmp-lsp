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
    let position = node.start_position();
    throttled_warn(&DEPTH_REPORTS, MAX_DEPTH_REPORTS, || {
        format!(
            "CST descent hit the depth cap ({MAX_CST_DESCENT_DEPTH}) in {site} at \
             {}:{} (node kind `{}`) — deeper nodes were skipped. Real Kotlin/Java \
             nests a few dozen levels, so this points at either a pathologically \
             large expression or a resolution loop.",
            position.row + 1,
            position.column + 1,
            node.kind(),
        )
    });
}

/// Log `message()` via `log::warn!`, capped at `limit` calls through this
/// particular `counter`.
///
/// Generalizes the counter-and-suppress dance [`report_cst_depth_exceeded`]
/// established, so every other "this should be rare, but on pathological or
/// looping input could fire on every request" call site can declare its own
/// `static AtomicUsize` and reuse this instead of reinventing the throttle.
/// `message` is a closure (not a plain `String`) so the formatting work is
/// skipped entirely once a site has gone quiet.
pub(crate) fn throttled_warn(
    counter: &std::sync::atomic::AtomicUsize,
    limit: usize,
    message: impl FnOnce() -> String,
) {
    let seen = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if seen >= limit {
        return;
    }
    let suffix = if seen + 1 == limit {
        " Further reports suppressed."
    } else {
        ""
    };
    log::warn!("{}{suffix}", message());
}

/// Describe a `spawn_blocking`/`tokio::spawn` task that failed to complete —
/// panicked or was cancelled — for use inside a [`throttled_warn`] call at
/// each `JoinHandle` await site that currently converts the `Err` into an
/// empty/default result.
///
/// A panic there is never a legitimate "nothing found": the task's own logic
/// decides what counts as absent and returns `None`/`vec![]` on that path
/// deliberately. Reaching `Err` means the task unwound instead, and without
/// this the caller's fallback silently looks identical to a real miss —
/// exactly the "diagnostic that never fired" / "results silently missing"
/// failure mode this module's other guard exists to catch.
/// `what` should say which task and, where cheap, which input (a URI, a
/// symbol name); `err` is the `JoinError` tokio handed back.
pub(crate) fn join_failure_message(what: &str, err: &tokio::task::JoinError) -> String {
    format!(
        "background task panicked or was cancelled while {what}: {err} — falling back to an \
         empty/default result that will look like an ordinary miss to the caller"
    )
}
