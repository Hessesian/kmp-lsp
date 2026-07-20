//! Per-candidate CST verification for find-references — see
//! `docs/superpowers/specs/2026-07-20-cst-find-references-design.md`.

use tower_lsp::lsp_types::Location;

use crate::indexer::{Indexer, NavigationSource};
use crate::resolver::{receiver_type_agreement, ReceiverType, ReceiverTypeAgreement};

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
}

pub(crate) fn verify_candidates(
    indexer: &Indexer,
    query_declaring_type: Option<&str>,
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
        };
    };
    let query_declaring_type = ReceiverType::from_raw(query_declaring_type.to_owned()).leaf;

    let mut kept = Vec::new();
    let mut rejected = Vec::new();
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
                ..
            } => {
                let candidate_type = ReceiverType::from_raw(receiver_type.clone()).leaf;
                // The supertype walk (Inherited case) may spend sidecar IPC —
                // charge it against the same budget before running it.
                if io_budget == 0 {
                    kept.push(NavigationSource::NameScan(candidate));
                    continue;
                }
                io_budget -= 1;
                match receiver_type_agreement(
                    indexer,
                    &candidate_type,
                    candidate.uri.as_str(),
                    &query_declaring_type,
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
                // Verified by exact (name, enclosing class) match. A mismatch
                // is a *weaker* signal than a proven type mismatch — two
                // same-named unrelated declarations aren't the "wrong
                // receiver type" case `ReceiverTypeAgreement` models — so
                // err toward keeping (`NameScan`), never reject here.
                let enclosing_class =
                    indexer.enclosing_class_at(&candidate.uri, candidate.range.start.line);
                let matches_query = enclosing_class
                    .as_deref()
                    .map(|class_name| ReceiverType::from_raw(class_name.to_owned()).leaf)
                    == Some(query_declaring_type.clone());
                if matches_query {
                    kept.push(NavigationSource::CstResolved(candidate));
                } else {
                    kept.push(NavigationSource::NameScan(candidate));
                }
            }
            _ => kept.push(NavigationSource::NameScan(candidate)),
        }
    }

    VerifiedReferences { kept, rejected }
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::{Position, Range, Url};

    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file:///t{path}")).unwrap()
    }

    fn location(uri: &Url, line: u32, col_start: u32, col_end: u32) -> Location {
        Location {
            uri: uri.clone(),
            range: Range::new(Position::new(line, col_start), Position::new(line, col_end)),
        }
    }

    /// House decoy: a candidate on `File.save()` must be REJECTED (present
    /// in `rejected`, absent from `kept`) when the query's declaring type
    /// is `User`.
    #[test]
    fn unrelated_candidate_is_rejected_not_dropped_silently() {
        let src = "class User { fun save() {} }\n\
                   class File { fun save() {} }\n\
                   fun f(file: File) { file.save() }\n";
        let u = uri("/D.kt");
        let idx = Indexer::new();
        idx.index_content(&u, src);
        idx.store_live_tree(&u, src);
        let col = src.lines().nth(2).unwrap().find("save").unwrap() as u32;
        let candidate = location(&u, 2, col, col + 4);

        let result = verify_candidates(&idx, Some("User"), vec![candidate.clone()]);
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

    /// House decoy, positive: an inherited-member reference through a
    /// subtype instance must be kept as `CstResolved`.
    #[test]
    fn inherited_candidate_is_kept_as_cst_resolved() {
        let src = "open class User { fun save() {} }\n\
                   class DerivedUser : User()\n\
                   fun f(derived: DerivedUser) { derived.save() }\n";
        let u = uri("/D.kt");
        let idx = Indexer::new();
        idx.index_content(&u, src);
        idx.store_live_tree(&u, src);
        let col = src.lines().nth(2).unwrap().find("save").unwrap() as u32;
        let candidate = location(&u, 2, col, col + 4);

        let result = verify_candidates(&idx, Some("User"), vec![candidate.clone()]);
        assert!(result.rejected.is_empty());
        assert!(matches!(
            result.kept.as_slice(),
            [NavigationSource::CstResolved(loc)] if *loc == candidate
        ));
    }

    #[test]
    fn no_query_identity_passes_every_candidate_through_as_name_scan() {
        let u = uri("/D.kt");
        let idx = Indexer::new();
        let candidate = location(&u, 0, 0, 4);
        let result = verify_candidates(&idx, None, vec![candidate.clone()]);
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
        let idx = Indexer::new();
        idx.index_content(&decls_uri, decls_src);

        let n = MAX_VERIFICATION_IO_OPERATIONS + 10;
        let candidate_src = "fun f(file: File) { file.save() }\n";
        let col = candidate_src.find("save").unwrap() as u32;
        let candidates: Vec<Location> = (0..n)
            .map(|i| {
                let name = format!("C{i}.kt");
                std::fs::write(root.join(&name), candidate_src).unwrap();
                let candidate_uri = Url::from_file_path(root.join(&name)).unwrap();
                location(&candidate_uri, 0, col, col + 4)
            })
            .collect();

        let result = verify_candidates(&idx, Some("User"), candidates.clone());

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
}
