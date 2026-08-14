//! Hover feature — rich Markdown hover computed from the index and live cursor context.
//!
//! Uses `WorkspaceRead` as the capability bound rather than the new capability traits because
//! the underlying resolution pipeline (`resolve_symbol_info`, `enrich_at_location`,
//! `build_subst_map`) depends on `IndexRead`, and `WorkspaceRead: IndexRead`.
//! Migrating these to the new traits is tracked as part of F5 cleanup.

use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Url};

use crate::backend::cursor::CursorContext;
use crate::backend::format::{format_contextual_hover, format_symbol_hover};
use crate::indexer::apply_type_subst;
use crate::indexer::resolution::{
    build_subst_map, enrich_at_location, resolve_symbol_info, ResolveOptions, SubstitutionContext,
    WorkspaceRead,
};
use crate::resolver::ReceiverType;
use crate::StrExt;

/// Compute a hover response for the cursor at `position` in `uri`.
///
/// Returns `None` when no useful hover information is available (unknown symbol,
/// cursor on a keyword, etc.).
pub(crate) fn compute_hover<W: WorkspaceRead>(
    workspace: &W,
    ctx: &CursorContext,
    uri: &Url,
    position: Position,
) -> Option<Hover> {
    if let Some(hover) = contextual_lambda_hover(workspace, ctx, uri, position) {
        return Some(hover);
    }
    if ctx.qualifier.is_none() && ctx.lambda_decl.is_some() {
        return jar_loading_hint(workspace);
    }
    if let Some(hover) = contextual_receiver_hover(workspace, ctx, uri, position) {
        return Some(hover);
    }
    regular_symbol_hover(workspace, ctx, uri, position).or_else(|| jar_loading_hint(workspace))
}

fn contextual_lambda_hover<W: WorkspaceRead>(
    workspace: &W,
    ctx: &CursorContext,
    uri: &Url,
    position: Position,
) -> Option<Hover> {
    if ctx.qualifier.is_some() {
        return None;
    }
    let receiver_type = ctx.contextual.as_ref()?;
    let type_name = contextual_hover_type_name(workspace, receiver_type, uri, position.line);
    let (leaf, qualifier) = type_detail_parts(&type_name);
    let signature = format!("{} {}: {type_name}", hover_binding_keyword(uri), ctx.word);
    let detail = resolve_hover_markdown(workspace, leaf, qualifier, uri, position.line)
        .or_else(|| crate::stdlib::hover(leaf));
    Some(make_markdown_hover(format_contextual_hover(
        &signature,
        uri.path(),
        detail.as_deref(),
    )))
}

fn contextual_hover_type_name<W: WorkspaceRead>(
    workspace: &W,
    receiver_type: &ReceiverType,
    uri: &Url,
    line: u32,
) -> String {
    let subst = build_subst_map(workspace, uri.as_str(), line);
    if subst.is_empty() {
        return receiver_type.raw.clone();
    }
    apply_type_subst(&receiver_type.raw, &subst)
}

fn contextual_receiver_hover<W: WorkspaceRead>(
    workspace: &W,
    ctx: &CursorContext,
    uri: &Url,
    position: Position,
) -> Option<Hover> {
    let receiver_type = ctx.contextual.as_ref()?;
    ctx.qualifier.as_ref()?;
    let location = resolve_with_receiver_fallback(workspace, &ctx.word, receiver_type, uri)
        .into_iter()
        .next()?;
    let info = enrich_at_location(
        workspace,
        &location,
        &ctx.word,
        hover_substitution_context(uri, position.line),
        &ResolveOptions::hover(),
    )?;
    Some(make_markdown_hover(format_symbol_hover(&info, uri.path())))
}

fn regular_symbol_hover<W: WorkspaceRead>(
    workspace: &W,
    ctx: &CursorContext,
    uri: &Url,
    position: Position,
) -> Option<Hover> {
    if ctx.qualifier.is_none() {
        if let Some(hover) = call_callee_hover(workspace, ctx, uri, position) {
            return hover;
        }
    }
    let markdown = resolve_hover_markdown(
        workspace,
        &ctx.word,
        ctx.qualifier.as_deref(),
        uri,
        position.line,
    )
    .or_else(|| crate::stdlib::hover(&ctx.word));
    if let Some(markdown) = markdown {
        return Some(make_markdown_hover(markdown));
    }
    fallback_local_binding_hover(workspace, ctx, uri, position.line)
}

/// `Some(hover)` when the cursor sits on a call's callee and the call's own
/// shape resolved it (`hover` itself may be `None`, meaning: don't fall
/// through to the unfiltered lookups below — an arity-filtered miss is a
/// deliberate "the same-file candidate can't be the target", not "give the
/// name-only path another try", which would just re-find the same wrong-arity
/// match `find_definition_for_call` already ruled out. `None` (not
/// `Some(None)`) means the cursor isn't on a call's callee at all, so the
/// normal name-based hover path should run as before.
///
/// Same shape computation `goto_definition` already uses (see
/// `call_shape_at_callee`) — same underlying bug (`resolve_local`/
/// `resolve_chain` matching same-file candidates by name alone, oblivious to
/// arity), reached through hover's separate `resolve_symbol`-based path.
fn call_callee_hover<W: WorkspaceRead>(
    workspace: &W,
    ctx: &CursorContext,
    uri: &Url,
    position: Position,
) -> Option<Option<Hover>> {
    let indexer = workspace.as_indexer()?;
    let shape = crate::features::definition::call_shape_at_callee(indexer, uri, position)?;
    let mut locations = indexer.find_definition_for_call(&ctx.word, uri, shape);
    if locations.is_empty() {
        // Same reasoning as goto-definition: a bare call inside an extension
        // function's own body may target a same-named member/extension of
        // that function's *own* receiver (an implicit `this.name(...)`),
        // which the plain bare-name lookup above has no way to see.
        if let Some(receiver) =
            crate::features::definition::enclosing_extension_receiver_at(indexer, uri, position)
        {
            locations = indexer
                .find_definition_for_implicit_receiver_call(&receiver, &ctx.word, uri, shape);
        }
    }
    let location = locations.into_iter().next();
    Some(location.and_then(|location| {
        let info = enrich_at_location(
            workspace,
            &location,
            &ctx.word,
            hover_substitution_context(uri, position.line),
            &ResolveOptions::hover(),
        )?;
        Some(make_markdown_hover(format_symbol_hover(&info, uri.path())))
    }))
}

fn fallback_local_binding_hover<W: WorkspaceRead>(
    workspace: &W,
    ctx: &CursorContext,
    uri: &Url,
    line: u32,
) -> Option<Hover> {
    if ctx.qualifier.is_some() {
        return None;
    }
    let indexer = workspace.as_indexer()?;
    let type_name = crate::resolver::infer::infer_variable_type_raw(indexer, &ctx.word, uri)?;
    let signature = format!("{} {}: {type_name}", hover_binding_keyword(uri), ctx.word);
    let (leaf, qualifier) = type_detail_parts(&type_name);
    let detail = resolve_hover_markdown(workspace, leaf, qualifier, uri, line)
        .or_else(|| crate::stdlib::hover(leaf));
    Some(make_markdown_hover(format_contextual_hover(
        &signature,
        uri.path(),
        detail.as_deref(),
    )))
}

fn resolve_hover_markdown<W: WorkspaceRead>(
    workspace: &W,
    word: &str,
    qualifier: Option<&str>,
    uri: &Url,
    line: u32,
) -> Option<String> {
    resolve_symbol_info(
        workspace,
        word,
        qualifier,
        uri,
        hover_substitution_context(uri, line),
        &ResolveOptions::hover(),
    )
    .map(|info| format_symbol_hover(&info, uri.path()))
}

/// Resolve a symbol name with receiver-type fallback.
///
/// Tries the fully-qualified receiver name first; on miss, falls back to the
/// leaf type name (e.g. `DashboardViewModel` instead of `com.example.DashboardViewModel`).
pub(crate) fn resolve_with_receiver_fallback<W: WorkspaceRead>(
    workspace: &W,
    word: &str,
    rt: &ReceiverType,
    uri: &Url,
) -> Vec<tower_lsp::lsp_types::Location> {
    let locs = workspace.find_definition_qualified(word, Some(&rt.qualified), uri);
    if locs.is_empty() && rt.leaf != rt.qualified {
        workspace.find_definition_qualified(word, Some(&rt.leaf), uri)
    } else {
        locs
    }
}

/// Split a potentially-generic, nullable type name into (leaf, qualifier) for detail lookup.
///
/// Strips generic params and `?` before splitting on `.`:
/// - `"ResultState.Success<Optional<FamilyAccount>>"` → `("Success", Some("ResultState"))`
/// - `"Optional<FamilyAccount>"` → `("Optional", None)`
/// - `"User?"` → `("User", None)`
/// - `"FamilyAccount"` → `("FamilyAccount", None)`
fn type_detail_parts(type_name: &str) -> (&str, Option<&str>) {
    let base = type_name
        .split('<')
        .next()
        .unwrap_or(type_name)
        .strip_nullable()
        .trim_end_matches('.');
    match base.rsplit_once('.') {
        Some((qualifier, leaf)) => (leaf, Some(qualifier)),
        None => (base, None),
    }
}

fn hover_binding_keyword(uri: &Url) -> &'static str {
    crate::Language::from_path(uri.path()).val_keyword()
}

fn hover_substitution_context(uri: &Url, line: u32) -> SubstitutionContext<'_> {
    SubstitutionContext::CrossFile {
        calling_uri: uri.as_str(),
        cursor_line: Some(line),
    }
}

fn make_markdown_hover(markdown: String) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: None,
    }
}

/// Return a brief "JAR index loading" hover hint when the phase is `Pending`
/// or `InProgress`.  Returns `None` for `Unavailable`, `Ready`, and `Failed`
/// (in those cases the caller should stay silent rather than showing noise).
fn jar_loading_hint<W: WorkspaceRead>(workspace: &W) -> Option<Hover> {
    if workspace.jar_phase().is_loading() {
        Some(make_markdown_hover(
            "_JAR symbols are still indexing — try again in a moment._".to_owned(),
        ))
    } else {
        None
    }
}

#[cfg(test)]
#[path = "hover_tests.rs"]
mod tests;
