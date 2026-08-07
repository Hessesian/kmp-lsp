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
/// Real Kotlin/Java syntax bottoms out at a few dozen levels of nesting,
/// so this sits well above anything a human writes while staying far below
/// the few-thousand frames that overflow an 8 MiB stack. Without a cap,
/// tree depth translates directly into stack frames and a pathological
/// input (one expression with tens of thousands of chained operators, or
/// an unclosed brace followed by tens of thousands of lines) aborts the
/// whole process.
///
/// Not a correctness bound in the usual sense: once hit, callers stop
/// descending into that subtree — under-reporting deeper results rather
/// than erroring — the same trade-off `walk_hierarchy` already makes with
/// its own `max_depth`. Report every hit through
/// [`report_cst_depth_exceeded!`] so the trade-off stays visible.
pub(crate) const MAX_CST_DESCENT_DEPTH: usize = 512;

/// How many hits one [`WarnThrottle`] logs per window. A single pathological
/// tree trips a cap once per node past the limit, so an unthrottled log would
/// bury everything else; the first few carry all the diagnostic value.
pub(crate) const MAX_DEPTH_REPORTS: usize = 5;

/// How long a [`WarnThrottle`] stays quiet before its budget refills.
const WARN_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

static PROCESS_START: std::sync::LazyLock<std::time::Instant> =
    std::sync::LazyLock::new(std::time::Instant::now);

/// A warn budget that refills every [`WARN_WINDOW`], for sites that can fire
/// repeatedly over a long-running server's lifetime.
///
/// [`throttled_warn`]'s plain lifetime budget is right for a site that reports
/// a one-off bug — a task that panicked once is a bug you fix, not one you
/// need re-reported hourly. A depth cap or a broken cycle is different: it
/// tracks whatever file the user has open, so a lifetime budget spent during
/// startup indexing would leave the server permanently silent about every
/// later occurrence.
pub(crate) struct WarnThrottle {
    limit: usize,
    reports: std::sync::atomic::AtomicUsize,
    window_started_secs: std::sync::atomic::AtomicU64,
}

impl WarnThrottle {
    pub(crate) const fn new(limit: usize) -> Self {
        Self {
            limit,
            reports: std::sync::atomic::AtomicUsize::new(0),
            window_started_secs: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Log `message()` unless this window's budget is already spent.
    ///
    /// Two threads racing the window reset can cost or grant one extra line;
    /// that is well within what a diagnostic log tolerates, and is worth not
    /// taking a lock on a path that fires during a stack-depth emergency.
    pub(crate) fn warn(&self, message: impl FnOnce() -> String) {
        use std::sync::atomic::Ordering::Relaxed;
        let now = PROCESS_START.elapsed().as_secs();
        if now.saturating_sub(self.window_started_secs.load(Relaxed)) >= WARN_WINDOW.as_secs() {
            self.window_started_secs.store(now, Relaxed);
            self.reports.store(0, Relaxed);
        }
        let seen = self.reports.fetch_add(1, Relaxed);
        if seen >= self.limit {
            return;
        }
        let suffix = if seen + 1 == self.limit {
            " Further reports from here suppressed until the next window."
        } else {
            ""
        };
        log::warn!("{}{suffix}", message());
    }
}

/// Record that a recursive CST descent stopped at [`MAX_CST_DESCENT_DEPTH`].
///
/// The cap converts a stack overflow into degraded output, but a silent bail
/// leaves nothing to explain *why* results went missing — and a hit on
/// ordinary-looking source is itself the signal that something is wrong (a
/// cycle, or a walker reaching far deeper than real syntax should).
///
/// Call through the [`report_cst_depth_exceeded!`] macro rather than
/// directly: it gives each walker its own budget, so the one walker caught in
/// a loop cannot crowd the other fifteen out of the log.
pub(crate) fn log_cst_depth_exceeded(
    throttle: &WarnThrottle,
    site: &str,
    node: tree_sitter::Node<'_>,
) {
    let position = node.start_position();
    throttle.warn(|| {
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

/// Report a [`MAX_CST_DESCENT_DEPTH`] bail, with a warn budget private to the
/// call site. Each expansion declares its own `static`, which is the whole
/// point — see [`report_cst_depth_exceeded`].
macro_rules! report_cst_depth_exceeded {
    ($site:expr, $node:expr) => {{
        static THROTTLE: $crate::util::WarnThrottle =
            $crate::util::WarnThrottle::new($crate::util::MAX_DEPTH_REPORTS);
        $crate::util::log_cst_depth_exceeded(&THROTTLE, $site, $node);
    }};
}
pub(crate) use report_cst_depth_exceeded;

static RESOLUTION_CYCLE_REPORTS: WarnThrottle = WarnThrottle::new(MAX_DEPTH_REPORTS);

/// Record that a resolution refused to re-enter itself, breaking a cycle.
///
/// Unlike a depth cap — which can fire on merely deep input — reaching here
/// always means the source contains a genuine reference loop, or that
/// inference followed one it should not have. It is never routine, so it is
/// always worth a line in the log: silently returning `None` here is what
/// makes a missing type look like an ordinary unresolvable one.
pub(crate) fn report_resolution_cycle(site: &str, name: &str, uri: &tower_lsp::lsp_types::Url) {
    RESOLUTION_CYCLE_REPORTS.warn(|| {
        format!(
            "resolution cycle broken in {site}: `{name}` in {} is already being resolved further \
             up the stack, so its type is reported as unknown. This means a self- or \
             mutually-referential declaration, and without this guard it would recurse until the \
             stack was exhausted.",
            uri.path(),
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
