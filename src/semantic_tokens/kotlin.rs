//! Phase 1 CST-only classification for Kotlin declarations.

use tower_lsp::lsp_types::{SemanticTokenModifier, SemanticTokenType};
use tree_sitter::Node;

use crate::queries::{
    KIND_ANNOTATION, KIND_BINDING_PATTERN_KIND, KIND_CLASS_DECL, KIND_CLASS_PARAM,
    KIND_COMPANION_OBJ, KIND_ENUM_ENTRY, KIND_FUN_DECL, KIND_IDENTIFIER, KIND_IMPORT_HEADER,
    KIND_KW_AS, KIND_KW_AS_SAFE, KIND_KW_BY, KIND_KW_ENUM, KIND_KW_IN, KIND_KW_INTERFACE,
    KIND_KW_IN_NOT, KIND_KW_IS, KIND_KW_IS_NOT, KIND_KW_VAL, KIND_MULTI_ANNOTATION,
    KIND_MULTI_VAR_DECL, KIND_OBJECT_DECL, KIND_PARAMETER, KIND_PRIMARY_CTOR, KIND_PROP_DECL,
    KIND_SIMPLE_IDENT, KIND_TYPE_IDENT, KIND_TYPE_PARAM, KIND_VALUE_ARG, KIND_VAR_DECL,
};

use super::helpers::{
    child_ident, find_annotation_ident, first_child_of_kind, has_deprecated_annotation,
    has_keyword_child, has_modifier, is_in_companion_body, is_inside_class_body, is_top_level,
    push_token, push_token_at_byte_range, value_arg_label,
};
use super::{modifier_bit, type_index, RawToken, Source};

pub(super) fn walk_kotlin(node: Node<'_>, src: &Source<'_>, out: &mut Vec<RawToken>) {
    for node in crate::indexer::walk::descendants(node) {
        classify_kotlin(node, src, out);
    }
}

fn classify_kotlin(node: Node<'_>, src: &Source<'_>, out: &mut Vec<RawToken>) {
    let kind = node.kind();
    match kind {
        k if k == KIND_CLASS_DECL => kotlin_class_token(node, src, out),
        k if k == KIND_OBJECT_DECL => kotlin_object_token(node, src, out),
        k if k == KIND_COMPANION_OBJ => kotlin_companion_token(node, src, out),
        k if k == KIND_FUN_DECL => kotlin_fun_token(node, src, out),
        k if k == KIND_PROP_DECL => kotlin_prop_token(node, src, out),
        k if k == KIND_TYPE_PARAM => kotlin_type_param_token(node, src, out),
        k if k == KIND_CLASS_PARAM => kotlin_class_param_token(node, src, out),
        k if k == KIND_PRIMARY_CTOR => kotlin_primary_constructor_keyword_token(node, src, out),
        KIND_PARAMETER => {
            let mods = modifier_bit(&SemanticTokenModifier::DECLARATION);
            if let Some(name) = child_ident(node) {
                push_token(
                    name,
                    type_index(&SemanticTokenType::PARAMETER),
                    mods,
                    src,
                    out,
                );
            }
        }
        KIND_ENUM_ENTRY => {
            let mods = modifier_bit(&SemanticTokenModifier::DECLARATION)
                | modifier_bit(&SemanticTokenModifier::READONLY);
            if let Some(name) = child_ident(node) {
                push_token(
                    name,
                    type_index(&SemanticTokenType::ENUM_MEMBER),
                    mods,
                    src,
                    out,
                );
            }
        }
        KIND_ANNOTATION | KIND_MULTI_ANNOTATION if find_annotation_ident(node).is_some() => {
            push_token(node, type_index(&SemanticTokenType::DECORATOR), 0, src, out);
        }
        k if k == KIND_VALUE_ARG => {
            if let Some(label) = value_arg_label(node) {
                push_token(
                    label,
                    type_index(&SemanticTokenType::PARAMETER),
                    0,
                    src,
                    out,
                );
            }
        }
        k if k == KIND_KW_IS
            || k == KIND_KW_IS_NOT
            || k == KIND_KW_AS
            || k == KIND_KW_AS_SAFE
            || k == KIND_KW_IN
            || k == KIND_KW_IN_NOT
            || k == KIND_KW_BY =>
        {
            push_token(node, type_index(&SemanticTokenType::KEYWORD), 0, src, out);
        }
        k if k == KIND_IMPORT_HEADER => {
            // Highlight the dotted import path (e.g. `java.util.Scanner`) as a namespace.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == KIND_IDENTIFIER {
                    push_token(
                        child,
                        type_index(&SemanticTokenType::NAMESPACE),
                        0,
                        src,
                        out,
                    );
                    break;
                }
            }
        }
        _ => {}
    }
}

fn kotlin_class_token(node: Node<'_>, src: &Source<'_>, out: &mut Vec<RawToken>) {
    let token_type = if has_keyword_child(node, KIND_KW_INTERFACE) {
        type_index(&SemanticTokenType::INTERFACE)
    } else if has_keyword_child(node, KIND_KW_ENUM) {
        type_index(&SemanticTokenType::ENUM)
    } else if has_modifier(node, src, "data") {
        type_index(&SemanticTokenType::STRUCT)
    } else {
        type_index(&SemanticTokenType::CLASS)
    };
    let mut mods = modifier_bit(&SemanticTokenModifier::DECLARATION);
    if has_modifier(node, src, "abstract") {
        mods |= modifier_bit(&SemanticTokenModifier::ABSTRACT);
    }
    if has_deprecated_annotation(node, src.bytes) {
        mods |= modifier_bit(&SemanticTokenModifier::DEPRECATED);
    }
    if let Some(name) = child_ident(node) {
        push_token(name, token_type, mods, src, out);
    }
}

fn kotlin_object_token(node: Node<'_>, src: &Source<'_>, out: &mut Vec<RawToken>) {
    let mut mods = modifier_bit(&SemanticTokenModifier::DECLARATION);
    if has_deprecated_annotation(node, src.bytes) {
        mods |= modifier_bit(&SemanticTokenModifier::DEPRECATED);
    }
    if let Some(name) = child_ident(node) {
        push_token(
            name,
            type_index(&SemanticTokenType::NAMESPACE),
            mods,
            src,
            out,
        );
    }
}

fn kotlin_companion_token(node: Node<'_>, src: &Source<'_>, out: &mut Vec<RawToken>) {
    let mods = modifier_bit(&SemanticTokenModifier::DECLARATION)
        | modifier_bit(&SemanticTokenModifier::STATIC);
    let ns_type = type_index(&SemanticTokenType::NAMESPACE);
    if let Some(name) = child_ident(node) {
        push_token(name, ns_type, mods, src, out);
    } else if let Some(obj_kw) = first_child_of_kind(node, "object") {
        push_token(obj_kw, ns_type, mods, src, out);
    }
}

fn kotlin_fun_token(node: Node<'_>, src: &Source<'_>, out: &mut Vec<RawToken>) {
    let token_type = if has_modifier(node, src, "operator") {
        type_index(&SemanticTokenType::OPERATOR)
    } else if is_inside_class_body(node) {
        type_index(&SemanticTokenType::METHOD)
    } else {
        type_index(&SemanticTokenType::FUNCTION)
    };
    let mut mods = modifier_bit(&SemanticTokenModifier::DECLARATION);
    if has_modifier(node, src, "suspend") {
        mods |= modifier_bit(&SemanticTokenModifier::ASYNC);
    }
    if has_modifier(node, src, "abstract") {
        mods |= modifier_bit(&SemanticTokenModifier::ABSTRACT);
    }
    if has_deprecated_annotation(node, src.bytes) {
        mods |= modifier_bit(&SemanticTokenModifier::DEPRECATED);
    }
    if is_in_companion_body(node) || is_top_level(node) {
        mods |= modifier_bit(&SemanticTokenModifier::STATIC);
    }
    if let Some(name) = child_ident(node) {
        push_token(name, token_type, mods, src, out);
    }
}

fn kotlin_prop_token(node: Node<'_>, src: &Source<'_>, out: &mut Vec<RawToken>) {
    let is_val = first_child_of_kind(node, KIND_BINDING_PATTERN_KIND)
        .map(|bpk| has_keyword_child(bpk, KIND_KW_VAL))
        .unwrap_or_else(|| has_keyword_child(node, KIND_KW_VAL));
    let token_type = if is_inside_class_body(node) {
        type_index(&SemanticTokenType::PROPERTY)
    } else {
        type_index(&SemanticTokenType::VARIABLE)
    };
    let mut mods = modifier_bit(&SemanticTokenModifier::DECLARATION);
    if is_val {
        mods |= modifier_bit(&SemanticTokenModifier::READONLY);
    }
    if has_deprecated_annotation(node, src.bytes) {
        mods |= modifier_bit(&SemanticTokenModifier::DEPRECATED);
    }
    if is_in_companion_body(node) || is_top_level(node) {
        mods |= modifier_bit(&SemanticTokenModifier::STATIC);
    }
    if let Some(var_decl) = first_child_of_kind(node, KIND_VAR_DECL) {
        if let Some(name) = child_ident(var_decl) {
            push_token(name, token_type, mods, src, out);
        }
    } else if let Some(multi) = first_child_of_kind(node, KIND_MULTI_VAR_DECL) {
        let mut cursor = multi.walk();
        if cursor.goto_first_child() {
            loop {
                let vd = cursor.node();
                if vd.is_named() {
                    if let Some(name) = child_ident(vd) {
                        push_token(name, token_type, mods, src, out);
                    }
                }
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }
}

fn kotlin_type_param_token(node: Node<'_>, src: &Source<'_>, out: &mut Vec<RawToken>) {
    let mods = modifier_bit(&SemanticTokenModifier::DECLARATION);
    if let Some(ident) = first_child_of_kind(node, KIND_TYPE_IDENT)
        .or_else(|| first_child_of_kind(node, KIND_SIMPLE_IDENT))
    {
        push_token(
            ident,
            type_index(&SemanticTokenType::TYPE_PARAMETER),
            mods,
            src,
            out,
        );
    }
}

/// Emits a KEYWORD token for the literal `constructor` keyword of an
/// annotated/modified primary constructor (`class Foo @Inject constructor(...)`).
///
/// The ABI-15 tree-sitter-kotlin grammar (`fwcd/tree-sitter-kotlin` main as
/// of 2026-02) no longer represents `constructor` as its own node at all —
/// confirmed empirically, not just renamed — so there's nothing to dispatch
/// on directly. `constructor` is only written out when the primary
/// constructor has modifiers/annotations before it (an implicit primary
/// constructor, e.g. `class Foo(val x: Int)`, never writes the word at all),
/// so this scans only the gap between the last modifier and the parameter
/// list's `(` for the literal text, and does nothing if it isn't there.
fn kotlin_primary_constructor_keyword_token(
    node: Node<'_>,
    src: &Source<'_>,
    out: &mut Vec<RawToken>,
) {
    let gap_start =
        first_child_of_kind(node, "modifiers").map_or(node.start_byte(), |m| m.end_byte());
    let Some(lparen) = first_child_of_kind(node, "(") else {
        return;
    };
    let gap_end = lparen.start_byte();
    if gap_end <= gap_start {
        return;
    }
    let Ok(gap_text) = std::str::from_utf8(&src.bytes[gap_start..gap_end]) else {
        return;
    };
    let Some(offset_in_gap) = trivia_skipped_offset(gap_text) else {
        return;
    };
    if !gap_text[offset_in_gap..].starts_with("constructor") {
        return;
    }
    push_token_at_byte_range(
        gap_start + offset_in_gap,
        "constructor".len(),
        type_index(&SemanticTokenType::KEYWORD),
        0,
        src,
        out,
    );
}

/// Byte offset of the first non-trivia (non-whitespace, non-comment) content
/// in `text`, or `None` if it's all trivia. Only `//` line comments and
/// `/* */` block comments can appear in the constructor-keyword gap — no
/// identifier can legally precede `constructor` there — so skipping them
/// (rather than a raw substring search) keeps a `// mentions constructor`
/// comment from being highlighted as the keyword.
fn trivia_skipped_offset(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes[i..].starts_with(b"//") {
            i += text[i..].find('\n').unwrap_or(bytes.len() - i);
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            i += text[i..].find("*/").map_or(bytes.len() - i, |end| end + 2);
            continue;
        }
        break;
    }
    (i < bytes.len()).then_some(i)
}

fn kotlin_class_param_token(node: Node<'_>, src: &Source<'_>, out: &mut Vec<RawToken>) {
    let has_val = first_child_of_kind(node, KIND_BINDING_PATTERN_KIND)
        .is_some_and(|bpk| has_keyword_child(bpk, KIND_KW_VAL));
    let has_var = first_child_of_kind(node, KIND_BINDING_PATTERN_KIND)
        .is_some_and(|bpk| has_keyword_child(bpk, "var"));
    let Some(name) = child_ident(node) else {
        return;
    };
    let (token_type, mut mods) = if has_val {
        (
            type_index(&SemanticTokenType::PROPERTY),
            modifier_bit(&SemanticTokenModifier::DECLARATION)
                | modifier_bit(&SemanticTokenModifier::READONLY),
        )
    } else if has_var {
        (
            type_index(&SemanticTokenType::PROPERTY),
            modifier_bit(&SemanticTokenModifier::DECLARATION),
        )
    } else {
        (
            type_index(&SemanticTokenType::PARAMETER),
            modifier_bit(&SemanticTokenModifier::DECLARATION),
        )
    };
    if is_in_companion_body(node) {
        mods |= modifier_bit(&SemanticTokenModifier::STATIC);
    }
    if has_deprecated_annotation(node, src.bytes) {
        mods |= modifier_bit(&SemanticTokenModifier::DEPRECATED);
    }
    push_token(name, token_type, mods, src, out);
}
