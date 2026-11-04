//! The resolver's side of the inference seam: `Indexer` answers the questions
//! the CST inference engine asks through [`InferDeps`] by calling name
//! resolution. It lives here, above the index, so the index never depends on
//! the resolver.

use tower_lsp::lsp_types::{SymbolKind, Url};

use super::{infer_variable_type_from_cst, infer_variable_type_raw};
use crate::indexer::infer::deps::{
    CallShape, CallableInfo, ExtensionCallable, InferDeps, MemberTypeParameters, OuterScopedParams,
    ReceiverCall,
};
use crate::indexer::infer::sig::{collect_signature, find_fun_params_text_fast};
use crate::indexer::{Indexer, MAX_BY_NAME_DEFS};

impl InferDeps for Indexer {
    fn find_fun_params_text(&self, fn_name: &str, uri: &Url) -> Option<String> {
        // Index-only: lambda/receiver inference is a hot path (called per lambda in
        // inlay hints/hover); the old rg + all-files fallback spawned subprocesses for
        // un-indexed stdlib callees (`let`/`forEachIndexed`) and made inlay time out.
        find_fun_params_text_fast(fn_name, self, uri)
    }
    fn find_var_type(&self, var_name: &str, uri: &Url) -> Option<String> {
        infer_variable_type_raw(self, var_name, uri)
            .or_else(|| infer_variable_type_from_cst(self, var_name, uri))
    }
    fn find_field_type(
        &self,
        class_name: &str,
        field_name: &str,
        uri: &Url,
    ) -> Option<(String, Url)> {
        if let Some(type_name) = synthetic_enum_field(self, class_name, field_name) {
            // No real declaration site (compiler-synthesized); the call site's
            // own file is the closest thing to a reachability anchor.
            return Some((type_name, uri.clone()));
        }
        crate::resolver::Resolver::field_type(self, class_name, field_name, uri)
    }
    fn find_fun_return_type(&self, fn_name: &str, uri: &Url) -> Option<String> {
        crate::resolver::infer::find_fun_return_type_by_name(self, fn_name, uri)
    }

    fn find_fun_return_type_reachable_with_origin(
        &self,
        fn_name: &str,
        uri: &Url,
    ) -> Option<(String, Url)> {
        let declared =
            crate::resolver::infer::find_fun_return_type_reachable_declared(self, fn_name, uri)?;
        let declared_in = declared.declared_in.unwrap_or_else(|| uri.clone());
        Some((declared.type_name, declared_in))
    }
    fn find_method_return_nullability_for_type(
        &self,
        class_name: &str,
        method_name: &str,
        uri: &Url,
    ) -> crate::detail_text::ReturnNullability {
        crate::resolver::Resolver::method_return_type_declared(
            self,
            class_name,
            method_name,
            Some(uri),
        )
        .map_or(crate::detail_text::ReturnNullability::Unknown, |declared| {
            declared.nullability
        })
    }
    fn find_fun_return_nullability(
        &self,
        fn_name: &str,
        uri: &Url,
    ) -> crate::detail_text::ReturnNullability {
        crate::resolver::infer::find_fun_return_type_reachable_declared(self, fn_name, uri)
            .map_or(crate::detail_text::ReturnNullability::Unknown, |declared| {
                declared.nullability
            })
    }
    fn find_fun_return_type_reachable(&self, fn_name: &str, uri: &Url) -> Option<String> {
        crate::resolver::infer::find_fun_return_type_reachable(self, fn_name, uri)
    }
    fn find_class_type_params_declared_in(
        &self,
        class_name: &str,
        declaring_uri: &Url,
    ) -> Vec<String> {
        let declared_params = self
            .file_data_for(declaring_uri.as_str())
            .and_then(|file_data| {
                file_data
                    .symbols
                    .iter()
                    .find(|symbol| {
                        let is_class_like = matches!(
                            symbol.kind,
                            SymbolKind::CLASS
                                | SymbolKind::INTERFACE
                                | SymbolKind::ENUM
                                | SymbolKind::STRUCT
                        );
                        is_class_like && symbol.name == class_name
                    })
                    .map(|symbol| symbol.type_params().to_vec())
            });
        declared_params.unwrap_or_else(|| self.find_class_type_params(class_name))
    }
    fn find_class_type_params(&self, class_name: &str) -> Vec<String> {
        if let Some(locations) = self.definitions.get(class_name) {
            for loc in locations.iter() {
                let Some(url) = self.file_table.url(loc.file) else {
                    continue;
                };
                if let Some(file_data) = self.files.get(url.as_str()) {
                    if let Some(sym) = file_data
                        .symbols
                        .iter()
                        .find(|s| s.name == class_name && !s.type_params().is_empty())
                    {
                        return sym.type_params().to_vec();
                    }
                }
            }
        }
        // Fallback: JAR-indexed classes. Same synthetic-line-as-index
        // addressing find_fun_callable_info (this same file) already uses
        // for JAR symbols -- a JAR symbol's "line" is really an index into
        // its file's own `symbols` Vec, not a real line number.
        let mut cache_backed_only = 0usize;
        crate::indexer::jar::ensure_jar_definitions_for(self, class_name, &mut cache_backed_only);
        if let Some(jar_locs) = self.jar_definitions.get(class_name) {
            for loc in jar_locs.iter().take(MAX_BY_NAME_DEFS) {
                if let Some(file_data) = self.jar_files.get(loc.uri.as_str()) {
                    if let Some(sym) = file_data
                        .symbols
                        .get(loc.range.start.line as usize)
                        .filter(|s| s.name == class_name && !s.type_params().is_empty())
                    {
                        return sym.type_params().to_vec();
                    }
                }
            }
        }
        Vec::new()
    }
    fn find_method_return_type_with_origin_for_type(
        &self,
        class_name: &str,
        method_name: &str,
        uri: &Url,
    ) -> Option<(String, Url)> {
        if let Some(type_name) = synthetic_enum_method(self, class_name, method_name) {
            return Some((type_name, uri.clone()));
        }
        // Member fns, extension fns, and supertype inheritance in one pass —
        // the catalogue composite. The calling file's uri is load-bearing for
        // jar receivers: it drives import-reachability AND on-demand Tier-2
        // materialization of jar extension sets (a `None` here left
        // `Modifier.fillMaxSize()`-style receivers unresolvable until some
        // other feature happened to promote the same symbols).
        let declared = crate::resolver::Resolver::method_return_type_declared(
            self,
            class_name,
            method_name,
            Some(uri),
        )?;
        let declared_in = declared.declared_in.unwrap_or_else(|| uri.clone());
        Some((declared.type_name, declared_in))
    }
    fn find_method_return_type_for_type(
        &self,
        class_name: &str,
        method_name: &str,
        uri: &Url,
    ) -> Option<String> {
        self.find_method_return_type_with_origin_for_type(class_name, method_name, uri)
            .map(|(type_name, _)| type_name)
    }
    fn find_method_params_text(
        &self,
        class_name: &str,
        method_name: &str,
        uri: &Url,
        shape: crate::indexer::infer::deps::CallShape,
    ) -> Option<String> {
        crate::indexer::infer::sig::find_method_params_in_class(
            self,
            class_name,
            method_name,
            uri,
            shape,
        )
    }
    fn find_fun_callable_info(
        &self,
        fn_name: &str,
        receiver: Option<ReceiverCall<'_>>,
        uri: &Url,
    ) -> Option<CallableInfo> {
        if let Some(receiver) = receiver {
            match receiver_bound_callable(self, fn_name, receiver, uri) {
                ReceiverBoundCallable::Extension(info) => return info,
                ReceiverBoundCallable::Member => return None,
                ReceiverBoundCallable::NoApplicableExtension => {}
            }
        }
        // Workspace definitions first. The declaration the call actually binds
        // to (import/package-reachable from `uri`) decides; only when nothing
        // is reachable do the capped by-name candidates stand in — an
        // unrelated same-named generic must not donate its type parameters to
        // a callee that declares none.
        let reachable = crate::resolver::resolve_symbol_scoped_only(self, fn_name, uri);
        let candidates = if reachable.is_empty() {
            self.workspace_def_candidates(fn_name)
        } else {
            reachable
        };
        let from_workspace = candidates.iter().find_map(|loc| {
            let file_data = self.files.get(loc.uri.as_str())?;
            let sym = file_data
                .symbols
                .iter()
                .find(|s| s.name == fn_name && !s.type_params().is_empty())?;
            Some(CallableInfo {
                type_params: sym.type_params().to_vec(),
                extension_receiver_type: sym.declared_extension_receiver().to_owned(),
            })
        });
        if from_workspace.is_some() {
            return from_workspace;
        }
        // Fallback: JAR-indexed files (sidecar symbols carry type_params). JAR symbols
        // use a synthetic line == index into `symbols`, so address each entry directly
        // (O(1)) rather than a per-loc linear `find` over the whole jar's symbol list.
        // Promote-before-read (zero budget): inference-deps path, called per-name
        // across a visible range — no blocking sidecar IPC here.
        let mut cache_backed_only = 0usize;
        crate::indexer::jar::ensure_jar_definitions_for(self, fn_name, &mut cache_backed_only);
        let jar_locs = self.jar_definitions.get(fn_name)?;

        // A common name collides across the whole Gradle cache -- `collect` has
        // 175 declarations in a real Android project, of which ~140 are build
        // tooling (the Kotlin compiler, KSP, IntelliJ core) that application
        // code can never import. Taking the first match makes the answer depend
        // on JAR load order, which is on-demand and so varies between sessions.
        // Try the candidates the caller could actually have imported first.
        let caller_file_data = self
            .files
            .get(uri.as_str())
            .map(|entry| entry.value().clone());
        let reachable_first = jar_locs
            .iter()
            .filter(|loc| {
                self.jar_candidate_is_reachable(loc, fn_name, caller_file_data.as_deref())
            })
            .chain(jar_locs.iter())
            .take(MAX_BY_NAME_DEFS);

        for loc in reachable_first {
            if let Some(file_data) = self.jar_files.get(loc.uri.as_str()) {
                if let Some(sym) = file_data
                    .symbols
                    .get(loc.range.start.line as usize)
                    .filter(|s| s.name == fn_name && !s.type_params().is_empty())
                {
                    return Some(CallableInfo {
                        type_params: sym.type_params().to_vec(),
                        extension_receiver_type: sym.declared_extension_receiver().to_owned(),
                    });
                }
            }
        }
        None
    }
    fn find_contextual_type(
        &self,
        name: &str,
        uri: &Url,
        line: usize,
        col: usize,
    ) -> Option<String> {
        use tower_lsp::lsp_types::Position;
        self.infer_lambda_param_type_at(name, uri, Position::new(line as u32, col as u32))
    }
    fn find_outer_scoped_fun_params(
        &self,
        outer: &str,
        fn_name: &str,
        uri: &Url,
    ) -> OuterScopedParams {
        // Index-only lookup (no rg) — this runs on the inlay-hints/hover hot path.
        let outer_locations = self.resolve_symbol_no_rg(outer, uri);
        let Some(outer_location) = outer_locations.first() else {
            // Not indexed yet — enrich in the background so a later request resolves.
            self.submit_enrichment(outer);
            return OuterScopedParams::ForbidGlobalFallback;
        };
        OuterScopedParams::Candidates(crate::indexer::infer::sig::collect_all_fun_params_texts(
            fn_name,
            outer_location.uri.as_str(),
            self,
        ))
    }
    fn has_type_definition(&self, name: &str) -> bool {
        self.definition_locations(name).into_iter().any(|loc| {
            self.files
                .get(loc.uri.as_str())
                .or_else(|| self.jar_files.get(loc.uri.as_str()))
                .map(|file_data| {
                    file_data.symbols.iter().any(|symbol| {
                        symbol.name == name
                            && matches!(
                                symbol.kind,
                                SymbolKind::CLASS
                                    | SymbolKind::INTERFACE
                                    | SymbolKind::ENUM
                                    | SymbolKind::OBJECT
                                    | SymbolKind::STRUCT
                            )
                    })
                })
                .unwrap_or(false)
        })
    }
    fn names_type_in_scope(&self, type_text: &str, uri: &Url) -> bool {
        let Some(type_path) = crate::StrExt::bare_type_name(type_text) else {
            return false;
        };
        let is_builtin = crate::text_utils::KOTLIN_BUILTIN_TYPES.contains(&type_path.as_str());
        if is_builtin {
            return true;
        }
        let path_resolves = || {
            !crate::resolver::resolve_type_path_declarations(
                self,
                &type_path,
                uri,
                crate::resolver::ResolveIo::IndexOnly,
            )
            .is_empty()
        };
        // `resolve_type_path_declarations` falls back past `uri`'s scope for
        // a bare root; a package-qualified path names its type outright.
        let root = type_path.split('.').next().unwrap_or(&type_path);
        let is_package_qualified = crate::StrExt::starts_with_lowercase(root);
        if is_package_qualified {
            return path_resolves();
        }
        let root_in_scope =
            !crate::resolver::resolve_symbol_scoped_only(self, root, uri).is_empty();
        root_in_scope && path_resolves()
    }
    fn same_type_in_scopes(&self, type_text: &str, first_uri: &Url, second_uri: &Url) -> bool {
        let declarations_of = |type_path: &str, uri: &Url| {
            crate::resolver::resolve_type_path_declarations(
                self,
                type_path,
                uri,
                crate::resolver::ResolveIo::IndexOnly,
            )
        };
        let shadows_in_scope = |type_path: &str, uri: &Url| {
            let root = type_path.split('.').next().unwrap_or(type_path);
            let is_package_qualified = crate::StrExt::starts_with_lowercase(root);
            !is_package_qualified
                && !crate::resolver::resolve_symbol_scoped_only(self, root, uri).is_empty()
        };
        type_paths_in(type_text).into_iter().all(|type_path| {
            let is_builtin = crate::text_utils::KOTLIN_BUILTIN_TYPES.contains(&type_path.as_str());
            let builtin_is_shadowed =
                shadows_in_scope(&type_path, first_uri) || shadows_in_scope(&type_path, second_uri);
            if is_builtin && !builtin_is_shadowed {
                return true;
            }
            let first = declarations_of(&type_path, first_uri);
            !first.is_empty() && first == declarations_of(&type_path, second_uri)
        })
    }
    fn find_member_type_params(
        &self,
        fn_name: &str,
        receiver: ReceiverCall<'_>,
        uri: &Url,
    ) -> Option<MemberTypeParameters> {
        crate::resolver::member_type_parameters_taking_call(
            self,
            receiver.receiver_type,
            fn_name,
            uri,
            receiver.shape,
        )
    }
    fn find_extension_callable(
        &self,
        fn_name: &str,
        receiver: ReceiverCall<'_>,
        uri: &Url,
    ) -> Option<ExtensionCallable> {
        let declarations = crate::resolver::extension_declarations_on(
            self,
            receiver.receiver_type,
            fn_name,
            uri,
            receiver.shape,
        );
        let declaration = declaration_selected_by_shape(&declarations, receiver.shape)?;
        let symbol = declaration.symbol()?;
        let (return_type, return_nullability) = declared_return_type(declaration)?;
        Some(ExtensionCallable {
            generics: CallableInfo {
                type_params: symbol.type_params().to_vec(),
                extension_receiver_type: symbol.declared_extension_receiver().to_owned(),
            },
            return_type,
            return_nullability,
            declared_in: declaration.uri.clone(),
        })
    }
}

/// The dotted type names a type text mentions, outermost first:
/// `Map<a.Key, List<Value>?>` -> `Map`, `a.Key`, `List`, `Value`.
fn type_paths_in(type_text: &str) -> Vec<String> {
    type_text
        .split(|char: char| !(char.is_alphanumeric() || char == '_' || char == '.'))
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The return type an extension declaration states, from its detail or, when
/// that is truncated, its source signature.
fn declared_return_type(
    declaration: &crate::resolver::ExtensionDeclaration,
) -> Option<(String, crate::detail_text::ReturnNullability)> {
    let symbol = declaration.symbol()?;
    let from_detail = crate::detail_text::extract_return_type_from_detail(&symbol.detail);
    if let Some(type_name) = from_detail {
        let nullability =
            crate::detail_text::extract_return_nullability_from_detail(&symbol.detail);
        return Some((type_name, nullability));
    }
    let signature = collect_signature(declaration.lines()?, symbol.selection_start() as usize);
    let type_name = crate::detail_text::extract_return_type_from_detail(&signature)?;
    let nullability = crate::detail_text::extract_return_nullability_from_detail(&signature);
    Some((type_name, nullability))
}

/// What a call on a known receiver type binds `fn_name` to, as far as the
/// extension registry can tell.
enum ReceiverBoundCallable {
    /// An extension declared on the receiver (or its nearest supertype)
    /// applies; carries its generic metadata, `None` when it declares no
    /// type parameters. Final: a same-named overload for another receiver
    /// must not stand in.
    Extension(Option<CallableInfo>),
    /// A member of the receiver's class takes the call and outranks any
    /// extension: its generics are the class's own, substituted from the
    /// receiver's type arguments, so there is no callable metadata.
    Member,
    /// No in-scope extension applies — an unindexed hierarchy.
    NoApplicableExtension,
}

fn receiver_bound_callable(
    indexer: &Indexer,
    fn_name: &str,
    receiver: ReceiverCall<'_>,
    uri: &Url,
) -> ReceiverBoundCallable {
    let member_takes_the_call = crate::resolver::member_type_parameters_taking_call(
        indexer,
        receiver.receiver_type,
        fn_name,
        uri,
        receiver.shape,
    )
    .is_some();
    if member_takes_the_call {
        return ReceiverBoundCallable::Member;
    }
    let declarations = crate::resolver::extension_declarations_on(
        indexer,
        receiver.receiver_type,
        fn_name,
        uri,
        receiver.shape,
    );
    if declarations.is_empty() {
        return ReceiverBoundCallable::NoApplicableExtension;
    }
    let generic = declaration_selected_by_shape(&declarations, receiver.shape)
        .and_then(|declaration| declaration.symbol())
        .filter(|symbol| !symbol.type_params().is_empty())
        .map(|symbol| CallableInfo {
            type_params: symbol.type_params().to_vec(),
            extension_receiver_type: symbol.declared_extension_receiver().to_owned(),
        });
    ReceiverBoundCallable::Extension(generic)
}

/// The declaring symbol of the extension overload a call of `shape` binds —
/// the same pick `find_method_params_text` makes for the parameter list.
/// Without readable signatures, the first declaration stands in.
fn declaration_selected_by_shape(
    declarations: &[crate::resolver::ExtensionDeclaration],
    shape: CallShape,
) -> Option<&crate::resolver::ExtensionDeclaration> {
    let mut readable = Vec::new();
    let mut signatures = Vec::new();
    for declaration in declarations {
        if declaration.symbol().is_none() {
            continue;
        }
        if let Some(signature) = crate::indexer::infer::sig::declaration_signature(declaration) {
            readable.push(declaration);
            signatures.push(signature);
        }
    }
    match crate::indexer::infer::sig::best_candidate_index(&signatures, shape) {
        Some(index) => Some(readable[index]),
        None => declarations
            .iter()
            .find(|declaration| declaration.symbol().is_some()),
    }
}

// ─── Synthetic enum members ──────────────────────────────────────────────────
//
// Kotlin generates these on every enum class:
//   .entries  → EnumEntries<T>  (effectively List<T>)
//   .values() → Array<T>
//   .valueOf(String) → T
//   .name     → String  (instance)
//   .ordinal  → Int     (instance)

fn is_enum_class(indexer: &Indexer, class_name: &str) -> bool {
    let Some(locs) = indexer.definitions.get(class_name) else {
        return false;
    };
    for loc in locs.iter() {
        let Some(url) = indexer.file_table.url(loc.file) else {
            continue;
        };
        if let Some(fd) = indexer.files.get(url.as_str()) {
            if fd
                .symbols
                .iter()
                .any(|s| s.name == class_name && s.kind == SymbolKind::ENUM)
            {
                return true;
            }
        }
    }
    false
}

fn synthetic_enum_field(indexer: &Indexer, class_name: &str, field_name: &str) -> Option<String> {
    // Check name first to avoid expensive is_enum_class lookup for non-synthetic fields
    match field_name {
        "entries" | "name" | "ordinal" => {}
        _ => return None,
    }
    if !is_enum_class(indexer, class_name) {
        return None;
    }
    match field_name {
        "entries" => Some(format!("List<{class_name}>")),
        "name" => Some("String".to_string()),
        "ordinal" => Some("Int".to_string()),
        _ => None,
    }
}

fn synthetic_enum_method(indexer: &Indexer, class_name: &str, method_name: &str) -> Option<String> {
    match method_name {
        "values" | "valueOf" => {}
        _ => return None,
    }
    if !is_enum_class(indexer, class_name) {
        return None;
    }
    match method_name {
        "values" => Some(format!("Array<{class_name}>")),
        "valueOf" => Some(class_name.to_string()),
        _ => None,
    }
}
