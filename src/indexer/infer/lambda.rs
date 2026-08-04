//! Pure helpers for recognising and decomposing Kotlin lambda/function types.
//!
//! All functions are pure: they accept `&str` / `usize` arguments and return
//! `Option<String>`.  No `Indexer` dependency; no I/O.

#[cfg(test)]
#[path = "lambda_tests.rs"]
mod tests;

use crate::StrExt;

/// Kotlin stdlib scope functions whose lambda receives the object as `this` (receiver lambdas).
/// For these, `this` inside the lambda refers to `T` (the receiver), so a type hint is valid.
///
/// Note: `let` and `also` are intentionally excluded — their lambda parameter is `it`, not `this`.
pub(crate) const RECEIVER_THIS_FNS: &[&str] = &["run", "apply"];

/// Kotlin stdlib scope functions whose `T` parameter IS the receiver type.
pub(crate) const SCOPE_FUNCTIONS: &[&str] =
    &["let", "also", "run", "apply", "takeIf", "takeUnless"];

/// Functions whose return value is the result of their trailing lambda, so the
/// inferred type is the lambda's last expression. Compose `remember { Foo() }`
/// returns `Foo`; without this the call resolves against an unrelated same-named
/// overload (e.g. the Kotlin compiler's `VariableStorage.remember`).
pub(crate) const LAMBDA_RESULT_FNS: &[&str] = &["remember", "rememberSaveable"];

/// DI-framework factory functions of the shape `inline fun <reified T> name(): T`
/// (Koin's `get`/`inject`, AndroidX's `viewModel`/`activityViewModel`, Retrofit-style
/// `create`). Callers reach for these specifically so the compiler can infer `T`
/// from the call-site type argument — the exact information a caller like this
/// one, that has no compiler, would otherwise need the function's *signature*
/// indexed to recover. These functions live in DI/framework libraries that are
/// frequently not indexed at all (unpromoted JAR, or simply absent from a
/// project's own workspace), so name-based/return-type lookup
/// (`find_fun_return_type_reachable`/`find_fun_return_type`) legitimately finds
/// nothing. When that happens, `resolve_call_expr_type` falls back to reading
/// the call's own `<T>` type argument directly — sound specifically because
/// each of these functions' contract is "return exactly `T`", so no signature
/// lookup is needed to know the result type once the call syntax matches.
pub(crate) const GENERIC_FACTORY_FNS: &[&str] =
    &["get", "inject", "viewModel", "activityViewModel", "create"];

/// Kotlin's built-in numeric/`Char` conversion functions (`Number.toLong()`,
/// `Char.toInt()`, …) paired with their fixed return type. These are stdlib
/// intrinsics on every primitive numeric type (and `Char`/`String`), so the
/// *name* alone determines the return type — no signature lookup is needed,
/// which matters because the receiver is frequently a type this indexer has
/// no declaration for at all (a `String`/`Int`/etc. literal or an unresolved
/// expression), so member-based lookup (`find_method_return_type_for_type`)
/// has nothing to find.
pub(crate) const NUMERIC_CONVERSION_FNS: &[(&str, &str)] = &[
    ("toByte", "Byte"),
    ("toShort", "Short"),
    ("toInt", "Int"),
    ("toLong", "Long"),
    ("toFloat", "Float"),
    ("toDouble", "Double"),
    ("toChar", "Char"),
];

/// Return the Nth (0-based) input type from a functional type expression.
///
/// `lambda_type_nth_input("(String, Boolean) -> Unit", 0)` → `Some("String")`
/// `lambda_type_nth_input("(String, Boolean) -> Unit", 1)` → `Some("Boolean")`
/// `lambda_type_nth_input("() -> Unit", 0)` → `None`
pub(crate) fn lambda_type_nth_input(type_name: &str, n: usize) -> Option<String> {
    let type_name = type_name.trim();
    // Strip `suspend` keyword — Kotlin allows `suspend (T) -> Unit` as a type.
    let type_name = strip_suspend(type_name);
    if !type_name.starts_with('(') {
        return None;
    }
    // Find matching `)` using separate paren/angle depth so `>` in `->` is
    // never mistaken for a closing angle bracket.
    let mut paren_depth: i32 = 0;
    let mut _angle_depth: i32 = 0;
    let mut close = None;
    for (i, ch) in type_name.char_indices() {
        match ch {
            '(' => paren_depth += 1,
            ')' => {
                paren_depth -= 1;
                if paren_depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            '<' => _angle_depth += 1,
            '>' => _angle_depth -= 1,
            _ => {}
        }
    }
    let close = close?;
    let inner = type_name[1..close].trim();
    if inner.is_empty() {
        return None;
    }

    // If `inner` is itself a function type (outer parens were just wrapping:
    // `((T) -> R)` → inner = `(T) -> R`), recurse into it.
    if inner.starts_with('(') && inner.contains("->") {
        if let Some(t) = lambda_type_nth_input(inner, n) {
            return Some(t);
        }
    }

    // Split inner by comma at depth 0.
    let mut args: Vec<&str> = Vec::new();
    let mut start = 0;
    let mut d: i32 = 0;
    for (i, ch) in inner.char_indices() {
        match ch {
            '(' | '<' | '[' => d += 1,
            ')' | '>' | ']' => d -= 1,
            ',' if d == 0 => {
                args.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    args.push(&inner[start..]);

    let arg = args.get(n).map(|s| s.trim())?;
    // Strip named-param prefix `name:`.
    let arg = if let Some(c) = arg.find(':') {
        arg[c + 1..].trim()
    } else {
        arg
    };
    // Strip `suspend` keyword from function-type args like `suspend (T) -> Unit`.
    let arg = strip_suspend(arg);
    // Allow dots for qualified types like `CreditCardDashboardInteractor.CardProduct`.
    let base: String = arg.dotted_ident_prefix();
    // Trim any trailing dots.
    let base = base.trim_end_matches('.');
    if base.is_empty() || !base.starts_with_uppercase() {
        return None;
    }
    Some(base.to_owned())
}

/// Given a Kotlin function/lambda type, extracts the receiver type if it is a **receiver
/// lambda** (`T.() -> R` or `T.(Params) -> R`).  Returns `None` for regular lambdas
/// (`(T) -> R`) since `this` in those refers to the enclosing class, not the param.
pub(crate) fn lambda_type_receiver(type_name: &str) -> Option<String> {
    // A *nullable* function type is parenthesised: `(Receiver.() -> R)?`. Strip a
    // leading `(` so the receiver before `.(` isn't preceded by the wrapping paren
    // (e.g. a Compose slot `content: (LazyListScope.() -> Unit)? = null`).
    let type_name = strip_suspend(type_name.trim())
        .trim_start_matches('(')
        .trim_start();
    if let Some(dot_paren) = type_name.find(".(") {
        let receiver = type_name[..dot_paren].trim();
        let base: String = receiver.dotted_ident_prefix();
        let base = base.trim_end_matches('.');
        if !base.is_empty() {
            return Some(base.to_owned());
        }
    }
    None
}

/// Strip a leading `suspend` keyword from a Kotlin function type string.
/// `"suspend (T) -> Unit"` → `"(T) -> Unit"`;
/// `"suspend ProducerScope<E>.() -> Unit"` → `"ProducerScope<E>.() -> Unit"`.
/// Requires `suspend` to be a whole word (followed by whitespace) so identifiers like
/// `suspendable` are untouched. In a function-type string the only word `suspend` can
/// be is the modifier, so the following token (`(`, a receiver type, …) is the type.
#[inline]
fn strip_suspend(type_name: &str) -> &str {
    if let Some(rest) = type_name.strip_prefix("suspend") {
        if rest.starts_with(char::is_whitespace) {
            return rest.trim_start();
        }
    }
    type_name
}
