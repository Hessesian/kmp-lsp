//! Per-candidate CST verification for find-references — see
//! `docs/superpowers/specs/2026-07-20-cst-find-references-design.md`.

use std::collections::HashMap;

use tower_lsp::lsp_types::Location;

use crate::indexer::{
    CallShape, Indexer, InferDeps, NavigationSource, OccurrenceSite, ResolvedType, SymbolRole,
};
use crate::resolver::{ReceiverType, ReceiverTypeAgreement, Resolver};

/// Per-request cap on IO-costed verification steps (a candidate's file
/// needing a fresh disk read, or a supertype walk that may spend blocking
/// JAR-sidecar IPC). Once exhausted, remaining candidates stay `NameScan`
/// unverified — never dropped, never rejected on budget grounds alone.
const MAX_VERIFICATION_IO_OPERATIONS: usize = 48;

pub(crate) struct VerifiedReferences {
    pub kept: Vec<NavigationSource<Location>>,
    // Intentionally excluded from `find_references_with_qualifier`'s output —
    // dropping proven-unrelated candidates is the whole point of this pass.
    // Read only by this module's own tests, which assert rejection actually
    // happens rather than candidates silently vanishing from `kept`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub rejected: Vec<Location>,
    /// Declaration-role candidates proven, in either direction, to be in an
    /// override relationship with the query's declaring type. Ignored by
    /// find-references (its call always passes `query_declaring_type_uri:
    /// None` — `verified_references_for`'s `detect_reverse_overrides: false`
    /// forwards `None` here regardless of whether a real declaring-type URI
    /// was computed — so this is always empty there); consumed only by 6c
    /// rename to decide the override-participation refusal. A candidate here is ALSO
    /// present in `kept` as `CstResolved` — the two fields answer different
    /// questions ("is this the same identity" vs. "does an override relate
    /// to it") and are not mutually exclusive.
    pub proven_overrides: Vec<Location>,
}

pub(crate) fn verify_candidates(
    indexer: &Indexer,
    query_declaring_type: Option<&str>,
    query_arity: Option<(u8, u8)>,
    query_declaring_type_uri: Option<&str>,
    sidecar_budget: usize,
    candidates: Vec<Location>,
) -> VerifiedReferences {
    let mut verifier = CandidateVerifier {
        indexer,
        query: query_declaring_type.map(|raw| QueryIdentity {
            declaring_type: ReceiverType::from_raw(raw.to_owned()).leaf,
            arity: query_arity,
            declaring_type_uri: query_declaring_type_uri,
        }),
        sidecar_budget,
        io_budget: MAX_VERIFICATION_IO_OPERATIONS,
        agreements: HashMap::new(),
    };
    let mut verified = VerifiedReferences {
        kept: Vec::new(),
        rejected: Vec::new(),
        proven_overrides: Vec::new(),
    };
    for candidate in candidates {
        verifier.verify(candidate, &mut verified);
    }
    verified
}

/// What the query's own cursor resolved to. Absent when the cursor's
/// declaring/receiver type could not be inferred — candidates then get only
/// the syntactic check.
struct QueryIdentity<'a> {
    declaring_type: String,
    arity: Option<(u8, u8)>,
    declaring_type_uri: Option<&'a str>,
}

/// `(candidate type, the file declaring it, target type)` — everything a
/// [`ReceiverTypeAgreement`] depends on within one request.
type AgreementKey = (String, String, String);

struct CandidateVerifier<'a> {
    indexer: &'a Indexer,
    query: Option<QueryIdentity<'a>>,
    sidecar_budget: usize,
    io_budget: usize,
    /// Agreement answers already paid for in this request. A common member
    /// name (`get`, `map`, `size`) yields thousands of hits on a handful of
    /// receiver types; without this each hit would re-walk the same
    /// hierarchy and the budget would run out on the first few dozen.
    agreements: HashMap<AgreementKey, ReceiverTypeAgreement>,
}

impl CandidateVerifier<'_> {
    fn verify(&mut self, candidate: Location, verified: &mut VerifiedReferences) {
        if self.io_budget == 0 {
            verified.kept.push(NavigationSource::NameScan(candidate));
            return;
        }
        let file_already_indexed = self.indexer.files.contains_key(candidate.uri.as_str())
            || self.indexer.live_lines.contains_key(candidate.uri.as_str());
        if !file_already_indexed {
            self.io_budget -= 1;
        }
        let site = crate::indexer::classify_occurrence_at(
            self.indexer,
            &candidate.uri,
            crate::types::CursorPos {
                line: candidate.range.start.line as usize,
                utf16_col: candidate.range.start.character as usize,
            },
        );
        let symbol = match site {
            OccurrenceSite::Symbol(symbol) => symbol,
            // Comment text, string text and accessor keywords spell the name
            // but refer to nothing — rejected whatever the query is.
            OccurrenceSite::NotCode(kind) => {
                log::trace!(
                    "references: rejected {kind:?} match at {}:{}",
                    candidate.uri,
                    candidate.range.start.line
                );
                verified.rejected.push(candidate);
                return;
            }
            OccurrenceSite::Unclassified => {
                verified.kept.push(NavigationSource::NameScan(candidate));
                return;
            }
        };
        if self.query.is_none() {
            verified.kept.push(NavigationSource::NameScan(candidate));
            return;
        }
        match &symbol.role {
            SymbolRole::Reference {
                receiver_type: Some(receiver_type),
                shape,
                ..
            } => self.verify_member_reference(candidate, receiver_type, *shape, verified),
            SymbolRole::Declaration { .. } => self.verify_declaration(candidate, verified),
            _ => verified.kept.push(NavigationSource::NameScan(candidate)),
        }
    }

    fn verify_member_reference(
        &mut self,
        candidate: Location,
        receiver_type: &ResolvedType,
        shape: Option<CallShape>,
        verified: &mut VerifiedReferences,
    ) {
        let Some(query) = &self.query else {
            verified.kept.push(NavigationSource::NameScan(candidate));
            return;
        };
        // A same-type candidate whose own call shape can't satisfy the
        // query's arity is a name collision, not a genuine reference —
        // `receiver_type_agreement` below only compares types, so a same-file
        // self-declaration/self-call registered on the same receiver type
        // (e.g. `Flow.collect` vs. a local `Flow.collect(scope, block)`
        // self-shadow) would otherwise look identical to the real target.
        // Only rejects when both `shape` (the candidate is itself a call) and
        // the query arity (known and trustworthy — see
        // `verified_references_for`) are present.
        if let (Some(shape), Some((required, total))) = (shape, query.arity) {
            if !shape.accepts(required, total) {
                verified.rejected.push(candidate);
                return;
            }
        }
        let query_type = query.declaring_type.clone();
        let candidate_type = ReceiverType::from_raw(receiver_type.type_path()).leaf;
        // `Exact` (same type) and `Unresolvable` (candidate type not indexed)
        // return before any supertype walk, so only a real walk is charged.
        let will_walk =
            candidate_type != query_type && self.indexer.has_type_definition(&candidate_type);
        // The supertype walk reads the receiver type's OWN declaration for
        // its supertypes, so it must start from the file that declares that
        // type — not from the call site, which declares nothing of the kind.
        // An unresolvable declaration is kept, never rejected.
        let Some(candidate_declaration_uri) = (if will_walk {
            receiver_type_declaration_uri(self.indexer, receiver_type, &candidate.uri)
        } else {
            Some(candidate.uri.to_string())
        }) else {
            verified.kept.push(NavigationSource::NameScan(candidate));
            return;
        };
        let Some(agreement) = self.agreement(
            candidate_type,
            candidate_declaration_uri,
            query_type,
            will_walk,
        ) else {
            verified.kept.push(NavigationSource::NameScan(candidate));
            return;
        };
        match agreement {
            ReceiverTypeAgreement::Exact | ReceiverTypeAgreement::Inherited => {
                verified.kept.push(NavigationSource::CstResolved(candidate));
            }
            ReceiverTypeAgreement::Unrelated => verified.rejected.push(candidate),
            ReceiverTypeAgreement::Unresolvable => {
                verified.kept.push(NavigationSource::NameScan(candidate));
            }
        }
    }

    fn verify_declaration(&mut self, candidate: Location, verified: &mut VerifiedReferences) {
        let Some(query) = &self.query else {
            verified.kept.push(NavigationSource::NameScan(candidate));
            return;
        };
        let query_type = query.declaring_type.clone();
        let query_uri = query.declaring_type_uri.map(str::to_owned);
        let Some(class_name) = self
            .indexer
            .enclosing_class_at(&candidate.uri, candidate.range.start.line)
        else {
            verified.kept.push(NavigationSource::NameScan(candidate));
            return;
        };
        let candidate_type = ReceiverType::from_raw(class_name).leaf;

        let forward_will_walk =
            candidate_type != query_type && self.indexer.has_type_definition(&candidate_type);
        let Some(forward) = self.agreement(
            candidate_type.clone(),
            candidate.uri.to_string(),
            query_type.clone(),
            forward_will_walk,
        ) else {
            verified.kept.push(NavigationSource::NameScan(candidate));
            return;
        };

        // Reverse direction: is the QUERY a subtype of the CANDIDATE's type —
        // i.e. the cursor is on the override, and this candidate is the base
        // it overrides? Only attempted when the caller knows the query's own
        // declaring URI (rename's override-participation check).
        let reverse = query_uri.map(|query_uri| {
            let reverse_will_walk =
                query_type != candidate_type && self.indexer.has_type_definition(&query_type);
            self.agreement(
                query_type.clone(),
                query_uri,
                candidate_type.clone(),
                reverse_will_walk,
            )
            .unwrap_or(ReceiverTypeAgreement::Unresolvable)
        });

        let is_proven_override = matches!(forward, ReceiverTypeAgreement::Inherited)
            || matches!(reverse, Some(ReceiverTypeAgreement::Inherited));
        if is_proven_override {
            verified.proven_overrides.push(candidate.clone());
        }

        match forward {
            ReceiverTypeAgreement::Exact | ReceiverTypeAgreement::Inherited => {
                verified.kept.push(NavigationSource::CstResolved(candidate));
            }
            // A mismatch here is a *weaker* signal than a proven type
            // mismatch — two same-named unrelated declarations aren't the
            // "wrong receiver type" case `ReceiverTypeAgreement` models — so
            // err toward keeping (`NameScan`), never reject here.
            ReceiverTypeAgreement::Unrelated | ReceiverTypeAgreement::Unresolvable => {
                verified.kept.push(NavigationSource::NameScan(candidate));
            }
        }
    }

    /// The agreement of `candidate_type` (declared in `declaration_uri`)
    /// with `target_type`, answered from this request's memo when already
    /// known. `None` when a walk is needed and the IO budget is spent.
    fn agreement(
        &mut self,
        candidate_type: String,
        declaration_uri: String,
        target_type: String,
        will_walk: bool,
    ) -> Option<ReceiverTypeAgreement> {
        let key = (candidate_type, declaration_uri, target_type);
        if let Some(known) = self.agreements.get(&key) {
            return Some(*known);
        }
        if will_walk {
            if self.io_budget == 0 {
                return None;
            }
            self.io_budget -= 1;
        }
        let (candidate_type, declaration_uri, target_type) = &key;
        let agreement = self.indexer.receiver_type_agreement(
            candidate_type,
            declaration_uri,
            target_type,
            self.sidecar_budget,
        );
        self.agreements.insert(key, agreement);
        Some(agreement)
    }
}

/// The file declaring `receiver`'s type, resolved where the type name was
/// written (`call_site` when that is a `jar:` file).
fn receiver_type_declaration_uri(
    indexer: &Indexer,
    receiver: &crate::indexer::ResolvedType,
    call_site: &tower_lsp::lsp_types::Url,
) -> Option<String> {
    crate::resolver::resolve_type_path_declarations(
        indexer,
        &receiver.type_path(),
        receiver.resolution_scope(call_site),
        crate::resolver::ResolveIo::IndexOnly,
    )
    .into_iter()
    .next()
    .map(|location| location.uri.to_string())
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::{Position, Range, Url};

    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file:///t{path}")).unwrap()
    }

    fn location(file_uri: &Url, line: u32, column_start: u32, column_end: u32) -> Location {
        Location {
            uri: file_uri.clone(),
            range: Range::new(
                Position::new(line, column_start),
                Position::new(line, column_end),
            ),
        }
    }

    /// House decoy: a candidate on `File.save()` must be REJECTED (present
    /// in `rejected`, absent from `kept`) when the query's declaring type
    /// is `User`.
    #[test]
    fn unrelated_candidate_is_rejected_not_dropped_silently() {
        let source = "class User { fun save() {} }\n\
                   class File { fun save() {} }\n\
                   fun f(file: File) { file.save() }\n";
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);
        let column = source.lines().nth(2).unwrap().find("save").unwrap() as u32;
        let candidate = location(&file_uri, 2, column, column + 4);

        let result = verify_candidates(
            &indexer,
            Some("User"),
            None,
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            vec![candidate.clone()],
        );
        assert!(
            result.kept.is_empty(),
            "must not be kept, got {:?}",
            result.kept.len()
        );
        assert_eq!(
            result.rejected,
            vec![candidate],
            "must be in rejected, not silently absent"
        );
    }

    /// Slice 3: a call site on a SUBTYPE of the query's declaring type, in a file
    /// that declares neither. The supertype walk must start from the file that
    /// declares the receiver's type (`Derived`, in its own package), not from
    /// the call site — which has no `Derived` declaration to read supertypes from.
    #[test]
    fn subtype_call_site_in_a_file_that_declares_neither_type_is_kept() {
        let base_uri = uri("/types/Base.kt");
        let derived_uri = uri("/impl/Derived.kt");
        let caller_uri = uri("/app/Use.kt");
        let caller_source = "package app
                   import impl.Derived
                   fun f(derived: Derived) { derived.save() }
";
        let indexer = Indexer::new();
        indexer.index_content(
            &base_uri,
            "package types\nopen class Base { fun save() {} }\n",
        );
        indexer.index_content(
            &derived_uri,
            "package impl\nimport types.*\nclass Derived : Base()\n",
        );
        indexer.index_content(&caller_uri, caller_source);
        indexer.store_live_tree(&caller_uri, caller_source);
        let Some(column) = caller_source
            .lines()
            .nth(2)
            .and_then(|line| line.find("save"))
        else {
            panic!("fixture line missing `save`");
        };
        let candidate = location(&caller_uri, 2, column as u32, column as u32 + 4);

        let result = verify_candidates(
            &indexer,
            Some("Base"),
            None,
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            vec![candidate.clone()],
        );

        assert!(
            result.rejected.is_empty(),
            "a Derived receiver inherits Base.save, got rejected: {:?}",
            result.rejected
        );
        assert_eq!(result.kept.len(), 1);
    }

    /// A call site whose receiver type agrees with the query's declaring
    /// type, but whose own declared arity is provably incompatible with
    /// what the query actually targets, must be rejected — not kept just
    /// because `receiver_type_agreement` (type-only) can't tell it apart.
    /// Reported bug: find-references on `Flow`'s real 1-arg `collect`
    /// member also surfaced a same-file 2-arg `Flow.collect(scope, block)`
    /// self-shadow's own call sites as if they were genuine references.
    #[test]
    fn same_type_wrong_arity_candidate_is_rejected() {
        let source = "class CoroutineScope\n\
                   class Flow<T>\n\
                   fun realTarget(x: Flow<String>, arg: (String) -> Unit) {\n\
                       x.collect(arg)\n\
                   }\n\
                   fun wrongShadowCall(x: Flow<String>, s: CoroutineScope, arg: (String) -> Unit) {\n\
                       x.collect(s, arg)\n\
                   }\n";
        let file_uri = uri("/Flow.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        let Some(real_col) = source.lines().nth(3).and_then(|l| l.find("collect")) else {
            panic!("fixture line missing `collect` on the real-target line");
        };
        let Some(wrong_col) = source.lines().nth(6).and_then(|l| l.find("collect")) else {
            panic!("fixture line missing `collect` on the wrong-shadow line");
        };
        let real_candidate = location(&file_uri, 3, real_col as u32, real_col as u32 + 7);
        let wrong_candidate = location(&file_uri, 6, wrong_col as u32, wrong_col as u32 + 7);

        // Querying for Flow's real 1-required-arg `collect` member.
        let result = verify_candidates(
            &indexer,
            Some("Flow"),
            Some((1, 1)),
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            vec![real_candidate.clone(), wrong_candidate.clone()],
        );
        assert!(
            result.kept.iter().any(|k| match k {
                NavigationSource::CstResolved(l) | NavigationSource::NameScan(l) =>
                    *l == real_candidate,
            }),
            "the 1-arg call must be kept, got: {:?}",
            result.kept
        );
        assert_eq!(
            result.rejected,
            vec![wrong_candidate],
            "the 2-arg call to the arity-incompatible same-type shadow must \
             be rejected, not kept as if it were a genuine reference"
        );
    }

    /// House decoy, positive: an inherited-member reference through a
    /// subtype instance must be kept as `CstResolved`.
    #[test]
    fn inherited_candidate_is_kept_as_cst_resolved() {
        let source = "open class User { fun save() {} }\n\
                   class DerivedUser : User()\n\
                   fun f(derived: DerivedUser) { derived.save() }\n";
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);
        let column = source.lines().nth(2).unwrap().find("save").unwrap() as u32;
        let candidate = location(&file_uri, 2, column, column + 4);

        let result = verify_candidates(
            &indexer,
            Some("User"),
            None,
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            vec![candidate.clone()],
        );
        assert!(result.rejected.is_empty());
        assert!(matches!(
            result.kept.as_slice(),
            [NavigationSource::CstResolved(loc)] if *loc == candidate
        ));
    }

    /// The Declaration-arm bug this task fixes: an override's OWN declaration
    /// must classify the same way a reference *through* the subtype does
    /// (`Inherited` -> `CstResolved`), not fall to `NameScan` just because its
    /// enclosing class name isn't a byte-for-byte match against the query type.
    #[test]
    fn override_declaration_is_kept_as_cst_resolved_not_name_scan() {
        let source = "open class User { fun save() {} }\n\
                      class DerivedUser : User() {\n\
                      override fun save() {}\n\
                      }\n";
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);
        let column = source.lines().nth(2).unwrap().find("save").unwrap() as u32;
        let candidate = location(&file_uri, 2, column, column + 4);

        let result = verify_candidates(
            &indexer,
            Some("User"),
            None,
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            vec![candidate.clone()],
        );
        assert!(result.rejected.is_empty());
        assert!(
            matches!(
                result.kept.as_slice(),
                [NavigationSource::CstResolved(location)] if *location == candidate
            ),
            "override's own declaration must be CstResolved, got {:?}",
            result.kept
        );
    }

    #[test]
    fn no_query_identity_passes_every_candidate_through_as_name_scan() {
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        let candidate = location(&file_uri, 0, 0, 4);
        let result = verify_candidates(
            &indexer,
            None,
            None,
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            vec![candidate.clone()],
        );
        assert!(result.rejected.is_empty());
        assert!(matches!(
            result.kept.as_slice(),
            [NavigationSource::NameScan(loc)] if *loc == candidate
        ));
    }

    /// Budget decoy: once the IO budget is exhausted, remaining candidates
    /// stay in `kept` as `NameScan` — never moved to `rejected`, even when
    /// they WOULD have been proven unrelated with more budget.
    ///
    /// To actually exercise budgeting (not merely look like it does), every
    /// candidate here must genuinely cost IO and genuinely resolve to
    /// `ReceiverTypeAgreement::Unrelated` if fully verified:
    /// - One shared, indexed file declares both `User` and `File` (each with
    ///   a `save()` member) so `has_type_definition` resolves globally.
    /// - Each candidate lives in its OWN real file on disk that is never
    ///   indexed or opened, so `classify_symbol_at` must fall back to disk
    ///   (`live_doc_or_parse`'s cold-start path) — the disk-read budget
    ///   charge genuinely fires for every one.
    /// - Each candidate file is a `File`-typed receiver calling `save()`
    ///   (`fun f(file: File) { file.save() }`), which — once agreement is
    ///   checked — is provably `Unrelated` to the `User` query type.
    ///
    /// Every candidate costs 1 disk-read unit; only the FIRST also pays the
    /// agreement walk (`File` vs `User`), which the request memo then answers
    /// for every later `File` receiver. So with
    /// `MAX_VERIFICATION_IO_OPERATIONS = 48`, exactly
    /// `MAX_VERIFICATION_IO_OPERATIONS - 1` candidates are verified and
    /// rejected before the budget hits zero; every candidate after that must
    /// fall back to `NameScan` purely because the budget ran out, not
    /// because classification failed (their receiver type resolves fine).
    #[test]
    fn budget_exhaustion_never_rejects_only_skips_verification() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let decls_src = "class User { fun save() {} }\nclass File { fun save() {} }\n";
        std::fs::write(root.join("Decls.kt"), decls_src).unwrap();
        let decls_uri = Url::from_file_path(root.join("Decls.kt")).unwrap();
        let indexer = Indexer::new();
        indexer.index_content(&decls_uri, decls_src);

        let n = MAX_VERIFICATION_IO_OPERATIONS + 10;
        let candidate_src = "fun f(file: File) { file.save() }\n";
        let column = candidate_src.find("save").unwrap() as u32;
        let candidates: Vec<Location> = (0..n)
            .map(|i| {
                let name = format!("C{i}.kt");
                std::fs::write(root.join(&name), candidate_src).unwrap();
                let candidate_uri = Url::from_file_path(root.join(&name)).unwrap();
                location(&candidate_uri, 0, column, column + 4)
            })
            .collect();

        let result = verify_candidates(
            &indexer,
            Some("User"),
            None,
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            candidates.clone(),
        );

        let max_verifiable = MAX_VERIFICATION_IO_OPERATIONS - 1;
        assert_eq!(
            result.rejected.len(),
            max_verifiable,
            "exactly the candidates the budget could afford must be proven Unrelated and rejected"
        );
        assert!(
            !result.rejected.is_empty(),
            "verification must have genuinely run and excluded some candidates"
        );
        assert!(
            result.rejected.len() < candidates.len(),
            "budget exhaustion must leave some candidates unverified, not reject them all"
        );
        assert_eq!(
            result.kept.len() + result.rejected.len(),
            candidates.len(),
            "no candidate may vanish — every one is either kept or rejected"
        );
        assert!(
            result
                .kept
                .iter()
                .all(|k| matches!(k, NavigationSource::NameScan(_))),
            "budget-exhausted candidates must stay NameScan, never silently become CstResolved"
        );
    }

    /// Budget precision (Reference arm): an `Exact` agreement result
    /// (candidate type == query's declaring type, plain string equality, no
    /// walk) must NOT spend a budget unit. Spend the whole budget on
    /// `MAX_VERIFICATION_IO_OPERATIONS` such candidates, then prove one more
    /// `Exact`-match candidate at a distinct location still resolves
    /// `CstResolved` rather than falling to `NameScan` for lack of budget.
    ///
    /// NOTE: the originally-specified construction for this test used an
    /// `Unresolvable` filler (`fun filler(x: Ghost) { x.save() }`, `Ghost`
    /// undeclared) to drain the budget instead of `Exact`. That construction
    /// does not exercise this fix: `classify_symbol_at`
    /// (`src/indexer/infer/cst_symbol.rs`, the `Resolution::Resolved(t) if
    /// indexer.has_type_definition(...)` guard) already collapses an
    /// undeclared receiver's type to `receiver_type: None` *before*
    /// `verify_candidates` ever sees it, so the filler falls to the `_ =>
    /// NameScan` catch-all arm and never reaches `receiver_type_agreement`
    /// at all -- zero-cost with or without this fix, in both directions.
    /// Confirmed empirically: the original construction passed unmodified
    /// even with the pre-fix (unconditionally-charging) code still in place.
    /// Because `receiver_type_agreement`'s `Unresolvable` branch requires
    /// `candidate_type` to be `Some(_)` (i.e. already `has_type_definition`
    /// -gated true by the classifier) AND simultaneously not
    /// `has_type_definition` (the same predicate, same string, once
    /// `ReceiverType::from_raw(..).leaf` is applied) -- a contradiction --
    /// `Unresolvable` is provably unreachable via the Reference arm's real
    /// call path. `Exact` has no such gate (it is decided as a first,
    /// unconditional check inside `receiver_type_agreement` before
    /// `has_type_definition` is ever consulted) and reliably reaches this
    /// code, so this test uses `Exact` fillers instead. See
    /// `declaration_arm_inherited_walk_now_spends_and_respects_budget` below
    /// for a construction that exercises the Declaration arm's parallel fix
    /// (where `Unresolvable`/`Inherited` genuinely are reachable, since
    /// `enclosing_class_at` has no such gate).
    ///
    /// Every candidate lives in the SAME already-indexed file, so the
    /// disk-read charge never fires for any of them -- only the
    /// agreement-walk charge this fix touches is in play.
    #[test]
    fn exact_reference_agreement_does_not_spend_walk_budget() {
        let source = "class User { fun save() {} }\n\
                      fun filler(u: User) { u.save() }\n\
                      fun caller(user: User) { user.save() }\n";
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        let filler_column = source.lines().nth(1).unwrap().find("save").unwrap() as u32;
        let filler_candidate = location(&file_uri, 1, filler_column, filler_column + 4);

        let real_column = source.lines().nth(2).unwrap().find("save").unwrap() as u32;
        let real_candidate = location(&file_uri, 2, real_column, real_column + 4);

        // MAX_VERIFICATION_IO_OPERATIONS copies of the SAME Exact-match
        // candidate (position is all that matters -- classify_symbol_at is a
        // pure function of (uri, position), duplicates classify identically)
        // plus the one real Exact-match candidate, at a distinct location,
        // at the end.
        let mut candidates: Vec<Location> =
            std::iter::repeat_n(filler_candidate, MAX_VERIFICATION_IO_OPERATIONS).collect();
        candidates.push(real_candidate.clone());

        let result = verify_candidates(
            &indexer,
            Some("User"),
            None,
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            candidates,
        );
        assert!(
            result.kept.iter().any(|kept_source| matches!(
                kept_source,
                NavigationSource::CstResolved(location) if *location == real_candidate
            )),
            "the Exact-match candidate must resolve CstResolved even after \
             MAX_VERIFICATION_IO_OPERATIONS other Exact-match candidates \
             precede it, because Exact never spends a walk-budget unit, \
             got {:?}",
            result.kept
        );
    }

    /// Budget precision (Declaration arm): a genuine supertype walk
    /// (`Inherited`) spends budget, and once the budget is spent a
    /// Declaration-arm candidate needing a walk defers to `NameScan` instead
    /// of ignoring the budget and resolving `CstResolved` regardless.
    ///
    /// Each filler override lives in its OWN subclass, so each needs its own
    /// walk — the request memo only saves repeat walks of the same type
    /// (see `repeated_receiver_type_walks_once_and_verifies_every_candidate`).
    /// Every candidate lives in the SAME already-indexed file, so the
    /// disk-read charge never fires — only the agreement-walk charge is in
    /// play.
    #[test]
    fn declaration_arm_inherited_walk_now_spends_and_respects_budget() {
        let mut source =
            String::from("class User { fun save() {} }\nopen class Base { fun save() {} }\n");
        let mut filler_lines = Vec::new();
        for index in 0..MAX_VERIFICATION_IO_OPERATIONS {
            source.push_str(&format!("class DerivedFiller{index} : Base() {{\n"));
            filler_lines.push(source.lines().count());
            source.push_str("override fun save() {}\n}\n");
        }
        source.push_str("class DerivedReal : Base() {\n");
        let real_line = source.lines().count();
        source.push_str("override fun save() {}\n}\n");

        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, &source);
        indexer.store_live_tree(&file_uri, &source);

        let save_location = |line: usize| {
            let column = source.lines().nth(line).unwrap().find("save").unwrap() as u32;
            location(&file_uri, line as u32, column, column + 4)
        };
        let mut candidates: Vec<Location> = filler_lines
            .iter()
            .map(|line| save_location(*line))
            .collect();
        let real_candidate = save_location(real_line);
        candidates.push(real_candidate.clone());

        let result = verify_candidates(
            &indexer,
            Some("Base"),
            None,
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            candidates,
        );
        let fillers_resolved = result
            .kept
            .iter()
            .filter(|kept_source| matches!(kept_source, NavigationSource::CstResolved(_)))
            .count();
        assert_eq!(
            fillers_resolved, MAX_VERIFICATION_IO_OPERATIONS,
            "every filler override is a genuine walk the budget can afford"
        );
        assert!(
            result.kept.iter().any(|kept_source| matches!(
                kept_source,
                NavigationSource::NameScan(location) if *location == real_candidate
            )),
            "once MAX_VERIFICATION_IO_OPERATIONS genuine Inherited walks have \
             legitimately spent the whole budget, one more Declaration-arm \
             candidate needing a walk must defer to NameScan, got {:?}",
            result.kept
        );
    }

    /// The symmetric half of override detection: renaming FROM the concrete
    /// override's own declaration must ALSO detect the interface declaration
    /// as a proven override participant -- not just the forward direction
    /// (querying from the interface, finding the override).
    #[test]
    fn override_detected_symmetrically_from_the_concrete_side() {
        let source = "open class User {\n\
                      fun save() {}\n\
                      }\n\
                      class DerivedUser : User() {\n\
                      override fun save() {}\n\
                      }\n";
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        // Candidate: the INTERFACE's own declaration (line 1).
        let interface_column = source.lines().nth(1).unwrap().find("save").unwrap() as u32;
        let interface_candidate = location(&file_uri, 1, interface_column, interface_column + 4);

        // Query: "DerivedUser" (the CONCRETE/override side), declared at
        // file_uri itself -- exactly the case Task 4 supplies
        // query_declaring_type_uri for (cursor on a Declaration).
        let result = verify_candidates(
            &indexer,
            Some("DerivedUser"),
            None,
            Some(file_uri.as_str()),
            usize::MAX,
            vec![interface_candidate.clone()],
        );
        assert_eq!(
            result.proven_overrides,
            vec![interface_candidate],
            "querying from the override side must still detect the interface \
             declaration as a proven override participant"
        );
    }

    /// The forward direction (querying from the interface, the override's
    /// declaration is the candidate) must also populate `proven_overrides` --
    /// not just classify CstResolved.
    #[test]
    fn override_detected_from_the_interface_side_populates_proven_overrides() {
        let source = "open class User {\n\
                      fun save() {}\n\
                      }\n\
                      class DerivedUser : User() {\n\
                      override fun save() {}\n\
                      }\n";
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        let override_column = source.lines().nth(4).unwrap().find("save").unwrap() as u32;
        let override_candidate = location(&file_uri, 4, override_column, override_column + 4);

        let result = verify_candidates(
            &indexer,
            Some("User"),
            None,
            None,
            usize::MAX,
            vec![override_candidate.clone()],
        );
        assert_eq!(result.proven_overrides, vec![override_candidate]);
    }

    /// House decoy: two UNRELATED classes with same-named methods must never
    /// populate `proven_overrides` -- only a proven supertype/subtype
    /// relationship does.
    #[test]
    fn unrelated_same_named_declaration_is_not_a_proven_override() {
        let source = "class User {\n\
                      fun save() {}\n\
                      }\n\
                      class File {\n\
                      fun save() {}\n\
                      }\n";
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        let file_column = source.lines().nth(4).unwrap().find("save").unwrap() as u32;
        let file_candidate = location(&file_uri, 4, file_column, file_column + 4);

        let result = verify_candidates(
            &indexer,
            Some("User"),
            None,
            None,
            usize::MAX,
            vec![file_candidate],
        );
        assert!(result.proven_overrides.is_empty());
    }

    /// A common member name yields many hits on the same receiver type. The
    /// agreement walk for that type runs once per request, so every hit is
    /// verified — far past the point where per-hit walks would have spent
    /// the budget.
    #[test]
    fn repeated_receiver_type_walks_once_and_verifies_every_candidate() {
        let candidate_count = MAX_VERIFICATION_IO_OPERATIONS * 3;
        let mut source =
            String::from("class User { fun save() {} }\nclass File { fun save() {} }\n");
        for index in 0..candidate_count {
            source.push_str(&format!("fun f{index}(file: File) {{ file.save() }}\n"));
        }
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, &source);
        indexer.store_live_tree(&file_uri, &source);
        let candidates: Vec<Location> = (0..candidate_count)
            .map(|index| {
                let line = index + 2;
                let column = source.lines().nth(line).unwrap().find("save").unwrap() as u32;
                location(&file_uri, line as u32, column, column + 4)
            })
            .collect();

        let result = verify_candidates(
            &indexer,
            Some("User"),
            None,
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            candidates,
        );
        assert!(
            result.kept.is_empty(),
            "every File.save call is proven unrelated to User, got kept: {:?}",
            result.kept
        );
        assert_eq!(result.rejected.len(), candidate_count);
    }

    fn not_code_fixture() -> (Url, Indexer, String) {
        let source = "class Box {\n\
                      fun get(): Int = 1\n\
                      val size: Int\n\
                      get() = 2\n\
                      var label: String = \"\"\n\
                      set(value) { field = value }\n\
                      }\n\
                      // get the box\n\
                      /** Calls [get] or [Box.get]. */\n\
                      fun use(box: Box): String {\n\
                      box.get()\n\
                      return \"get $box ${box.get()}\"\n\
                      }\n"
        .to_owned();
        let file_uri = uri("/Box.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, &source);
        indexer.store_live_tree(&file_uri, &source);
        (file_uri, indexer, source)
    }

    /// Every `(line, column)` where the whole word `word` occurs — what
    /// `rg --word-regexp` hands the verifier.
    fn word_hits(file_uri: &Url, source: &str, word: &str) -> Vec<Location> {
        let mut hits = Vec::new();
        for (line_index, line) in source.lines().enumerate() {
            let mut search_from = 0;
            while let Some(offset) = line[search_from..].find(word) {
                let start = search_from + offset;
                let end = start + word.len();
                let is_word_char =
                    |character: char| character.is_alphanumeric() || character == '_';
                let before_is_word = line[..start].chars().next_back().is_some_and(is_word_char);
                let after_is_word = line[end..].chars().next().is_some_and(is_word_char);
                if !before_is_word && !after_is_word {
                    hits.push(location(
                        file_uri,
                        line_index as u32,
                        start as u32,
                        end as u32,
                    ));
                }
                search_from = end;
            }
        }
        hits
    }

    fn kept_lines(result: &VerifiedReferences) -> Vec<(u32, u32)> {
        let mut lines: Vec<(u32, u32)> = result
            .kept
            .iter()
            .map(|kept_source| match kept_source {
                NavigationSource::CstResolved(location) | NavigationSource::NameScan(location) => {
                    (location.range.start.line, location.range.start.character)
                }
            })
            .collect();
        lines.sort_unstable();
        lines
    }

    /// Comment text, string text and accessor keywords that spell `get` are
    /// rejected; real code — the declaration, the call, the call inside a
    /// `${…}` template and KDoc `[get]` links — is kept.
    #[test]
    fn non_code_matches_are_rejected_and_code_kept() {
        let (file_uri, indexer, source) = not_code_fixture();
        let hits = word_hits(&file_uri, &source, "get");
        let result = verify_candidates(
            &indexer,
            Some("Box"),
            None,
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            hits,
        );
        let column_of = |line: usize, needle: &str| {
            source.lines().nth(line).unwrap().find(needle).unwrap() as u32
        };
        let doc_line = source.lines().nth(8).unwrap();
        let first_link = doc_line.find("[get]").unwrap() as u32 + 1;
        let second_link = doc_line.find("Box.get").unwrap() as u32 + 4;
        let template_call = column_of(11, "box.get()") + 4;
        assert_eq!(
            kept_lines(&result),
            vec![
                (1, column_of(1, "get")),
                (8, first_link),
                (8, second_link),
                (10, column_of(10, "get")),
                (11, template_call),
            ],
            "kept the wrong set; rejected: {:?}",
            result.rejected
        );
        let mut rejected: Vec<(u32, u32)> = result
            .rejected
            .iter()
            .map(|location| (location.range.start.line, location.range.start.character))
            .collect();
        rejected.sort_unstable();
        assert_eq!(
            rejected,
            vec![
                (3, column_of(3, "get")),
                (7, column_of(7, "get")),
                (11, column_of(11, "get")),
            ],
            "the getter keyword, the line comment and the string text must be rejected"
        );
    }

    /// The syntactic check needs no query identity: even when the cursor's
    /// own type could not be inferred, comment and string matches go.
    #[test]
    fn non_code_matches_are_rejected_without_query_identity() {
        let (file_uri, indexer, source) = not_code_fixture();
        let hits = word_hits(&file_uri, &source, "get");
        let result = verify_candidates(
            &indexer,
            None,
            None,
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            hits,
        );
        assert_eq!(result.rejected.len(), 3, "rejected: {:?}", result.rejected);
        assert!(result
            .kept
            .iter()
            .all(|kept_source| matches!(kept_source, NavigationSource::NameScan(_))));
    }

    /// `set` as a setter keyword is rejected; `field`/`value` inside it are
    /// untouched (they are not `set`).
    #[test]
    fn setter_keyword_is_rejected() {
        let (file_uri, indexer, source) = not_code_fixture();
        let hits = word_hits(&file_uri, &source, "set");
        let result = verify_candidates(
            &indexer,
            Some("Box"),
            None,
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            hits,
        );
        assert!(result.kept.is_empty(), "kept: {:?}", result.kept);
        assert_eq!(result.rejected.len(), 1);
    }
}
