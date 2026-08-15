//! Per-candidate CST verification for find-references — see
//! `docs/superpowers/specs/2026-07-20-cst-find-references-design.md`.

use tower_lsp::lsp_types::Location;

use crate::indexer::{Indexer, InferDeps, NavigationSource};
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
    let Some(query_declaring_type) = query_declaring_type else {
        // No query identity — every candidate is exactly today's behavior.
        return VerifiedReferences {
            kept: candidates
                .into_iter()
                .map(NavigationSource::NameScan)
                .collect(),
            rejected: Vec::new(),
            proven_overrides: Vec::new(),
        };
    };
    let query_declaring_type = ReceiverType::from_raw(query_declaring_type.to_owned()).leaf;

    let mut kept = Vec::new();
    let mut rejected = Vec::new();
    let mut proven_overrides = Vec::new();
    let mut io_budget = MAX_VERIFICATION_IO_OPERATIONS;

    for candidate in candidates {
        if io_budget == 0 {
            kept.push(NavigationSource::NameScan(candidate));
            continue;
        }
        let file_already_indexed = indexer.files.contains_key(candidate.uri.as_str())
            || indexer.live_lines.contains_key(candidate.uri.as_str());
        if !file_already_indexed {
            io_budget -= 1;
        }
        let Some(symbol) = crate::indexer::classify_symbol_at(
            indexer,
            &candidate.uri,
            crate::types::CursorPos {
                line: candidate.range.start.line as usize,
                utf16_col: candidate.range.start.character as usize,
            },
        ) else {
            kept.push(NavigationSource::NameScan(candidate));
            continue;
        };
        match &symbol.role {
            crate::indexer::SymbolRole::Reference {
                receiver_type: Some(receiver_type),
                shape,
                ..
            } => {
                // A same-type candidate whose own call shape can't satisfy
                // the query's arity is a name collision, not a genuine
                // reference — `receiver_type_agreement` below only compares
                // types, so a same-file self-declaration/self-call
                // registered on the same receiver type (e.g. `Flow.collect`
                // vs. a local `Flow.collect(scope, block)` self-shadow)
                // would otherwise look identical to the real target. Only
                // rejects when both `shape` (the candidate is itself a call)
                // and `query_arity` (the query's target arity is known and
                // trustworthy — see `verified_references_for`) are present;
                // anything else keeps today's behavior unfiltered.
                if let (Some(shape), Some((required, total))) = (shape, query_arity) {
                    if !shape.accepts(required, total) {
                        rejected.push(candidate);
                        continue;
                    }
                }
                let candidate_type = ReceiverType::from_raw(receiver_type.clone()).leaf;
                // Only charge the agreement-walk unit when a walk will
                // actually run: `Exact` (same type, string equality) and
                // `Unresolvable` (candidate type not indexed) both return
                // from `receiver_type_agreement` before any supertype walk,
                // so charging for them exhausted the budget faster than the
                // real IO cost warranted.
                let will_walk = candidate_type != query_declaring_type
                    && indexer.has_type_definition(&candidate_type);
                if will_walk {
                    if io_budget == 0 {
                        kept.push(NavigationSource::NameScan(candidate));
                        continue;
                    }
                    io_budget -= 1;
                }
                match indexer.receiver_type_agreement(
                    &candidate_type,
                    candidate.uri.as_str(),
                    &query_declaring_type,
                    sidecar_budget,
                ) {
                    ReceiverTypeAgreement::Exact | ReceiverTypeAgreement::Inherited => {
                        kept.push(NavigationSource::CstResolved(candidate));
                    }
                    ReceiverTypeAgreement::Unrelated => rejected.push(candidate),
                    ReceiverTypeAgreement::Unresolvable => {
                        kept.push(NavigationSource::NameScan(candidate));
                    }
                }
            }
            crate::indexer::SymbolRole::Declaration { .. } => {
                let enclosing_class =
                    indexer.enclosing_class_at(&candidate.uri, candidate.range.start.line);
                match enclosing_class {
                    Some(class_name) => {
                        let candidate_type = ReceiverType::from_raw(class_name).leaf;

                        let forward_will_walk = candidate_type != query_declaring_type
                            && indexer.has_type_definition(&candidate_type);
                        if forward_will_walk {
                            if io_budget == 0 {
                                kept.push(NavigationSource::NameScan(candidate));
                                continue;
                            }
                            io_budget -= 1;
                        }
                        let forward = indexer.receiver_type_agreement(
                            &candidate_type,
                            candidate.uri.as_str(),
                            &query_declaring_type,
                            sidecar_budget,
                        );

                        // Reverse direction: is the QUERY a subtype of the
                        // CANDIDATE's type -- i.e. the cursor is on the
                        // override, and this candidate is the base it
                        // overrides? Only meaningful (and only ever
                        // attempted) when the caller knows the query's own
                        // declaring URI -- see Task 4 in the 6c rename plan
                        // for when that's available.
                        let reverse = query_declaring_type_uri.map(|query_uri| {
                            let reverse_will_walk = query_declaring_type != candidate_type
                                && indexer.has_type_definition(&query_declaring_type);
                            if reverse_will_walk {
                                if io_budget == 0 {
                                    return ReceiverTypeAgreement::Unresolvable;
                                }
                                io_budget -= 1;
                            }
                            indexer.receiver_type_agreement(
                                &query_declaring_type,
                                query_uri,
                                &candidate_type,
                                sidecar_budget,
                            )
                        });

                        let is_proven_override =
                            matches!(forward, ReceiverTypeAgreement::Inherited)
                                || matches!(reverse, Some(ReceiverTypeAgreement::Inherited));
                        if is_proven_override {
                            proven_overrides.push(candidate.clone());
                        }

                        match forward {
                            ReceiverTypeAgreement::Exact | ReceiverTypeAgreement::Inherited => {
                                kept.push(NavigationSource::CstResolved(candidate));
                            }
                            // A mismatch here is a *weaker* signal than a proven type
                            // mismatch — two same-named unrelated declarations aren't
                            // the "wrong receiver type" case `ReceiverTypeAgreement`
                            // models — so err toward keeping (`NameScan`), never
                            // reject here, same as before this fix.
                            ReceiverTypeAgreement::Unrelated
                            | ReceiverTypeAgreement::Unresolvable => {
                                kept.push(NavigationSource::NameScan(candidate));
                            }
                        }
                    }
                    None => kept.push(NavigationSource::NameScan(candidate)),
                }
            }
            _ => kept.push(NavigationSource::NameScan(candidate)),
        }
    }

    VerifiedReferences {
        kept,
        rejected,
        proven_overrides,
    }
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
    /// With `MAX_VERIFICATION_IO_OPERATIONS = 48` and every fully-verified
    /// candidate costing exactly 2 units (1 disk-read charge + 1 agreement
    /// charge — see `verify_candidates`), exactly the first
    /// `MAX_VERIFICATION_IO_OPERATIONS / 2` candidates can be verified and
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

        let max_verifiable = MAX_VERIFICATION_IO_OPERATIONS / 2;
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

    /// Budget precision (Declaration arm): before this fix, the Declaration
    /// arm never consulted `io_budget` at all (Task 2 wired
    /// `receiver_type_agreement` into that arm but with no budget gating),
    /// so a genuine supertype walk (`Inherited`) ran unconditionally
    /// regardless of remaining budget -- unlike the Reference arm's
    /// pre-existing (if imprecise) charge. This spends the whole budget on
    /// `MAX_VERIFICATION_IO_OPERATIONS` Declaration-arm override candidates
    /// that DO require a genuine walk (`Inherited`, so `will_walk` is
    /// correctly true and a charge belongs here), then proves one more such
    /// candidate, at a distinct location, correctly defers to `NameScan`
    /// once the budget is legitimately exhausted -- instead of ignoring the
    /// budget entirely and resolving `CstResolved` regardless, as the
    /// pre-fix Declaration arm did.
    ///
    /// Every candidate lives in the SAME already-indexed file, so the
    /// disk-read charge never fires for any of them -- only the
    /// agreement-walk charge this fix adds to the Declaration arm is in
    /// play.
    #[test]
    fn declaration_arm_inherited_walk_now_spends_and_respects_budget() {
        let source = "class User { fun save() {} }\n\
                      open class Base { fun save() {} }\n\
                      class DerivedFiller : Base() {\n\
                      override fun save() {}\n\
                      }\n\
                      class DerivedReal : Base() {\n\
                      override fun save() {}\n\
                      }\n";
        let file_uri = uri("/D.kt");
        let indexer = Indexer::new();
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        let filler_column = source.lines().nth(3).unwrap().find("save").unwrap() as u32;
        let filler_candidate = location(&file_uri, 3, filler_column, filler_column + 4);

        let real_column = source.lines().nth(6).unwrap().find("save").unwrap() as u32;
        let real_candidate = location(&file_uri, 6, real_column, real_column + 4);

        // MAX_VERIFICATION_IO_OPERATIONS copies of the SAME Inherited-walk
        // override-declaration candidate, plus the one real Inherited-walk
        // candidate, at a distinct location, at the end.
        let mut candidates: Vec<Location> =
            std::iter::repeat_n(filler_candidate, MAX_VERIFICATION_IO_OPERATIONS).collect();
        candidates.push(real_candidate.clone());

        let result = verify_candidates(
            &indexer,
            Some("Base"),
            None,
            None,
            MAX_VERIFICATION_IO_OPERATIONS,
            candidates,
        );
        assert!(
            result.kept.iter().any(|kept_source| matches!(
                kept_source,
                NavigationSource::NameScan(location) if *location == real_candidate
            )),
            "once MAX_VERIFICATION_IO_OPERATIONS genuine Inherited walks have \
             legitimately spent the whole budget, one more Declaration-arm \
             candidate needing a walk must defer to NameScan rather than \
             ignore the exhausted budget and resolve CstResolved regardless, \
             got {:?}",
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
}
