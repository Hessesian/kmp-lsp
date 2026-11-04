//! Parsing of a declaration's `detail` text (`"fun f(x: A): B<C>?"`,
//! `"val items: List<Product>"`): its declared return or property type, and
//! whether that return type admits `null`. Pure string work shared by the
//! index, the resolver and the text search, so it sits below all of them.

use crate::StrExt;

/// Map a JVM primitive type's Java-spelled name to the name Kotlin code
/// actually uses for it. The sidecar's pure-Java fallback (`JavaClassVisitor`
/// in `KotlinClassIndexer.kt`) reads a field's type straight off its ASM
/// `Type.className` — lowercase for a primitive (`"int"`, `"boolean"`, …) —
/// so a plain Java field like `public static int ic_account` (Android's
/// generated `R$drawable`, but any Java class's own `int`/`boolean`/…
/// field hits the same shape) would otherwise never pass
/// [`extract_property_type_from_detail`]'s uppercase check below.
fn kotlin_primitive_type_name(java_name: &str) -> Option<&'static str> {
    Some(match java_name {
        "int" => "Int",
        "boolean" => "Boolean",
        "long" => "Long",
        "float" => "Float",
        "double" => "Double",
        "byte" => "Byte",
        "short" => "Short",
        "char" => "Char",
        "void" => "Unit",
        _ => return None,
    })
}

/// Parse the declared type from a property `SymbolEntry.detail` string.
///
/// `"val ViewModel.viewModelScope: CoroutineScope get() = ..."` → `"CoroutineScope"`
/// `"val items: List<Product>"` → `"List<Product>"`
/// `"var count: Int"` → `"Int"`
/// `"val ic_account: int"` → `"Int"` (Java primitive field, see
/// [`kotlin_primitive_type_name`])
///
/// Finds the first `:` after the property name (skipping any receiver `Receiver.`)
/// then uses [`extract_type_with_generics`] to grab the type token.
pub(crate) fn extract_property_type_from_detail(detail: &str) -> Option<String> {
    // Strip optional visibility modifier then `val `/ `var ` prefix.
    let after_vis = detail
        .strip_prefix("public ")
        .or_else(|| detail.strip_prefix("internal "))
        .or_else(|| detail.strip_prefix("protected "))
        .or_else(|| detail.strip_prefix("private "))
        .unwrap_or(detail);
    let after_kw = after_vis
        .strip_prefix("val ")
        .or_else(|| after_vis.strip_prefix("var "))?;
    // Find the first `:` not inside angle-brackets — that separates name from type.
    let mut depth: i32 = 0;
    let colon_pos = after_kw.char_indices().find_map(|(i, c)| match c {
        '<' => {
            depth += 1;
            None
        }
        '>' => {
            depth -= 1;
            None
        }
        ':' if depth == 0 => Some(i),
        _ => None,
    })?;
    let type_part = after_kw[colon_pos + 1..].trim_start();
    let type_name = extract_type_with_generics(type_part);
    if let Some(primitive) = kotlin_primitive_type_name(&type_name) {
        return Some(primitive.to_owned());
    }
    if !type_name.is_empty() && type_name.starts_with_uppercase() {
        Some(type_name)
    } else {
        None
    }
}

/// Parse the return type from a `SymbolEntry.detail` signature string.
///
/// `"fun getDetail(req: Req): Response<Data>"` → `"Response<Data>"`
/// `"fun doSomething()"` → `None`
pub(crate) fn extract_return_type_from_detail(detail: &str) -> Option<String> {
    declared_return_in_detail(detail).map(|declared| declared.type_name)
}

/// Whether a declaration's stated return type is nullable, as far as its
/// detail says ([`extract_return_type_from_detail`] drops the `?`).
pub(crate) fn extract_return_nullability_from_detail(detail: &str) -> ReturnNullability {
    match declared_return_in_detail(detail) {
        Some(declared) if declared.nullable => ReturnNullability::Nullable,
        Some(_) => ReturnNullability::NonNull,
        None => ReturnNullability::Unknown,
    }
}

/// Whether a declared return type admits `null`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReturnNullability {
    NonNull,
    Nullable,
    /// The declaration states no return type this was read from.
    Unknown,
}

struct DeclaredReturnText {
    type_name: String,
    nullable: bool,
}

fn declared_return_in_detail(detail: &str) -> Option<DeclaredReturnText> {
    // If the detail was truncated (ends with `…`), parsing it would yield an
    // incomplete return type string that poisons downstream type substitution.
    // Return None so callers can fall back to the full source-line signature.
    if detail.contains('\u{2026}') {
        return None;
    }
    // Find the closing paren of the parameter list. We search backward for `):` pattern
    // because `= expr()` at the end can place a later `)` that isn't the params close.
    let mut search_from = detail.len();
    loop {
        let close_paren = detail[..search_from].rfind(')')?;
        let after = detail[close_paren + 1..].trim_start();
        if after.starts_with(':') {
            let type_part = after.strip_prefix(':').map(str::trim_start).unwrap_or("");
            let type_name = extract_type_with_generics(type_part);
            // A type name starts uppercase; a package-qualified one
            // (`types.Payload`) starts with its lowercase package, so judge the
            // LAST dotted segment.
            let type_path = type_name.dotted_ident_prefix();
            if !type_path.is_empty() && type_path.last_segment().starts_with_uppercase() {
                let nullable = type_part[type_name.len()..].starts_with('?');
                return Some(DeclaredReturnText {
                    type_name,
                    nullable,
                });
            }
        }
        search_from = close_paren;
    }
}

/// Extract a type name (with generics) from the start of a string.
///
/// `"List<Product> = emptyList()"` → `"List<Product>"`
/// `"StateFlow<UiState>"` → `"StateFlow<UiState>"`
/// `"User?"` → `"User"`  (nullable stripped at the outer `?`)
pub(crate) fn extract_type_with_generics(s: &str) -> String {
    let mut result = String::new();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '<' => {
                depth += 1;
                result.push(c);
            }
            '>' => {
                if depth > 0 {
                    depth -= 1;
                    result.push(c);
                    if depth == 0 {
                        break;
                    }
                } else {
                    break;
                }
            }
            // Stop at these outside of generic brackets.
            '?' | ' ' | '=' | ',' | ')' | '\n' if depth == 0 => break,
            _ => result.push(c),
        }
    }
    result
}
