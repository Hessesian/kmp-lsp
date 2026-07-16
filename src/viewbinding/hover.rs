//! ViewBinding-specific hover helpers extracted from the generic hover pipeline.

use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Url};

use crate::backend::cursor::CursorContext;
use crate::backend::format::{format_contextual_hover, format_symbol_hover};
use crate::indexer::resolution::{
    resolve_symbol_info, ResolveOptions, SubstitutionContext, WorkspaceRead,
};
use crate::viewbinding::navigation::{
    binding_field_hover_at_location, binding_field_hover_for_class, resolve_expected_binding_class,
};
use crate::Language;
use crate::StrExt;

/// Hover for bare binding-field access inside `with(binding)` / `binding.apply`.
pub(crate) fn binding_field_access_hover<W: WorkspaceRead>(
    workspace: &W,
    ctx: &CursorContext,
    uri: &Url,
    position: Position,
) -> Option<Hover> {
    let indexer = workspace.as_indexer()?;
    if ctx.word.starts_with_uppercase() {
        return None;
    }
    let expected_class = resolve_expected_binding_class(indexer, uri, position, ctx, None)?;
    let markdown = binding_field_hover_for_class(indexer, uri, &expected_class, &ctx.word)?;
    Some(make_markdown_hover(markdown))
}

/// Fallback hover for local binding variables when symbol resolution misses.
pub(crate) fn fallback_local_binding_hover<W: WorkspaceRead>(
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
    let signature = format!(
        "{} {}: {type_name}",
        Language::from_path(uri.path()).val_keyword(),
        ctx.word
    );
    let (leaf, qualifier) = type_detail_parts(&type_name);
    let detail = resolve_detail_markdown(workspace, leaf, qualifier, uri, line)
        .or_else(|| crate::stdlib::hover(leaf));
    Some(make_markdown_hover(format_contextual_hover(
        &signature,
        uri.path(),
        detail.as_deref(),
    )))
}

fn resolve_detail_markdown<W: WorkspaceRead>(
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
        SubstitutionContext::CrossFile {
            calling_uri: uri.as_str(),
            cursor_line: Some(line),
        },
        &ResolveOptions::hover(),
    )
    .map(|info| {
        binding_field_hover_at_location(workspace, &info.location, &info.name)
            .unwrap_or_else(|| format_symbol_hover(&info, uri.path()))
    })
}

fn type_detail_parts(type_name: &str) -> (&str, Option<&str>) {
    let base = type_name
        .split('<')
        .next()
        .unwrap_or(type_name)
        .trim_end_matches('?');
    match base.rsplit_once('.') {
        Some((qualifier, leaf)) => (leaf, Some(qualifier)),
        None => (base, None),
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
