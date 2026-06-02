use super::cursor::CursorContext;
use super::Backend;
use crate::parser::parse_by_extension;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

fn locs_to_response(locs: Vec<Location>) -> GotoDefinitionResponse {
    match locs.len() {
        1 => {
            GotoDefinitionResponse::Scalar(locs.into_iter().next().expect("len == 1 by match arm"))
        }
        _ => GotoDefinitionResponse::Array(locs),
    }
}

/// Converts a possibly-empty location list into an optional LSP response.
fn locs_to_opt_response(locs: Vec<Location>) -> Option<GotoDefinitionResponse> {
    match locs.len() {
        0 => None,
        1 => Some(GotoDefinitionResponse::Scalar(
            locs.into_iter().next().expect("len == 1 by match arm"),
        )),
        _ => Some(GotoDefinitionResponse::Array(locs)),
    }
}

impl Backend {
    pub(super) async fn goto_definition_impl(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let pp = params.text_document_position_params;
        let uri = &pp.text_document.uri;
        let position = pp.position;

        let Some(ctx) = CursorContext::build(&self.indexer, uri, position) else {
            return Ok(None);
        };

        // Special case: `this` keyword — navigate to the enclosing class definition.
        if ctx.qualifier.is_none() && ctx.word == "this" {
            if let Some(class_name) = self.indexer.enclosing_class_at(uri, position.line) {
                let locs = self
                    .indexer
                    .find_definition_qualified(&class_name, None, uri);
                if !locs.is_empty() {
                    return Ok(Some(locs_to_response(locs)));
                }
            }
            return Ok(None);
        }

        // Special case: `super` keyword — navigate to the enclosing class's first supertype.
        if ctx.qualifier.is_none() && ctx.word == "super" {
            if let Some(result) = self.goto_super_class(uri, position.line).await {
                return Ok(Some(result));
            }
            return Ok(None);
        }

        // Special case: `super.method(...)` — resolve `method` in the parent class.
        if ctx.qualifier.as_deref() == Some("super") {
            if let Some(result) = self.goto_super_method(uri, position.line, &ctx.word).await {
                return Ok(Some(result));
            }
            return Ok(None);
        }

        // `it` / named lambda parameter — resolve to the element/receiver type class.
        if ctx.qualifier.is_none() {
            if let Some(ref rt) = ctx.contextual {
                let lookup = rt.leaf.as_str();
                let locs = self.indexer.find_definition_qualified(lookup, None, uri);
                if !locs.is_empty() {
                    return Ok(Some(locs_to_response(locs)));
                }
            }
            // Lambda parameter with failed type inference — jump to `{ name -> }`.
            if let Some(loc) = ctx.lambda_decl.as_ref() {
                return Ok(Some(GotoDefinitionResponse::Scalar(loc.clone())));
            }
        }

        // `this.field` / `it.field` — use the already-resolved contextual receiver
        // so lookup finds the member in the correct class.
        if ctx.qualifier.is_some() {
            if let Some(ref rt) = ctx.contextual {
                let locs = self.resolve_with_receiver_fallback(&ctx.word, rt, uri);
                if !locs.is_empty() {
                    return Ok(Some(locs_to_response(locs)));
                }
            }
        }

        let locs = self
            .indexer
            .find_definition_qualified(&ctx.word, ctx.qualifier.as_deref(), uri);
        if !locs.is_empty() {
            return Ok(locs_to_opt_response(locs));
        }

        // Index miss (symbol not indexed or indexing in progress) → rg fallback.
        // Use effective_rg_root so searches use the open file's project root
        // when workspace_root points to a different project (e.g. android vs ios).
        let file_path = uri.to_file_path().ok();
        let (root_opt, scoped_paths, matcher) = self.rg_scope_for_file(file_path.as_deref()).await;
        let name_clone = ctx.word.clone();
        let rg_locs = tokio::task::spawn_blocking(move || {
            crate::rg::rg_find_definition(
                &name_clone,
                root_opt.as_deref(),
                &scoped_paths,
                matcher.as_deref(),
            )
        })
        .await
        .unwrap_or_default();
        Ok(locs_to_opt_response(rg_locs))
    }

    pub(super) async fn goto_implementation_impl(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let pp = params.text_document_position_params;
        let uri = &pp.text_document.uri;
        let position = pp.position;

        let Some((word, _qualifier)) = self.indexer.word_and_qualifier_at(uri, position) else {
            return Ok(None);
        };

        // Direct subtypes from the index.
        let mut locs: Vec<Location> = self
            .indexer
            .subtypes
            .get(&word)
            .map(|v| v.clone())
            .unwrap_or_default();

        // If index is empty for this symbol (cold start), try rg-based heuristic
        // to find implementors quickly to avoid client timeouts in large projects.
        if locs.is_empty() {
            let file_path = uri.to_file_path().ok();
            let (root_opt, scoped_paths, matcher) =
                self.rg_scope_for_file(file_path.as_deref()).await;
            let word_clone = word.clone();
            let rg_impls = tokio::task::spawn_blocking(move || {
                crate::rg::rg_find_implementors(
                    &word_clone,
                    root_opt.as_deref(),
                    &scoped_paths,
                    matcher.as_deref(),
                )
            })
            .await
            .unwrap_or_default();
            if !rg_impls.is_empty() {
                // Return early with rg results.
                return Ok(locs_to_opt_response(rg_impls));
            }
        }

        // Also collect transitive subtypes (BFS, depth-limited).
        let mut queue: Vec<String> = locs
            .iter()
            .filter_map(|loc| {
                let data = self.indexer.files.get(loc.uri.as_str())?;
                data.symbols
                    .iter()
                    .find(|s| s.selection_range == loc.range)
                    .map(|s| s.name.clone())
            })
            .collect();
        let mut visited = vec![word.clone()];
        while let Some(name) = queue.pop() {
            if visited.contains(&name) {
                continue;
            }
            visited.push(name.clone());
            if let Some(sub_locs) = self.indexer.subtypes.get(&name) {
                for loc in sub_locs.iter() {
                    if !locs
                        .iter()
                        .any(|l| l.uri == loc.uri && l.range == loc.range)
                    {
                        locs.push(loc.clone());
                        if let Some(data) = self.indexer.files.get(loc.uri.as_str()) {
                            if let Some(sym) =
                                data.symbols.iter().find(|s| s.selection_range == loc.range)
                            {
                                queue.push(sym.name.clone());
                            }
                        }
                    }
                }
            }
        }

        Ok(locs_to_opt_response(locs))
    }

    /// Collect the parent class names for the class enclosing `row` in `uri`.
    pub(super) fn super_names_at(&self, uri: &Url, row: u32) -> Vec<String> {
        let class_name = match self.indexer.enclosing_class_at(uri, row) {
            Some(n) => n,
            None => return vec![],
        };
        let locs = self
            .indexer
            .definitions
            .get(&class_name)
            .map(|v| v.clone())
            .unwrap_or_default();
        for loc in &locs {
            if let Some(file) = self.indexer.files.get(loc.uri.as_str()) {
                let names: Vec<String> = file
                    .supers
                    .iter()
                    .filter(|(l, _, _)| *l == loc.range.start.line)
                    .map(|(_, n, _)| n.clone())
                    .collect();
                if !names.is_empty() {
                    return names;
                }
            }
        }
        // Fallback: parse live_lines for the open file itself.
        if let Some(lines) = self.indexer.live_lines.get(uri.as_str()) {
            let content = lines.join("\n");
            let names: Vec<String> = parse_by_extension(uri.path(), &content)
                .supers
                .into_iter()
                .map(|(_, n, _)| n)
                .collect();
            if !names.is_empty() {
                return names;
            }
        }
        vec![]
    }

    pub(super) async fn rg_resolve(&self, uri: &Url, name: &str) -> Vec<Location> {
        let name_clone = name.to_string();
        let file_path = uri.to_file_path().ok();
        let (root_opt, scoped_paths, matcher) = self.rg_scope_for_file(file_path.as_deref()).await;
        tokio::task::spawn_blocking(move || {
            crate::rg::rg_find_definition(
                &name_clone,
                root_opt.as_deref(),
                &scoped_paths,
                matcher.as_deref(),
            )
        })
        .await
        .unwrap_or_default()
    }

    pub(super) async fn goto_super_class(
        &self,
        uri: &Url,
        row: u32,
    ) -> Option<GotoDefinitionResponse> {
        for super_name in &self.super_names_at(uri, row) {
            let locs = self
                .indexer
                .find_definition_qualified(super_name, None, uri);
            if !locs.is_empty() {
                return Some(locs_to_response(locs));
            }
            let rg_locs = self.rg_resolve(uri, super_name).await;
            if !rg_locs.is_empty() {
                return Some(locs_to_response(rg_locs));
            }
        }
        None
    }

    pub(super) async fn goto_super_method(
        &self,
        uri: &Url,
        row: u32,
        method: &str,
    ) -> Option<GotoDefinitionResponse> {
        // resolve_qualified already handles root=="super" via resolve_from_class_hierarchy.
        let locs = self
            .indexer
            .find_definition_qualified(method, Some("super"), uri);
        if !locs.is_empty() {
            return Some(locs_to_response(locs));
        }
        // Method not found in indexed hierarchy (e.g. Android SDK parent).
        // Fall back to navigating to the parent class itself.
        self.goto_super_class(uri, row).await
    }
}

impl Backend {
    /// Go to the type definition for the symbol at cursor position.
    ///
    /// Unlike `goto_definition` (which navigates to the symbol itself), this resolves
    /// the *type* of the symbol under the cursor and navigates to that type's definition.
    ///
    /// Patterns handled:
    /// - `val x: Foo` / `var x: Foo` — navigates to `Foo`
    /// - `fun foo(): Bar` — navigates to `Bar`
    /// - `it` / named lambda params — navigates to the inferred receiver type
    /// - `this` — navigates to the enclosing class
    /// - Falls back to `goto_definition` when no type-specific target is available.
    pub(super) async fn goto_type_definition_impl(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let pp = params.text_document_position_params;
        let uri = &pp.text_document.uri;
        let position = pp.position;

        let Some(ctx) = CursorContext::build(&self.indexer, uri, position) else {
            return Ok(None);
        };

        // 1. `this` → enclosing class definition.
        if ctx.qualifier.is_none() && ctx.word == "this" {
            if let Some(class_name) = self.indexer.enclosing_class_at(uri, position.line) {
                let locs = self
                    .indexer
                    .find_definition_qualified(&class_name, None, uri);
                if !locs.is_empty() {
                    return Ok(Some(locs_to_response(locs)));
                }
            }
            return Ok(None);
        }

        // 2. `it` / named lambda parameter with inferred type → navigate to the type.
        if ctx.qualifier.is_none() {
            if let Some(ref rt) = ctx.contextual {
                let lookup = rt.leaf.as_str();
                let locs = self.indexer.find_definition_qualified(lookup, None, uri);
                if !locs.is_empty() {
                    return Ok(Some(locs_to_response(locs)));
                }
            }
        }

        // 3. `it.field` / `this.field` — resolve member type via contextual receiver.
        if ctx.qualifier.is_some() {
            if let Some(ref rt) = ctx.contextual {
                let locs = self.resolve_with_receiver_fallback(&ctx.word, rt, uri);
                if !locs.is_empty() {
                    // Found the member definition — now try to resolve its type.
                    let loc = locs[0].clone();
                    if let Some(type_name) =
                        self.extract_type_from_definition(&loc, &ctx.word, uri, position.line)
                    {
                        let type_locs = self
                            .indexer
                            .find_definition_qualified(&type_name, None, uri);
                        if !type_locs.is_empty() {
                            return Ok(Some(locs_to_response(type_locs)));
                        }
                    }
                }
            }
        }

        // 4. Regular symbol: try to resolve its type from the signature.
        if let Some(type_name) = self.extract_type_from_ctx(&ctx, uri, position.line) {
            let locs = self
                .indexer
                .find_definition_qualified(&type_name, None, uri);
            if !locs.is_empty() {
                return Ok(Some(locs_to_response(locs)));
            }
        }

        // 5. Fall back to regular goto_definition.
        self.goto_definition_impl(GotoDefinitionParams {
            text_document_position_params: pp,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .await
    }

    /// Try to extract a type name from a resolved symbol's signature.
    ///
    /// Handles `val x: TypeName`, `var x: TypeName`, `fun foo(): ReturnType`,
    /// and constructor parameter signatures.
    fn extract_type_from_signature(&self, sig: &str) -> Option<String> {
        // `val x: SomeType<...>` or `var x: SomeType<...>`
        if sig.starts_with("val ") || sig.starts_with("var ") {
            if let Some(after_colon) = sig.split(':').nth(1) {
                let trimmed = after_colon.trim();
                // Take the first identifier (strip generics).
                let type_name = trimmed.split(['<', '(', ' ', '?']).next()?.trim();
                if !type_name.is_empty() {
                    return Some(type_name.to_owned());
                }
            }
        }
        // `fun foo(): ReturnType`
        if sig.starts_with("fun ") {
            // Find the closing paren and look for `: ReturnType` after it.
            if let Some(after_colon) = sig.split("): ").nth(1).or_else(|| sig.split("):").nth(1)) {
                let trimmed = after_colon.trim();
                let type_name = trimmed.split(['<', ' ', '?']).next()?.trim();
                if !type_name.is_empty() {
                    return Some(type_name.to_owned());
                }
            }
        }
        None
    }

    /// Extract the type name from the symbol at cursor position using `resolve_symbol_info`.
    fn extract_type_from_ctx(&self, ctx: &CursorContext, uri: &Url, line: u32) -> Option<String> {
        use crate::indexer::resolution::{
            resolve_symbol_info, ResolveOptions, SubstitutionContext,
        };
        let info = resolve_symbol_info(
            self.indexer.as_ref(),
            &ctx.word,
            ctx.qualifier.as_deref(),
            uri,
            SubstitutionContext::CrossFile {
                calling_uri: uri.as_str(),
                cursor_line: Some(line),
            },
            &ResolveOptions {
                allow_rg: true,
                include_doc: false,
                apply_subst: true,
                prefer_cached_detail: false,
            },
        )?;
        self.extract_type_from_signature(&info.signature)
    }

    /// Extract the type name for a symbol found at the given location.
    fn extract_type_from_definition(
        &self,
        loc: &Location,
        word: &str,
        uri: &Url,
        line: u32,
    ) -> Option<String> {
        use crate::indexer::resolution::{enrich_at_location, ResolveOptions, SubstitutionContext};
        let info = enrich_at_location(
            self.indexer.as_ref(),
            loc,
            word,
            SubstitutionContext::CrossFile {
                calling_uri: uri.as_str(),
                cursor_line: Some(line),
            },
            &ResolveOptions {
                allow_rg: true,
                include_doc: false,
                apply_subst: true,
                prefer_cached_detail: false,
            },
        )?;
        self.extract_type_from_signature(&info.signature)
    }
}
