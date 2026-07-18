use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

const COVERAGE_MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/kotlin_spec/coverage.toml"
));
const COVERAGE_FRAGMENTS: [&str; 20] = [
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/introduction.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/syntax.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/type-system.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/builtins.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/declarations.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/inheritance.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/scoping.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/statements.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/expressions.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/operators.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/packages.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/overload-resolution.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/cdfa.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/type-constraints.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/type-inference.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/rtti.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/exceptions.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/annotations.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/coroutines.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.core/concurrency.toml"
    )),
];
const EVOLUTION_FRAGMENTS: [&str; 6] = [
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.evolution/1.9.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.evolution/2.0.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.evolution/2.1.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.evolution/2.2.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.evolution/2.3.toml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/kotlin_spec/coverage/kotlin.evolution/2.4.toml"
    )),
];
const PREVIEW_EVOLUTION_FRAGMENT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/kotlin_spec/coverage/kotlin.evolution/2.4.20-beta1.toml"
));
const DOCUMENTATION_FRAGMENT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/kotlin_spec/coverage/kotlin.documentation.toml"
));
const SPECIFICATION_REPOSITORY: &str = "Kotlin/kotlin-spec";
const SPECIFICATION_REVISION: &str = "2f7aa0524ec27e788dfacd550f144809f2e0254c";
const NORMATIVE_ROOT: &str = "docs/src/md";
const LANGUAGE_TARGET_VERSION: &str = "2.4";
const LANGUAGE_TARGET_RELEASE: &str = "v2.4.10";
const LANGUAGE_TARGET_REVISION: &str = "5687445832cd835b4509b9fbc264cdf1a8201093";
const LANGUAGE_BASELINE_RELEASE: &str = "v1.9.0";
const LANGUAGE_BASELINE_REVISION: &str = "bcf27812cd28041e0b9ffa3bfe52fc58c397d0eb";
const PREVIEW_TARGET_RELEASE: &str = "v2.4.20-Beta1";
const PREVIEW_TARGET_REVISION: &str = "adf58296cf6637999310c497834b3d97516abf5f";
const PREVIEW_RELEASE_LINE: &str = "2.4.20-Beta1";
const DOCUMENTATION_REPOSITORY: &str = "JetBrains/kotlin-web-site";
const DOCUMENTATION_REVISION: &str = "7c270c2ac320fbee4884927f056b89d32f2a002e";
const DOCUMENTATION_SOURCE_ROOT: &str = "docs/topics";
const DOCUMENTATION_TOC_PATH: &str = "docs/kr.tree";
const DOCUMENTATION_TOC_TITLE: &str = "Language guide";
const DOCUMENTATION_TOPIC_COUNT: usize = 49;
const EVOLUTION_RELEASE_LINES: [&str; 6] = ["1.9", "2.0", "2.1", "2.2", "2.3", "2.4"];
const EVOLUTION_SOURCE_IDENTITIES: [(&str, &str, &str, Option<&str>); 6] = [
    (
        LANGUAGE_TARGET_REVISION,
        "docs/changelogs/ChangeLog-1.9.X.md",
        "## 1.9.24",
        Some("## 1.9.0"),
    ),
    (
        LANGUAGE_TARGET_REVISION,
        "docs/changelogs/ChangeLog-2.0.X.md",
        "## 2.0.21",
        None,
    ),
    (
        LANGUAGE_TARGET_REVISION,
        "docs/changelogs/ChangeLog-2.1.X.md",
        "## 2.1.21",
        None,
    ),
    (
        LANGUAGE_TARGET_REVISION,
        "docs/changelogs/ChangeLog-2.2.X.md",
        "## 2.2.21",
        None,
    ),
    (
        "eb08be1d1e0114988f5c7388b5c14855cdf819e0",
        "docs/changelogs/ChangeLog-2.3.X.md",
        "## 2.3.21",
        None,
    ),
    (
        LANGUAGE_TARGET_REVISION,
        "ChangeLog.md",
        "## 2.4.10-RC2",
        None,
    ),
];
const NORMATIVE_SOURCES: [&str; 20] = [
    "kotlin.core/introduction.md",
    "kotlin.core/syntax.md",
    "kotlin.core/type-system.md",
    "kotlin.core/builtins.md",
    "kotlin.core/declarations.md",
    "kotlin.core/inheritance.md",
    "kotlin.core/scoping.md",
    "kotlin.core/statements.md",
    "kotlin.core/expressions.md",
    "kotlin.core/operators.md",
    "kotlin.core/packages.md",
    "kotlin.core/overload-resolution.md",
    "kotlin.core/cdfa.md",
    "kotlin.core/type-constraints.md",
    "kotlin.core/type-inference.md",
    "kotlin.core/rtti.md",
    "kotlin.core/exceptions.md",
    "kotlin.core/annotations.md",
    "kotlin.core/coroutines.md",
    "kotlin.core/concurrency.md",
];

struct CoverageMatrix {
    specification: Specification,
    sources: Vec<SourceLedger>,
    requirements: Vec<Requirement>,
    language_target: LanguageTarget,
    preview_target: PreviewTarget,
    evolution_sources: Vec<EvolutionSourceLedger>,
    evolution_audit_items: Vec<EvolutionAuditItem>,
    evolution_requirements: Vec<EvolutionRequirement>,
    preview_sources: Vec<EvolutionSourceLedger>,
    preview_audit_items: Vec<EvolutionAuditItem>,
    documentation: DocumentationIdentity,
    documentation_topics: Vec<DocumentationTopicLedger>,
    documentation_audit_items: Vec<DocumentationAuditItem>,
    retired_requirements: Vec<RetiredRequirement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoverageManifest {
    specification: Specification,
    sources: Vec<SourceLedger>,
    language_target: LanguageTarget,
    preview_target: PreviewTarget,
    evolution_sources: Vec<EvolutionSourceLedger>,
    preview_sources: Vec<EvolutionSourceLedger>,
    documentation: DocumentationIdentity,
    documentation_topics: Vec<DocumentationTopicLedger>,
    #[serde(default)]
    retired_requirements: Vec<RetiredRequirement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequirementFragment {
    #[serde(default)]
    requirements: Vec<Requirement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvolutionFragment {
    #[serde(default)]
    audit_items: Vec<EvolutionAuditItem>,
    #[serde(default)]
    requirements: Vec<EvolutionRequirement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentationFragment {
    #[serde(default)]
    audit_items: Vec<DocumentationAuditItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Specification {
    version: String,
    repository: String,
    revision: String,
    normative_root: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LanguageTarget {
    language_version: String,
    compiler_release: String,
    target_revision: String,
    baseline_release: String,
    baseline_revision: String,
    audit_status: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewTarget {
    language_version: String,
    compiler_release: String,
    target_revision: String,
    relationship: String,
    audit_status: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentationIdentity {
    repository: String,
    revision: String,
    source_root: String,
    toc_path: String,
    toc_title: String,
    topic_count: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentationTopicLedger {
    toc_order: usize,
    source_path: String,
    audit_status: String,
    claim_count: Option<usize>,
    linked_requirement_count: Option<usize>,
    excluded_claim_count: Option<usize>,
    rationale: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceLedger {
    path: String,
    audit_status: String,
    exact_active: Option<usize>,
    exact_ignored: Option<usize>,
    heuristic_active: Option<usize>,
    heuristic_ignored: Option<usize>,
    out_of_scope_excluded: Option<usize>,
    rationale: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvolutionSourceLedger {
    release_line: String,
    audit_status: String,
    source_revision: String,
    source_path: String,
    source_start_heading: String,
    source_end_heading: Option<String>,
    audit_item_count: Option<usize>,
    exact_active: Option<usize>,
    exact_ignored: Option<usize>,
    heuristic_active: Option<usize>,
    heuristic_ignored: Option<usize>,
    out_of_scope_excluded: Option<usize>,
    rationale: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceCitation {
    source_file: String,
    source_heading: Option<String>,
    source_anchor: Option<String>,
    source_line_start: usize,
    source_line_end: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentationCitation {
    repository: String,
    revision: String,
    source_path: String,
    source_heading: Option<String>,
    source_anchor: Option<String>,
    source_line_start: usize,
    source_line_end: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Requirement {
    id: String,
    #[serde(default)]
    previous_ids: Vec<String>,
    source_file: String,
    source_heading: Option<String>,
    source_anchor: Option<String>,
    source_line_start: usize,
    source_line_end: usize,
    #[serde(default)]
    related_sources: Vec<SourceCitation>,
    statement: String,
    classification: String,
    capabilities: Vec<String>,
    status: String,
    #[serde(default)]
    tests: Vec<String>,
    #[serde(default)]
    duplicates: Vec<String>,
    fixture: Option<String>,
    sample_evidence: Option<String>,
    oracle: String,
    fallback_oracle: Option<String>,
    ignore_reason: Option<String>,
    observed_failure: Option<String>,
    expected_behavior: Option<String>,
    heuristic_limitations: Option<String>,
    exclusion_kind: Option<String>,
    exclusion_rationale: Option<String>,
    verified_through: Option<String>,
    #[serde(default)]
    evolution_audit_ids: Vec<String>,
    #[serde(default)]
    documentation_citations: Vec<DocumentationCitation>,
    #[serde(default)]
    migration_citations: Vec<DocumentationCitation>,
    migration_note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvolutionAuditItem {
    id: String,
    release_line: String,
    source_heading: String,
    source_category: String,
    source_line_start: usize,
    source_line_end: usize,
    issue: Option<String>,
    statement: String,
    disposition: String,
    #[serde(default)]
    requirement_ids: Vec<String>,
    duplicate_of: Option<String>,
    rationale: Option<String>,
    exclusion_kind: Option<String>,
    target_membership_evidence: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvolutionRequirement {
    id: String,
    introduced_in: String,
    maturity: String,
    required_compiler_flag: Option<String>,
    required_opt_in: Option<String>,
    stabilized_in: Option<String>,
    #[serde(default)]
    maturity_changes: Vec<String>,
    change_kind: String,
    statement: String,
    capabilities: Vec<String>,
    classification: String,
    status: String,
    #[serde(default)]
    audit_item_ids: Vec<String>,
    #[serde(default)]
    tests: Vec<String>,
    fixture: Option<String>,
    sample_evidence: Option<String>,
    oracle: String,
    fallback_oracle: Option<String>,
    ignore_reason: Option<String>,
    observed_failure: Option<String>,
    expected_behavior: Option<String>,
    heuristic_limitations: Option<String>,
    exclusion_kind: Option<String>,
    exclusion_rationale: Option<String>,
    #[serde(default)]
    documentation_citations: Vec<DocumentationCitation>,
    #[serde(default)]
    migration_citations: Vec<DocumentationCitation>,
    #[serde(default)]
    compiler_citations: Vec<KotlinCitation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KotlinCitation {
    revision: String,
    source_path: String,
    source_heading: Option<String>,
    source_anchor: Option<String>,
    source_line_start: usize,
    source_line_end: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentationAuditItem {
    id: String,
    source_path: String,
    source_heading: Option<String>,
    source_anchor: Option<String>,
    source_line_start: usize,
    source_line_end: usize,
    statement: String,
    disposition: String,
    #[serde(default)]
    requirement_ids: Vec<String>,
    #[serde(default)]
    evolution_audit_ids: Vec<String>,
    duplicate_of: Option<String>,
    exclusion_kind: Option<String>,
    exclusion_rationale: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetiredRequirement {
    id: String,
    retired_in: String,
    statement: String,
    rationale: String,
    #[serde(default)]
    replacement_ids: Vec<String>,
    source_revision: String,
    source_path: String,
    source_line_start: usize,
    source_line_end: usize,
}

#[test]
fn coverage_matrix_has_valid_traceability_entries() {
    let matrix = parse_coverage_matrix();
    assert_specification_identity(&matrix.specification);
    assert_language_target_identity(&matrix.language_target);
    assert_preview_target_identity(&matrix.preview_target);
    assert_documentation_identity(&matrix.documentation);
    assert!(!matrix.requirements.is_empty());

    let source_ledgers = assert_source_ledgers(&matrix.sources, &matrix.requirements);
    let test_source = specification_test_source();
    let mut requirement_ids = HashSet::new();
    let mut previous_ids = HashSet::new();
    let mut primary_tests = HashSet::new();

    for requirement in &matrix.requirements {
        assert!(
            requirement_ids.insert(requirement.id.as_str()),
            "duplicate specification requirement ID: {}",
            requirement.id
        );
        for previous_id in &requirement.previous_ids {
            assert!(
                previous_ids.insert(previous_id.as_str()),
                "legacy requirement ID {previous_id} is mapped more than once"
            );
        }

        assert_requirement(requirement, &source_ledgers);
        assert_primary_tests(requirement, &test_source, &mut primary_tests);
    }

    let evolution_audit_item_ids = assert_evolution_matrix(
        &matrix,
        &test_source,
        &mut requirement_ids,
        &mut primary_tests,
    );
    assert_preview_matrix(&matrix, &evolution_audit_item_ids);
    assert_documentation_matrix(&matrix, &requirement_ids, &evolution_audit_item_ids);
    assert_documentation_citations(&matrix);
    assert_retired_requirements(&matrix, &requirement_ids);
    assert_legacy_requirements_are_verified_when_complete(&matrix);
    assert_complete_language_target_has_no_unfinished_ledgers(&matrix);

    for requirement_id in requirement_ids {
        assert!(
            !previous_ids.contains(requirement_id),
            "requirement ID {requirement_id} is both current and previous"
        );
    }

    assert_all_primary_tests_are_traced(&test_source, &primary_tests);
    for coverage_document in std::iter::once(COVERAGE_MANIFEST)
        .chain(COVERAGE_FRAGMENTS)
        .chain(EVOLUTION_FRAGMENTS)
        .chain(std::iter::once(PREVIEW_EVOLUTION_FRAGMENT))
        .chain(std::iter::once(DOCUMENTATION_FRAGMENT))
    {
        assert!(
            !coverage_document.to_ascii_lowercase().contains("uncertain"),
            "coverage manifest and fragments must not contain uncertain entries"
        );
    }
}

#[test]
#[ignore = "requires the read-only kotlin-spec authoring checkout"]
fn coverage_matrix_matches_pinned_source_checkout() {
    let matrix = parse_coverage_matrix();
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("kotlin-spec");
    assert_checkout_revision(&checkout);

    let normative_root = checkout.join(&matrix.specification.normative_root);
    for source_ledger in &matrix.sources {
        assert_source_file_exists(&normative_root, &source_ledger.path);
    }
    for requirement in &matrix.requirements {
        let citation = requirement_primary_citation(requirement);
        assert_citation_matches_checkout(&normative_root, &citation, &requirement.id);
        for citation in &requirement.related_sources {
            assert_citation_matches_checkout(&normative_root, citation, &requirement.id);
        }
    }
    assert_included_syntax_grammar_matches_authoring_source(&checkout, &matrix.requirements);
}

#[test]
#[ignore = "requires the read-only Kotlin authoring checkout"]
fn evolution_matrix_matches_pinned_kotlin_checkout() {
    let matrix = parse_coverage_matrix();
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("kotlin");
    assert_kotlin_target_revision(&checkout, &matrix.language_target);

    for source_ledger in &matrix.evolution_sources {
        assert_evolution_source_matches_checkout(
            &checkout,
            source_ledger,
            &matrix.evolution_audit_items,
        );
    }

    for source_ledger in &matrix.preview_sources {
        assert_evolution_source_matches_checkout(
            &checkout,
            source_ledger,
            &matrix.preview_audit_items,
        );
    }

    for requirement in &matrix.evolution_requirements {
        for citation in &requirement.compiler_citations {
            assert_kotlin_citation_matches_checkout(&checkout, citation, &requirement.id);
        }
    }
    for requirement in &matrix.retired_requirements {
        let citation = KotlinCitation {
            revision: requirement.source_revision.clone(),
            source_path: requirement.source_path.clone(),
            source_heading: None,
            source_anchor: None,
            source_line_start: requirement.source_line_start,
            source_line_end: requirement.source_line_end,
        };
        assert_kotlin_citation_matches_checkout(&checkout, &citation, &requirement.id);
    }
}

#[test]
#[ignore = "requires the read-only kotlin-web-site authoring checkout"]
fn documentation_matrix_matches_pinned_kotlin_web_site_checkout() {
    let matrix = parse_coverage_matrix();
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("kotlin-web-site");
    assert_kotlin_web_site_revision(&checkout, &matrix.documentation);
    assert_documentation_topics_match_pinned_toc(&checkout, &matrix);
    assert_documentation_citations_match_checkout(&checkout, &matrix);
}

fn parse_coverage_matrix() -> CoverageMatrix {
    let manifest: CoverageManifest =
        toml::from_str(COVERAGE_MANIFEST).expect("coverage.toml must be a valid TOML manifest");
    assert_eq!(
        manifest.sources.len(),
        COVERAGE_FRAGMENTS.len(),
        "coverage manifest and fragment counts differ"
    );

    let mut requirements = Vec::new();
    for ((source_ledger, expected_path), fragment_document) in manifest
        .sources
        .iter()
        .zip(NORMATIVE_SOURCES)
        .zip(COVERAGE_FRAGMENTS)
    {
        assert_eq!(
            source_ledger.path, expected_path,
            "source ledger order changed"
        );
        let fragment: RequirementFragment =
            toml::from_str(fragment_document).unwrap_or_else(|error| {
                panic!("coverage fragment for {expected_path} is invalid: {error}")
            });
        assert_fragment_matches_source(source_ledger, &fragment.requirements);
        requirements.extend(fragment.requirements);
    }

    let (evolution_audit_items, evolution_requirements) = parse_evolution_fragments(&manifest);
    let preview_fragment: EvolutionFragment = toml::from_str(PREVIEW_EVOLUTION_FRAGMENT)
        .expect("Kotlin preview coverage fragment must be valid TOML");
    assert_eq!(
        manifest.preview_sources.len(),
        1,
        "exactly one preview source ledger is required"
    );
    assert_evolution_fragment_matches_source(&manifest.preview_sources[0], &preview_fragment);
    if manifest.preview_sources[0].audit_status == "complete" {
        assert_no_unfinished_markers(PREVIEW_EVOLUTION_FRAGMENT, PREVIEW_RELEASE_LINE);
    }
    let documentation_fragment: DocumentationFragment = toml::from_str(DOCUMENTATION_FRAGMENT)
        .expect("Kotlin documentation coverage fragment must be valid TOML");
    assert_no_unfinished_markers(DOCUMENTATION_FRAGMENT, "Language guide audit items");

    CoverageMatrix {
        specification: manifest.specification,
        sources: manifest.sources,
        requirements,
        language_target: manifest.language_target,
        preview_target: manifest.preview_target,
        evolution_sources: manifest.evolution_sources,
        evolution_audit_items,
        evolution_requirements,
        preview_sources: manifest.preview_sources,
        preview_audit_items: preview_fragment.audit_items,
        documentation: manifest.documentation,
        documentation_topics: manifest.documentation_topics,
        documentation_audit_items: documentation_fragment.audit_items,
        retired_requirements: manifest.retired_requirements,
    }
}

fn parse_evolution_fragments(
    manifest: &CoverageManifest,
) -> (Vec<EvolutionAuditItem>, Vec<EvolutionRequirement>) {
    assert_eq!(
        manifest.evolution_sources.len(),
        EVOLUTION_FRAGMENTS.len(),
        "evolution source ledger and fragment counts differ"
    );

    let mut audit_items = Vec::new();
    let mut requirements = Vec::new();
    for ((source_ledger, expected_release_line), fragment_document) in manifest
        .evolution_sources
        .iter()
        .zip(EVOLUTION_RELEASE_LINES)
        .zip(EVOLUTION_FRAGMENTS)
    {
        assert_eq!(
            source_ledger.release_line, expected_release_line,
            "evolution source ledger order changed"
        );
        let fragment: EvolutionFragment =
            toml::from_str(fragment_document).unwrap_or_else(|error| {
                panic!(
                    "evolution fragment for {} is invalid: {error}",
                    source_ledger.release_line
                )
            });
        assert_evolution_fragment_matches_source(source_ledger, &fragment);
        if source_ledger.audit_status == "complete" {
            assert_no_unfinished_markers(fragment_document, &source_ledger.release_line);
        }
        audit_items.extend(fragment.audit_items);
        requirements.extend(fragment.requirements);
    }

    (audit_items, requirements)
}

fn assert_evolution_fragment_matches_source(
    source_ledger: &EvolutionSourceLedger,
    fragment: &EvolutionFragment,
) {
    for audit_item in &fragment.audit_items {
        assert_eq!(
            audit_item.release_line, source_ledger.release_line,
            "{} is stored outside its release-line fragment",
            audit_item.id
        );
    }
    for requirement in &fragment.requirements {
        assert_eq!(
            requirement.introduced_in, source_ledger.release_line,
            "{} is stored outside its release-line fragment",
            requirement.id
        );
    }
}

fn assert_fragment_matches_source(source_ledger: &SourceLedger, requirements: &[Requirement]) {
    for requirement in requirements {
        assert_eq!(
            requirement.source_file, source_ledger.path,
            "{} is stored outside its source fragment",
            requirement.id
        );
    }
}

fn assert_specification_identity(specification: &Specification) {
    assert_eq!(specification.version, "1.9-rfc+0.1");
    assert_eq!(specification.repository, SPECIFICATION_REPOSITORY);
    assert_eq!(specification.revision, SPECIFICATION_REVISION);
    assert_eq!(specification.normative_root, NORMATIVE_ROOT);
}

fn assert_language_target_identity(language_target: &LanguageTarget) {
    assert_eq!(language_target.language_version, LANGUAGE_TARGET_VERSION);
    assert_eq!(language_target.compiler_release, LANGUAGE_TARGET_RELEASE);
    assert_eq!(language_target.target_revision, LANGUAGE_TARGET_REVISION);
    assert_eq!(language_target.baseline_release, LANGUAGE_BASELINE_RELEASE);
    assert_eq!(
        language_target.baseline_revision,
        LANGUAGE_BASELINE_REVISION
    );
    assert!(
        matches!(
            language_target.audit_status.as_str(),
            "in-progress" | "complete"
        ),
        "language target has invalid audit status {}",
        language_target.audit_status
    );
}

fn assert_preview_target_identity(preview_target: &PreviewTarget) {
    assert_eq!(preview_target.language_version, LANGUAGE_TARGET_VERSION);
    assert_eq!(preview_target.compiler_release, PREVIEW_TARGET_RELEASE);
    assert_eq!(preview_target.target_revision, PREVIEW_TARGET_REVISION);
    assert_eq!(preview_target.relationship, "preview-next-compiler-release");
    assert!(matches!(
        preview_target.audit_status.as_str(),
        "pending" | "complete"
    ));
}

fn assert_documentation_identity(documentation: &DocumentationIdentity) {
    assert_eq!(documentation.repository, DOCUMENTATION_REPOSITORY);
    assert_eq!(documentation.revision, DOCUMENTATION_REVISION);
    assert_eq!(documentation.source_root, DOCUMENTATION_SOURCE_ROOT);
    assert_eq!(documentation.toc_path, DOCUMENTATION_TOC_PATH);
    assert_eq!(documentation.toc_title, DOCUMENTATION_TOC_TITLE);
    assert_eq!(documentation.topic_count, DOCUMENTATION_TOPIC_COUNT);
}

fn assert_evolution_matrix<'matrix>(
    matrix: &'matrix CoverageMatrix,
    test_source: &str,
    requirement_ids: &mut HashSet<&'matrix str>,
    primary_tests: &mut HashSet<String>,
) -> HashSet<&'matrix str> {
    assert_evolution_source_ledgers(
        &matrix.evolution_sources,
        &matrix.evolution_audit_items,
        &matrix.evolution_requirements,
    );

    let mut audit_item_ids = HashSet::new();
    for audit_item in &matrix.evolution_audit_items {
        assert!(
            audit_item_ids.insert(audit_item.id.as_str()),
            "duplicate Kotlin evolution audit item ID: {}",
            audit_item.id
        );
        assert_evolution_audit_item(audit_item, false);
    }

    assert_legacy_evolution_links(&matrix.requirements, &audit_item_ids);

    for requirement in &matrix.evolution_requirements {
        assert!(
            requirement_ids.insert(requirement.id.as_str()),
            "duplicate current requirement ID: {}",
            requirement.id
        );
        assert_evolution_requirement(requirement, test_source, primary_tests);
        for audit_item_id in &requirement.audit_item_ids {
            assert!(
                audit_item_ids.contains(audit_item_id.as_str()),
                "{} cites missing evolution audit item {audit_item_id}",
                requirement.id
            );
        }
    }

    for audit_item in &matrix.evolution_audit_items {
        assert_evolution_audit_item_links(audit_item, requirement_ids, &audit_item_ids, false);
        assert_evolution_links_are_bidirectional(matrix, audit_item);
    }

    audit_item_ids
}

fn assert_evolution_links_are_bidirectional(
    matrix: &CoverageMatrix,
    audit_item: &EvolutionAuditItem,
) {
    for requirement_id in &audit_item.requirement_ids {
        let baseline_requirement_has_link = matrix
            .requirements
            .iter()
            .find(|requirement| requirement.id == requirement_id.as_str())
            .is_some_and(|requirement| {
                requirement
                    .evolution_audit_ids
                    .iter()
                    .any(|audit_item_id| audit_item_id == &audit_item.id)
            });
        let evolution_requirement_has_link = matrix
            .evolution_requirements
            .iter()
            .find(|requirement| requirement.id == requirement_id.as_str())
            .is_some_and(|requirement| {
                requirement
                    .audit_item_ids
                    .iter()
                    .any(|audit_item_id| audit_item_id == &audit_item.id)
            });
        assert!(
            baseline_requirement_has_link || evolution_requirement_has_link,
            "{} and {requirement_id} must link each other",
            audit_item.id
        );
    }
}

fn assert_preview_matrix(matrix: &CoverageMatrix, stable_audit_item_ids: &HashSet<&str>) {
    assert_eq!(matrix.preview_sources.len(), 1);
    let source_ledger = &matrix.preview_sources[0];
    assert_eq!(source_ledger.release_line, PREVIEW_RELEASE_LINE);
    assert_eq!(source_ledger.source_revision, PREVIEW_TARGET_REVISION);
    assert_eq!(source_ledger.source_path, "ChangeLog.md");
    assert_eq!(source_ledger.source_start_heading, "## 2.4.20-Beta1");
    assert_eq!(
        source_ledger.source_end_heading.as_deref(),
        Some("## 2.4.0")
    );
    assert_evolution_source_ledger_status(source_ledger, &preview_items(matrix), &[]);

    let mut preview_audit_item_ids = HashSet::new();
    for audit_item in &matrix.preview_audit_items {
        assert!(
            preview_audit_item_ids.insert(audit_item.id.as_str()),
            "duplicate preview audit item ID: {}",
            audit_item.id
        );
        assert!(
            !stable_audit_item_ids.contains(audit_item.id.as_str()),
            "preview audit item {} duplicates a stable audit ID",
            audit_item.id
        );
        assert_evolution_audit_item(audit_item, true);
    }

    let all_audit_item_ids: HashSet<&str> = stable_audit_item_ids
        .iter()
        .copied()
        .chain(preview_audit_item_ids.iter().copied())
        .collect();
    let current_requirement_ids = current_requirement_ids(matrix);
    for audit_item in &matrix.preview_audit_items {
        assert_evolution_audit_item_links(
            audit_item,
            &current_requirement_ids,
            &all_audit_item_ids,
            true,
        );
    }

    match matrix.preview_target.audit_status.as_str() {
        "pending" => assert_eq!(source_ledger.audit_status, "pending"),
        "complete" => assert_eq!(source_ledger.audit_status, "complete"),
        audit_status => panic!("invalid preview target audit status {audit_status}"),
    }
}

fn preview_items(matrix: &CoverageMatrix) -> Vec<&EvolutionAuditItem> {
    matrix.preview_audit_items.iter().collect()
}

fn current_requirement_ids(matrix: &CoverageMatrix) -> HashSet<&str> {
    matrix
        .requirements
        .iter()
        .map(|requirement| requirement.id.as_str())
        .chain(
            matrix
                .evolution_requirements
                .iter()
                .map(|requirement| requirement.id.as_str()),
        )
        .collect()
}

fn assert_documentation_matrix(
    matrix: &CoverageMatrix,
    requirement_ids: &HashSet<&str>,
    evolution_audit_item_ids: &HashSet<&str>,
) {
    assert_eq!(
        matrix.documentation_topics.len(),
        matrix.documentation.topic_count
    );
    let mut topic_paths = HashSet::new();
    for (index, topic_ledger) in matrix.documentation_topics.iter().enumerate() {
        assert_eq!(topic_ledger.toc_order, index + 1);
        assert!(
            topic_ledger
                .source_path
                .starts_with(&format!("{DOCUMENTATION_SOURCE_ROOT}/")),
            "documentation topic is outside the Language guide source root: {}",
            topic_ledger.source_path
        );
        assert!(topic_ledger.source_path.ends_with(".md"));
        assert!(
            topic_paths.insert(topic_ledger.source_path.as_str()),
            "duplicate documentation topic {}",
            topic_ledger.source_path
        );
    }

    let mut documentation_audit_item_ids = HashSet::new();
    for audit_item in &matrix.documentation_audit_items {
        assert!(
            documentation_audit_item_ids.insert(audit_item.id.as_str()),
            "duplicate documentation audit item ID: {}",
            audit_item.id
        );
        assert!(
            topic_paths.contains(audit_item.source_path.as_str()),
            "{} cites a page outside the Language guide ledger: {}",
            audit_item.id,
            audit_item.source_path
        );
        assert_documentation_audit_item(audit_item);
    }

    for audit_item in &matrix.documentation_audit_items {
        assert_documentation_audit_item_links(
            audit_item,
            requirement_ids,
            evolution_audit_item_ids,
            &documentation_audit_item_ids,
        );
    }

    for topic_ledger in &matrix.documentation_topics {
        let audit_items: Vec<&DocumentationAuditItem> = matrix
            .documentation_audit_items
            .iter()
            .filter(|audit_item| audit_item.source_path == topic_ledger.source_path)
            .collect();
        assert_documentation_topic_status(topic_ledger, &audit_items);
    }
}

fn assert_documentation_audit_item(audit_item: &DocumentationAuditItem) {
    let source_stem = Path::new(&audit_item.source_path)
        .file_stem()
        .and_then(|file_stem| file_stem.to_str())
        .expect("documentation source path must have a UTF-8 file stem")
        .replace('_', "-")
        .to_ascii_uppercase();
    let expected_prefix = format!("KDA-{source_stem}-");
    let ordinal = audit_item
        .id
        .strip_prefix(&expected_prefix)
        .unwrap_or_else(|| panic!("{} must start with {expected_prefix}", audit_item.id));
    assert_eq!(
        ordinal.len(),
        4,
        "{} must use a four-digit ordinal",
        audit_item.id
    );
    assert!(ordinal.chars().all(|character| character.is_ascii_digit()));
    assert_nonempty(
        audit_item
            .source_heading
            .as_deref()
            .or(audit_item.source_anchor.as_deref()),
        &audit_item.id,
        "source heading or anchor",
    );
    assert!(audit_item.source_line_start > 0);
    assert!(audit_item.source_line_end >= audit_item.source_line_start);
    assert_nonempty(Some(&audit_item.statement), &audit_item.id, "statement");
}

fn assert_documentation_audit_item_links(
    audit_item: &DocumentationAuditItem,
    requirement_ids: &HashSet<&str>,
    evolution_audit_item_ids: &HashSet<&str>,
    documentation_audit_item_ids: &HashSet<&str>,
) {
    match audit_item.disposition.as_str() {
        "covered-existing" | "covered-changed" | "covered-new" => {
            assert!(
                !audit_item.requirement_ids.is_empty(),
                "{} must link covered claims to current requirements",
                audit_item.id
            );
            assert!(audit_item.duplicate_of.is_none());
            assert!(audit_item.exclusion_kind.is_none());
            assert!(audit_item.exclusion_rationale.is_none());
            for requirement_id in &audit_item.requirement_ids {
                assert!(
                    requirement_ids.contains(requirement_id.as_str()),
                    "{} cites missing current requirement {requirement_id}",
                    audit_item.id
                );
            }
            for evolution_audit_id in &audit_item.evolution_audit_ids {
                assert!(
                    evolution_audit_item_ids.contains(evolution_audit_id.as_str()),
                    "{} cites missing stable evolution item {evolution_audit_id}",
                    audit_item.id
                );
            }
        }
        "duplicate" => {
            assert!(audit_item.requirement_ids.is_empty());
            assert!(audit_item.evolution_audit_ids.is_empty());
            assert!(audit_item.exclusion_kind.is_none());
            assert!(audit_item.exclusion_rationale.is_none());
            let duplicate_of = audit_item
                .duplicate_of
                .as_deref()
                .expect("duplicate documentation claim must name its canonical item");
            assert_ne!(duplicate_of, audit_item.id);
            assert!(
                documentation_audit_item_ids.contains(duplicate_of),
                "{} duplicates missing documentation item {duplicate_of}",
                audit_item.id
            );
        }
        "excluded" => {
            assert!(audit_item.requirement_ids.is_empty());
            assert!(audit_item.evolution_audit_ids.is_empty());
            assert!(audit_item.duplicate_of.is_none());
            assert_documentation_exclusion_kind(
                audit_item.exclusion_kind.as_deref(),
                &audit_item.id,
            );
            assert_nonempty(
                audit_item.exclusion_rationale.as_deref(),
                &audit_item.id,
                "exclusion rationale",
            );
        }
        disposition => panic!(
            "{} has invalid documentation disposition {disposition}",
            audit_item.id
        ),
    }
}

fn assert_documentation_exclusion_kind(exclusion_kind: Option<&str>, item_id: &str) {
    assert!(
        matches!(
            exclusion_kind,
            Some(
                "build-tool"
                    | "compiler-infrastructure"
                    | "ide-only"
                    | "non-behavioral"
                    | "performance"
                    | "platform-specific"
                    | "runtime"
                    | "standard-library"
                    | "style"
            )
        ),
        "{item_id} must provide a valid documentation exclusion kind"
    );
}

fn assert_documentation_topic_status(
    topic_ledger: &DocumentationTopicLedger,
    audit_items: &[&DocumentationAuditItem],
) {
    assert_documentation_audit_item_sequence(topic_ledger, audit_items);
    match topic_ledger.audit_status.as_str() {
        "pending" => {
            assert!(
                audit_items.is_empty(),
                "pending topic {} must not contain audit items",
                topic_ledger.source_path
            );
            assert!(topic_ledger.claim_count.is_none());
            assert!(topic_ledger.linked_requirement_count.is_none());
            assert!(topic_ledger.excluded_claim_count.is_none());
            assert!(topic_ledger.rationale.is_none());
        }
        "complete" => assert_complete_documentation_topic(topic_ledger, audit_items),
        audit_status => panic!(
            "documentation topic {} has invalid audit status {audit_status}",
            topic_ledger.source_path
        ),
    }
}

fn assert_documentation_audit_item_sequence(
    topic_ledger: &DocumentationTopicLedger,
    audit_items: &[&DocumentationAuditItem],
) {
    let source_stem = Path::new(&topic_ledger.source_path)
        .file_stem()
        .and_then(|file_stem| file_stem.to_str())
        .expect("documentation source path must have a UTF-8 file stem")
        .replace('_', "-")
        .to_ascii_uppercase();
    for (index, audit_item) in audit_items.iter().enumerate() {
        let expected_id = format!("KDA-{source_stem}-{:04}", index + 1);
        assert_eq!(
            audit_item.id, expected_id,
            "documentation audit IDs must be contiguous for {}",
            topic_ledger.source_path
        );
    }
}

fn assert_complete_documentation_topic(
    topic_ledger: &DocumentationTopicLedger,
    audit_items: &[&DocumentationAuditItem],
) {
    assert_eq!(
        topic_ledger.claim_count,
        Some(audit_items.len()),
        "documentation claim count differs for {}",
        topic_ledger.source_path
    );
    let linked_requirement_ids: HashSet<&str> = audit_items
        .iter()
        .flat_map(|audit_item| audit_item.requirement_ids.iter().map(String::as_str))
        .collect();
    assert_eq!(
        topic_ledger.linked_requirement_count,
        Some(linked_requirement_ids.len()),
        "documentation requirement count differs for {}",
        topic_ledger.source_path
    );
    let excluded_claim_count = audit_items
        .iter()
        .filter(|audit_item| audit_item.disposition == "excluded")
        .count();
    assert_eq!(
        topic_ledger.excluded_claim_count,
        Some(excluded_claim_count),
        "documentation exclusion count differs for {}",
        topic_ledger.source_path
    );
    if audit_items.is_empty() {
        assert_nonempty(
            topic_ledger.rationale.as_deref(),
            &topic_ledger.source_path,
            "zero-claim rationale",
        );
    } else {
        assert!(topic_ledger.rationale.is_none());
    }
}

fn assert_legacy_evolution_links(requirements: &[Requirement], audit_item_ids: &HashSet<&str>) {
    for requirement in requirements {
        if requirement.evolution_audit_ids.is_empty() {
            assert!(
                requirement.migration_note.is_none(),
                "{} has a migration note without evolution evidence",
                requirement.id
            );
            continue;
        }

        assert_nonempty(
            requirement.migration_note.as_deref(),
            &requirement.id,
            "migration note",
        );
        assert!(
            !requirement.documentation_citations.is_empty()
                || !requirement.migration_citations.is_empty(),
            "{} must cite current or migration documentation when baseline behavior changes",
            requirement.id
        );
        for audit_item_id in &requirement.evolution_audit_ids {
            assert!(
                audit_item_ids.contains(audit_item_id.as_str()),
                "{} cites missing evolution audit item {audit_item_id}",
                requirement.id
            );
        }
    }
}

fn assert_documentation_citations(matrix: &CoverageMatrix) {
    let topic_paths: HashSet<&str> = matrix
        .documentation_topics
        .iter()
        .map(|topic| topic.source_path.as_str())
        .collect();

    for requirement in &matrix.requirements {
        for citation in &requirement.documentation_citations {
            assert_documentation_citation(citation, &requirement.id, &topic_paths);
            assert_documentation_citation_is_linked(
                citation,
                &requirement.id,
                &matrix.documentation_audit_items,
            );
        }
        for citation in &requirement.migration_citations {
            assert_migration_citation(citation, &requirement.id);
        }
    }

    for requirement in &matrix.evolution_requirements {
        for citation in &requirement.documentation_citations {
            assert_documentation_citation(citation, &requirement.id, &topic_paths);
            assert_documentation_citation_is_linked(
                citation,
                &requirement.id,
                &matrix.documentation_audit_items,
            );
        }
        for citation in &requirement.migration_citations {
            assert_migration_citation(citation, &requirement.id);
        }
        for citation in &requirement.compiler_citations {
            assert_kotlin_citation(citation, &requirement.id);
        }
        if requirement.classification != "out-of-scope" {
            let has_documentation_oracle = !requirement.documentation_citations.is_empty();
            let has_migration_oracle = !requirement.migration_citations.is_empty();
            let has_compiler_oracle = !requirement.compiler_citations.is_empty();
            assert!(
                has_documentation_oracle || has_migration_oracle || has_compiler_oracle,
                "{} must cite pinned documentation or compiler evidence",
                requirement.id
            );
        }
    }

    for audit_item in &matrix.documentation_audit_items {
        for requirement_id in &audit_item.requirement_ids {
            let citations = requirement_documentation_citations(matrix, requirement_id);
            assert!(
                citations.iter().any(|citation| {
                    documentation_citation_matches_audit_item(citation, audit_item)
                }),
                "{requirement_id} must cite linked documentation claim {}",
                audit_item.id
            );
        }
    }
}

fn assert_migration_citation(citation: &DocumentationCitation, requirement_id: &str) {
    assert_eq!(citation.repository, DOCUMENTATION_REPOSITORY);
    assert_eq!(citation.revision, DOCUMENTATION_REVISION);
    assert!(
        is_migration_documentation_path(&citation.source_path),
        "{requirement_id} cites an unsupported migration index: {}",
        citation.source_path
    );
    assert_nonempty(
        citation
            .source_heading
            .as_deref()
            .or(citation.source_anchor.as_deref()),
        requirement_id,
        "migration documentation heading or anchor",
    );
    assert!(citation.source_line_start > 0);
    assert!(citation.source_line_end >= citation.source_line_start);
}

fn is_migration_documentation_path(source_path: &str) -> bool {
    let is_whats_new =
        source_path.starts_with("docs/topics/whatsnew/whatsnew") && source_path.ends_with(".md");
    let is_compatibility_guide = source_path
        .starts_with("docs/topics/compatibility-guides/compatibility-guide-")
        && source_path.ends_with(".md");
    is_whats_new || is_compatibility_guide
}

fn assert_documentation_citation(
    citation: &DocumentationCitation,
    requirement_id: &str,
    topic_paths: &HashSet<&str>,
) {
    assert_eq!(citation.repository, DOCUMENTATION_REPOSITORY);
    assert_eq!(citation.revision, DOCUMENTATION_REVISION);
    assert!(
        topic_paths.contains(citation.source_path.as_str()),
        "{requirement_id} cites a page outside the Language guide ledger: {}",
        citation.source_path
    );
    assert_nonempty(
        citation
            .source_heading
            .as_deref()
            .or(citation.source_anchor.as_deref()),
        requirement_id,
        "documentation heading or anchor",
    );
    assert!(citation.source_line_start > 0);
    assert!(citation.source_line_end >= citation.source_line_start);
}

fn assert_documentation_citation_is_linked(
    citation: &DocumentationCitation,
    requirement_id: &str,
    audit_items: &[DocumentationAuditItem],
) {
    assert!(
        audit_items.iter().any(|audit_item| {
            audit_item
                .requirement_ids
                .iter()
                .any(|linked_id| linked_id == requirement_id)
                && documentation_citation_matches_audit_item(citation, audit_item)
        }),
        "{requirement_id} has an unlinked documentation citation for {}:{}-{}",
        citation.source_path,
        citation.source_line_start,
        citation.source_line_end
    );
}

fn documentation_citation_matches_audit_item(
    citation: &DocumentationCitation,
    audit_item: &DocumentationAuditItem,
) -> bool {
    citation.source_path == audit_item.source_path
        && citation.source_heading == audit_item.source_heading
        && citation.source_anchor == audit_item.source_anchor
        && citation.source_line_start == audit_item.source_line_start
        && citation.source_line_end == audit_item.source_line_end
}

fn requirement_documentation_citations<'matrix>(
    matrix: &'matrix CoverageMatrix,
    requirement_id: &str,
) -> Vec<&'matrix DocumentationCitation> {
    if let Some(requirement) = matrix
        .requirements
        .iter()
        .find(|requirement| requirement.id == requirement_id)
    {
        return requirement.documentation_citations.iter().collect();
    }
    matrix
        .evolution_requirements
        .iter()
        .find(|requirement| requirement.id == requirement_id)
        .unwrap_or_else(|| panic!("missing current requirement {requirement_id}"))
        .documentation_citations
        .iter()
        .collect()
}

fn assert_kotlin_citation(citation: &KotlinCitation, requirement_id: &str) {
    assert_eq!(citation.revision, LANGUAGE_TARGET_REVISION);
    assert_nonempty(
        Some(&citation.source_path),
        requirement_id,
        "compiler source path",
    );
    assert_nonempty(
        citation
            .source_heading
            .as_deref()
            .or(citation.source_anchor.as_deref()),
        requirement_id,
        "compiler source heading or anchor",
    );
    assert!(citation.source_line_start > 0);
    assert!(citation.source_line_end >= citation.source_line_start);
}

fn assert_complete_language_target_has_no_unfinished_ledgers(matrix: &CoverageMatrix) {
    if matrix.language_target.audit_status != "complete" {
        return;
    }

    assert!(
        matrix
            .evolution_sources
            .iter()
            .all(|source| source.audit_status == "complete"),
        "stable evolution ledgers must be complete"
    );
    assert!(
        matrix
            .documentation_topics
            .iter()
            .all(|topic| topic.audit_status == "complete"),
        "documentation ledgers must be complete"
    );
    assert_eq!(matrix.preview_target.audit_status, "complete");
    assert!(
        matrix
            .preview_sources
            .iter()
            .all(|source| source.audit_status == "complete"),
        "preview source ledgers must be complete"
    );

    for (release_line, fragment) in EVOLUTION_RELEASE_LINES.iter().zip(EVOLUTION_FRAGMENTS) {
        assert_no_unfinished_markers(fragment, release_line);
    }
    assert_no_unfinished_markers(PREVIEW_EVOLUTION_FRAGMENT, PREVIEW_RELEASE_LINE);
    assert_no_unfinished_markers(DOCUMENTATION_FRAGMENT, "Language guide");
}

fn assert_no_unfinished_markers(document: &str, source_name: &str) {
    let lowercase_document = document.to_ascii_lowercase();
    for marker in ["pending", "in-progress", "uncertain", "todo", "placeholder"] {
        assert!(
            !lowercase_document.contains(marker),
            "complete source {source_name} contains unfinished marker {marker}"
        );
    }
}

fn assert_evolution_source_ledgers(
    source_ledgers: &[EvolutionSourceLedger],
    audit_items: &[EvolutionAuditItem],
    requirements: &[EvolutionRequirement],
) {
    assert_eq!(source_ledgers.len(), EVOLUTION_RELEASE_LINES.len());

    for ((source_ledger, expected_release_line), expected_identity) in source_ledgers
        .iter()
        .zip(EVOLUTION_RELEASE_LINES)
        .zip(EVOLUTION_SOURCE_IDENTITIES)
    {
        assert_eq!(source_ledger.release_line, expected_release_line);
        let (expected_revision, expected_path, expected_start_heading, expected_end_heading) =
            expected_identity;
        assert_eq!(source_ledger.source_revision, expected_revision);
        assert_eq!(source_ledger.source_path, expected_path);
        assert_eq!(source_ledger.source_start_heading, expected_start_heading);
        assert_eq!(
            source_ledger.source_end_heading.as_deref(),
            expected_end_heading
        );

        let source_audit_items: Vec<&EvolutionAuditItem> = audit_items
            .iter()
            .filter(|audit_item| audit_item.release_line == source_ledger.release_line)
            .collect();
        let source_requirements: Vec<&EvolutionRequirement> = requirements
            .iter()
            .filter(|requirement| requirement.introduced_in == source_ledger.release_line)
            .collect();
        assert_evolution_source_ledger_status(
            source_ledger,
            &source_audit_items,
            &source_requirements,
        );
    }
}

fn assert_evolution_source_ledger_status(
    source_ledger: &EvolutionSourceLedger,
    audit_items: &[&EvolutionAuditItem],
    requirements: &[&EvolutionRequirement],
) {
    assert_evolution_audit_item_sequence(source_ledger, audit_items);
    match source_ledger.audit_status.as_str() {
        "pending" => {
            assert!(
                audit_items.is_empty(),
                "pending release {} must not contain audit items",
                source_ledger.release_line
            );
            assert!(
                requirements.is_empty(),
                "pending release {} must not contain requirements",
                source_ledger.release_line
            );
            assert!(source_ledger.audit_item_count.is_none());
            assert!(evolution_source_ledger_counts(source_ledger)
                .iter()
                .all(Option::is_none));
            assert!(source_ledger.rationale.is_none());
        }
        "complete" => {
            assert_complete_evolution_source_ledger(source_ledger, audit_items, requirements)
        }
        audit_status => panic!(
            "release {} has invalid audit status {audit_status}",
            source_ledger.release_line
        ),
    }
}

fn assert_evolution_audit_item_sequence(
    source_ledger: &EvolutionSourceLedger,
    audit_items: &[&EvolutionAuditItem],
) {
    let normalized_release_line = source_ledger
        .release_line
        .replace('.', "-")
        .to_ascii_uppercase();
    for (index, audit_item) in audit_items.iter().enumerate() {
        let expected_id = format!("KCA-{normalized_release_line}-{:04}", index + 1);
        assert_eq!(
            audit_item.id, expected_id,
            "release {} audit IDs must be contiguous in source order",
            source_ledger.release_line
        );
    }
}

fn assert_complete_evolution_source_ledger(
    source_ledger: &EvolutionSourceLedger,
    audit_items: &[&EvolutionAuditItem],
    requirements: &[&EvolutionRequirement],
) {
    assert_eq!(
        source_ledger.audit_item_count,
        Some(audit_items.len()),
        "release {} audit-item count does not match its fragment",
        source_ledger.release_line
    );
    let expected_counts = evolution_source_ledger_counts(source_ledger);
    assert!(
        expected_counts.iter().all(Option::is_some),
        "complete release {} must provide every requirement count",
        source_ledger.release_line
    );
    let declared_counts = expected_counts.map(|count| count.expect("counts checked above"));
    let actual_counts = evolution_requirement_counts(requirements);
    assert_eq!(
        declared_counts, actual_counts,
        "release {} requirement counts do not match its fragment",
        source_ledger.release_line
    );
    if audit_items.is_empty() && requirements.is_empty() {
        assert_nonempty(
            source_ledger.rationale.as_deref(),
            &source_ledger.release_line,
            "zero-item rationale",
        );
    } else {
        assert!(source_ledger.rationale.is_none());
    }
}

fn evolution_source_ledger_counts(source_ledger: &EvolutionSourceLedger) -> [Option<usize>; 5] {
    [
        source_ledger.exact_active,
        source_ledger.exact_ignored,
        source_ledger.heuristic_active,
        source_ledger.heuristic_ignored,
        source_ledger.out_of_scope_excluded,
    ]
}

fn evolution_requirement_counts(requirements: &[&EvolutionRequirement]) -> [usize; 5] {
    let mut counts = [0; 5];
    for requirement in requirements {
        let count_index = classification_status_index(
            &requirement.id,
            &requirement.classification,
            &requirement.status,
        );
        counts[count_index] += 1;
    }
    counts
}

fn assert_evolution_audit_item(audit_item: &EvolutionAuditItem, is_preview: bool) {
    if is_preview {
        assert_eq!(audit_item.release_line, PREVIEW_RELEASE_LINE);
    } else {
        assert_release_line(&audit_item.release_line, &audit_item.id);
    }
    assert_evolution_audit_item_id(audit_item);
    assert_nonempty(
        Some(&audit_item.source_heading),
        &audit_item.id,
        "source heading",
    );
    assert_nonempty(
        Some(&audit_item.source_category),
        &audit_item.id,
        "source category",
    );
    assert!(audit_item.source_line_start > 0);
    assert!(audit_item.source_line_end >= audit_item.source_line_start);
    assert_nonempty(Some(&audit_item.statement), &audit_item.id, "statement");
}

fn assert_evolution_audit_item_id(audit_item: &EvolutionAuditItem) {
    let normalized_release_line = audit_item
        .release_line
        .replace('.', "-")
        .to_ascii_uppercase();
    let expected_prefix = format!("KCA-{normalized_release_line}-");
    let ordinal = audit_item
        .id
        .strip_prefix(&expected_prefix)
        .unwrap_or_else(|| panic!("{} must start with {expected_prefix}", audit_item.id));
    assert_eq!(
        ordinal.len(),
        4,
        "{} must use a four-digit ordinal",
        audit_item.id
    );
    assert!(ordinal.chars().all(|character| character.is_ascii_digit()));
}

fn assert_evolution_audit_item_links(
    audit_item: &EvolutionAuditItem,
    requirement_ids: &HashSet<&str>,
    audit_item_ids: &HashSet<&str>,
    is_preview: bool,
) {
    match audit_item.disposition.as_str() {
        "covered-new" | "covered-changed" | "covered-existing" => {
            assert!(!is_preview, "{} is preview-only", audit_item.id);
            assert!(
                !audit_item.requirement_ids.is_empty(),
                "{} must link covered behavior to current requirements",
                audit_item.id
            );
            assert!(audit_item.duplicate_of.is_none());
            assert!(audit_item.rationale.is_none());
            assert!(audit_item.exclusion_kind.is_none());
            for requirement_id in &audit_item.requirement_ids {
                assert!(
                    requirement_ids.contains(requirement_id.as_str()),
                    "{} cites missing current requirement {requirement_id}",
                    audit_item.id
                );
            }
        }
        "duplicate" => {
            assert!(audit_item.requirement_ids.is_empty());
            let duplicate_of = audit_item
                .duplicate_of
                .as_deref()
                .expect("duplicate audit item must name its canonical item");
            assert!(
                audit_item_ids.contains(duplicate_of),
                "{} duplicates missing audit item {duplicate_of}",
                audit_item.id
            );
            assert!(audit_item.rationale.is_none());
            assert!(audit_item.exclusion_kind.is_none());
        }
        "excluded" => {
            assert!(audit_item.requirement_ids.is_empty());
            assert!(audit_item.duplicate_of.is_none());
            assert_nonempty(audit_item.rationale.as_deref(), &audit_item.id, "rationale");
            assert_evolution_exclusion_kind(audit_item.exclusion_kind.as_deref(), &audit_item.id);
        }
        "outside-target" => {
            assert!(audit_item.requirement_ids.is_empty());
            assert!(audit_item.duplicate_of.is_none());
            assert!(audit_item.exclusion_kind.is_none());
            assert_nonempty(audit_item.rationale.as_deref(), &audit_item.id, "rationale");
            assert_nonempty(
                audit_item.target_membership_evidence.as_deref(),
                &audit_item.id,
                "target-membership evidence",
            );
        }
        "preview-deferred" => {
            assert!(is_preview, "{} is not a preview audit item", audit_item.id);
            assert!(audit_item.requirement_ids.is_empty());
            assert!(audit_item.duplicate_of.is_none());
            assert!(audit_item.exclusion_kind.is_none());
            assert_nonempty(audit_item.rationale.as_deref(), &audit_item.id, "rationale");
            assert_nonempty(
                audit_item.target_membership_evidence.as_deref(),
                &audit_item.id,
                "stable-target boundary evidence",
            );
        }
        disposition => panic!(
            "{} has invalid evolution disposition {disposition}",
            audit_item.id
        ),
    }
}

fn assert_evolution_exclusion_kind(exclusion_kind: Option<&str>, item_id: &str) {
    assert!(
        matches!(
            exclusion_kind,
            Some(
                "analysis-api"
                    | "backend"
                    | "build-tool"
                    | "code-generation"
                    | "compiler-infrastructure"
                    | "compiler-plugin"
                    | "ide-only"
                    | "performance"
                    | "platform-specific"
                    | "runtime"
                    | "standard-library"
                    | "test-infrastructure"
            )
        ),
        "{item_id} must provide a valid evolution exclusion kind"
    );
}

fn assert_evolution_requirement(
    requirement: &EvolutionRequirement,
    test_source: &str,
    primary_tests: &mut HashSet<String>,
) {
    assert_release_line(&requirement.introduced_in, &requirement.id);
    assert_evolution_requirement_id(requirement);
    assert_evolution_maturity(requirement);
    assert!(
        matches!(requirement.change_kind.as_str(), "new" | "changed"),
        "{} has invalid change kind {}",
        requirement.id,
        requirement.change_kind
    );
    assert_nonempty(Some(&requirement.statement), &requirement.id, "statement");
    assert!(!requirement.capabilities.is_empty());
    assert_nonempty(Some(&requirement.oracle), &requirement.id, "oracle");
    assert_fallback_oracle(requirement.fallback_oracle.as_deref(), &requirement.id);
    assert_evolution_requirement_classification(requirement);
    assert_evolution_primary_tests(requirement, test_source, primary_tests);
}

fn assert_evolution_maturity(requirement: &EvolutionRequirement) {
    assert!(
        matches!(
            requirement.maturity.as_str(),
            "preview" | "experimental" | "beta" | "stable"
        ),
        "{} has invalid maturity {}",
        requirement.id,
        requirement.maturity
    );
    if let Some(required_compiler_flag) = requirement.required_compiler_flag.as_deref() {
        assert_nonempty(
            Some(required_compiler_flag),
            &requirement.id,
            "required compiler flag",
        );
    }
    if let Some(required_opt_in) = requirement.required_opt_in.as_deref() {
        assert_nonempty(Some(required_opt_in), &requirement.id, "required opt-in");
    }

    if requirement.maturity == "stable" {
        let stabilized_in = requirement
            .stabilized_in
            .as_deref()
            .unwrap_or_else(|| panic!("{} must name its stabilization release", requirement.id));
        assert_release_line(stabilized_in, &requirement.id);
        assert!(requirement.required_compiler_flag.is_none());
        assert!(requirement.required_opt_in.is_none());
    } else {
        assert!(requirement.stabilized_in.is_none());
    }

    let mut maturity_changes = HashSet::new();
    for maturity_change in &requirement.maturity_changes {
        assert!(
            maturity_changes.insert(maturity_change.as_str()),
            "{} repeats maturity change {maturity_change}",
            requirement.id
        );
        let (release_line, maturity) = maturity_change.split_once(':').unwrap_or_else(|| {
            panic!(
                "{} maturity changes must use <release-line>:<maturity>",
                requirement.id
            )
        });
        assert_release_line(release_line.trim(), &requirement.id);
        assert!(matches!(
            maturity.trim(),
            "preview" | "experimental" | "beta" | "stable"
        ));
    }
}

fn assert_evolution_requirement_id(requirement: &EvolutionRequirement) {
    let normalized_release_line = requirement.introduced_in.replace('.', "-");
    let expected_prefix = format!("KL-{normalized_release_line}-");
    let ordinal = requirement
        .id
        .strip_prefix(&expected_prefix)
        .unwrap_or_else(|| panic!("{} must start with {expected_prefix}", requirement.id));
    assert_eq!(
        ordinal.len(),
        4,
        "{} must use a four-digit ordinal",
        requirement.id
    );
    assert!(ordinal.chars().all(|character| character.is_ascii_digit()));
}

fn assert_evolution_requirement_classification(requirement: &EvolutionRequirement) {
    match requirement.classification.as_str() {
        "exact" | "heuristic" => assert_evolution_testable_requirement(requirement),
        "out-of-scope" => assert_evolution_excluded_requirement(requirement),
        classification => panic!(
            "{} has invalid classification {classification}",
            requirement.id
        ),
    }
}

fn assert_evolution_testable_requirement(requirement: &EvolutionRequirement) {
    assert!(matches!(requirement.status.as_str(), "active" | "ignored"));
    assert!(!requirement.tests.is_empty());
    assert_nonempty(requirement.fixture.as_deref(), &requirement.id, "fixture");
    assert_nonempty(
        requirement.sample_evidence.as_deref(),
        &requirement.id,
        "sample evidence",
    );
    assert!(requirement.exclusion_kind.is_none());
    assert!(requirement.exclusion_rationale.is_none());

    if requirement.classification == "heuristic" {
        assert_nonempty(
            requirement.heuristic_limitations.as_deref(),
            &requirement.id,
            "heuristic limitations",
        );
    } else {
        assert!(requirement.heuristic_limitations.is_none());
    }

    if requirement.status == "ignored" {
        assert_nonempty(
            requirement.ignore_reason.as_deref(),
            &requirement.id,
            "ignore reason",
        );
        assert_nonempty(
            requirement.observed_failure.as_deref(),
            &requirement.id,
            "observed failure",
        );
        assert_nonempty(
            requirement.expected_behavior.as_deref(),
            &requirement.id,
            "expected behavior",
        );
    } else {
        assert!(requirement.ignore_reason.is_none());
        assert!(requirement.observed_failure.is_none());
        assert!(requirement.expected_behavior.is_none());
    }
}

fn assert_evolution_excluded_requirement(requirement: &EvolutionRequirement) {
    assert_eq!(requirement.status, "excluded");
    assert!(requirement.tests.is_empty());
    assert!(requirement.fixture.is_none());
    assert!(requirement.sample_evidence.is_none());
    assert!(requirement.ignore_reason.is_none());
    assert!(requirement.observed_failure.is_none());
    assert!(requirement.expected_behavior.is_none());
    assert!(requirement.heuristic_limitations.is_none());
    assert_exclusion_kind(requirement.exclusion_kind.as_deref(), &requirement.id);
    assert_nonempty(
        requirement.exclusion_rationale.as_deref(),
        &requirement.id,
        "exclusion rationale",
    );
}

fn assert_evolution_primary_tests(
    requirement: &EvolutionRequirement,
    test_source: &str,
    primary_tests: &mut HashSet<String>,
) {
    for test_name in &requirement.tests {
        let expected_prefix = requirement.id.to_ascii_lowercase().replace('-', "_");
        assert!(
            test_name.starts_with(&expected_prefix),
            "primary test {test_name} must start with {expected_prefix}"
        );
        assert!(
            primary_tests.insert(test_name.clone()),
            "test {test_name} is primary evidence for more than one requirement"
        );
        assert!(
            test_source.contains(&format!("fn {test_name}(")),
            "test {test_name} named by {} does not exist",
            requirement.id
        );
        assert_test_status_for_requirement(
            &requirement.id,
            &requirement.status,
            test_name,
            test_source,
        );
    }
}

fn assert_retired_requirements(matrix: &CoverageMatrix, current_requirement_ids: &HashSet<&str>) {
    let mut retired_ids = HashSet::new();
    for requirement in &matrix.retired_requirements {
        assert!(
            retired_ids.insert(requirement.id.as_str()),
            "duplicate retired requirement ID: {}",
            requirement.id
        );
        assert!(
            !current_requirement_ids.contains(requirement.id.as_str()),
            "retired requirement {} remains current",
            requirement.id
        );
        assert_release_line(&requirement.retired_in, &requirement.id);
        assert_nonempty(Some(&requirement.statement), &requirement.id, "statement");
        assert_nonempty(Some(&requirement.rationale), &requirement.id, "rationale");
        assert!(!requirement.source_revision.trim().is_empty());
        assert!(!requirement.source_path.trim().is_empty());
        assert!(requirement.source_line_start > 0);
        assert!(requirement.source_line_end >= requirement.source_line_start);
        for replacement_id in &requirement.replacement_ids {
            assert!(
                current_requirement_ids.contains(replacement_id.as_str()),
                "{} cites missing replacement {replacement_id}",
                requirement.id
            );
        }
    }
}

fn assert_legacy_requirements_are_verified_when_complete(matrix: &CoverageMatrix) {
    if matrix.language_target.audit_status != "complete" {
        return;
    }

    for requirement in &matrix.requirements {
        assert_eq!(
            requirement.verified_through.as_deref(),
            Some(LANGUAGE_TARGET_VERSION),
            "{} must be verified through Kotlin {}",
            requirement.id,
            LANGUAGE_TARGET_VERSION
        );
    }
}

fn assert_release_line(release_line: &str, item_id: &str) {
    assert!(
        EVOLUTION_RELEASE_LINES.contains(&release_line),
        "{item_id} has invalid release line {release_line}"
    );
}

fn assert_source_ledgers<'matrix>(
    sources: &'matrix [SourceLedger],
    requirements: &[Requirement],
) -> HashMap<&'matrix str, &'matrix SourceLedger> {
    assert_eq!(sources.len(), NORMATIVE_SOURCES.len());
    let mut source_ledgers = HashMap::new();

    for (source_ledger, expected_path) in sources.iter().zip(NORMATIVE_SOURCES) {
        assert_eq!(
            source_ledger.path, expected_path,
            "source ledger order changed"
        );
        assert!(
            source_ledgers
                .insert(source_ledger.path.as_str(), source_ledger)
                .is_none(),
            "duplicate source ledger entry: {}",
            source_ledger.path
        );
        assert_source_ledger_status(source_ledger, requirements);
    }

    source_ledgers
}

fn assert_source_ledger_status(source_ledger: &SourceLedger, requirements: &[Requirement]) {
    match source_ledger.audit_status.as_str() {
        "pending" => {
            assert!(
                source_ledger_counts(source_ledger)
                    .iter()
                    .all(Option::is_none),
                "pending source {} must not claim final counts",
                source_ledger.path
            );
            assert!(
                source_ledger.rationale.is_none(),
                "pending source {} must not claim a final rationale",
                source_ledger.path
            );
        }
        "complete" => assert_complete_source_ledger(source_ledger, requirements),
        audit_status => panic!(
            "source {} has invalid audit status {audit_status}",
            source_ledger.path
        ),
    }
}

fn assert_complete_source_ledger(source_ledger: &SourceLedger, requirements: &[Requirement]) {
    let expected_counts = source_ledger_counts(source_ledger);
    assert!(
        expected_counts.iter().all(Option::is_some),
        "complete source {} must provide every final count",
        source_ledger.path
    );

    let actual_counts = requirement_counts_for_source(&source_ledger.path, requirements);
    let declared_counts = expected_counts.map(|count| count.expect("counts checked above"));
    assert_eq!(
        declared_counts, actual_counts,
        "source ledger counts do not match requirements for {}",
        source_ledger.path
    );

    let total_requirements: usize = actual_counts.iter().sum();
    if total_requirements == 0 {
        assert_nonempty(
            source_ledger.rationale.as_deref(),
            &source_ledger.path,
            "zero-requirement rationale",
        );
    } else {
        assert!(
            source_ledger.rationale.is_none(),
            "source {} with requirements must not carry a zero-requirement rationale",
            source_ledger.path
        );
    }
}

fn source_ledger_counts(source_ledger: &SourceLedger) -> [Option<usize>; 5] {
    [
        source_ledger.exact_active,
        source_ledger.exact_ignored,
        source_ledger.heuristic_active,
        source_ledger.heuristic_ignored,
        source_ledger.out_of_scope_excluded,
    ]
}

fn requirement_counts_for_source(source_path: &str, requirements: &[Requirement]) -> [usize; 5] {
    let mut counts = [0; 5];
    for requirement in requirements {
        if requirement.source_file != source_path {
            continue;
        }
        let count_index = classification_status_index(
            &requirement.id,
            &requirement.classification,
            &requirement.status,
        );
        counts[count_index] += 1;
    }
    counts
}

fn classification_status_index(item_id: &str, classification: &str, status: &str) -> usize {
    match (classification, status) {
        ("exact", "active") => 0,
        ("exact", "ignored") => 1,
        ("heuristic", "active") => 2,
        ("heuristic", "ignored") => 3,
        ("out-of-scope", "excluded") => 4,
        _ => panic!("{item_id} has an invalid classification/status"),
    }
}

fn assert_requirement(requirement: &Requirement, source_ledgers: &HashMap<&str, &SourceLedger>) {
    assert!(!requirement.id.trim().is_empty());
    assert!(!requirement.statement.trim().is_empty());
    assert!(!requirement.capabilities.is_empty());
    assert!(!requirement.oracle.trim().is_empty());

    assert_migrated_requirement(requirement, &requirement.source_file, source_ledgers);
}

fn assert_migrated_requirement(
    requirement: &Requirement,
    source_file: &str,
    source_ledgers: &HashMap<&str, &SourceLedger>,
) {
    assert!(
        source_ledgers.contains_key(source_file),
        "{} cites a source outside the normative ledger: {source_file}",
        requirement.id
    );
    assert_nonempty(
        requirement
            .source_heading
            .as_deref()
            .or(requirement.source_anchor.as_deref()),
        &requirement.id,
        "source heading or anchor",
    );
    assert!(requirement.source_line_start > 0);
    assert!(requirement.source_line_end >= requirement.source_line_start);
    assert_source_native_id(requirement, source_file);
    assert!(
        !requirement.oracle.to_ascii_lowercase().contains("pdf"),
        "migrated {} retains a PDF oracle",
        requirement.id
    );
    assert_fallback_oracle(requirement.fallback_oracle.as_deref(), &requirement.id);
    assert_migrated_classification(requirement);
}

fn assert_fallback_oracle(fallback_oracle: Option<&str>, item_id: &str) {
    if let Some(fallback_oracle) = fallback_oracle {
        assert!(
            !fallback_oracle.trim_start().starts_with("Not used"),
            "{item_id} must omit unused fallback metadata"
        );
    }
}

fn assert_source_native_id(requirement: &Requirement, source_file: &str) {
    let source_stem = Path::new(source_file)
        .file_stem()
        .and_then(|file_stem| file_stem.to_str())
        .expect("normative source path must have a UTF-8 file stem")
        .to_ascii_uppercase();
    let expected_prefix = format!("KS-{source_stem}-");
    let ordinal = requirement
        .id
        .strip_prefix(&expected_prefix)
        .unwrap_or_else(|| panic!("{} must start with {expected_prefix}", requirement.id));
    assert_eq!(
        ordinal.len(),
        4,
        "{} must use a four-digit ordinal",
        requirement.id
    );
    assert!(ordinal.chars().all(|character| character.is_ascii_digit()));
}

fn assert_migrated_classification(requirement: &Requirement) {
    match requirement.classification.as_str() {
        "exact" | "heuristic" => assert_migrated_testable_requirement(requirement),
        "out-of-scope" => assert_excluded_requirement(requirement),
        classification => panic!(
            "{} has invalid migrated classification {classification}",
            requirement.id
        ),
    }
}

fn assert_migrated_testable_requirement(requirement: &Requirement) {
    assert!(matches!(requirement.status.as_str(), "active" | "ignored"));
    assert!(!requirement.tests.is_empty());
    assert_nonempty(requirement.fixture.as_deref(), &requirement.id, "fixture");
    assert_nonempty(
        requirement.sample_evidence.as_deref(),
        &requirement.id,
        "sample evidence",
    );
    assert!(requirement.exclusion_kind.is_none());
    assert!(requirement.exclusion_rationale.is_none());

    if requirement.classification == "heuristic" {
        assert_nonempty(
            requirement.heuristic_limitations.as_deref(),
            &requirement.id,
            "heuristic limitations",
        );
    } else {
        assert!(requirement.heuristic_limitations.is_none());
    }

    if requirement.status == "ignored" {
        assert_nonempty(
            requirement.ignore_reason.as_deref(),
            &requirement.id,
            "ignore reason",
        );
        assert_nonempty(
            requirement.observed_failure.as_deref(),
            &requirement.id,
            "observed failure",
        );
        assert_nonempty(
            requirement.expected_behavior.as_deref(),
            &requirement.id,
            "expected behavior",
        );
    } else {
        assert!(requirement.ignore_reason.is_none());
        assert!(requirement.observed_failure.is_none());
        assert!(requirement.expected_behavior.is_none());
    }
}

fn assert_excluded_requirement(requirement: &Requirement) {
    assert_eq!(requirement.status, "excluded");
    assert!(requirement.tests.is_empty());
    assert!(requirement.fixture.is_none());
    assert!(requirement.sample_evidence.is_none());
    assert!(requirement.ignore_reason.is_none());
    assert!(requirement.observed_failure.is_none());
    assert!(requirement.expected_behavior.is_none());
    assert!(requirement.heuristic_limitations.is_none());
    assert_exclusion_kind(requirement.exclusion_kind.as_deref(), &requirement.id);
    assert_nonempty(
        requirement.exclusion_rationale.as_deref(),
        &requirement.id,
        "exclusion rationale",
    );
}

fn assert_exclusion_kind(exclusion_kind: Option<&str>, item_id: &str) {
    assert!(
        matches!(
            exclusion_kind,
            Some(
                "compiler-semantics"
                    | "runtime"
                    | "platform-defined"
                    | "standard-library"
                    | "unspecified"
            )
        ),
        "{item_id} must provide a valid exclusion kind"
    );
}

fn assert_primary_tests(
    requirement: &Requirement,
    test_source: &str,
    primary_tests: &mut HashSet<String>,
) {
    for test_name in &requirement.tests {
        assert!(test_name.starts_with("ks_"));
        let expected_prefix = requirement.id.to_ascii_lowercase().replace('-', "_");
        assert!(
            test_name.starts_with(&expected_prefix),
            "primary test {test_name} must start with {expected_prefix}"
        );
        assert!(
            primary_tests.insert(test_name.clone()),
            "test {test_name} is primary evidence for more than one requirement"
        );
        assert!(
            test_source.contains(&format!("fn {test_name}(")),
            "test {test_name} named by {} does not exist",
            requirement.id
        );
        assert_test_status(requirement, test_name, test_source);
    }

    for duplicate in &requirement.duplicates {
        assert!(!requirement.tests.contains(duplicate));
    }
}

fn assert_test_status(requirement: &Requirement, test_name: &str, test_source: &str) {
    assert_test_status_for_requirement(
        &requirement.id,
        &requirement.status,
        test_name,
        test_source,
    );
}

fn assert_test_status_for_requirement(
    requirement_id: &str,
    status: &str,
    test_name: &str,
    test_source: &str,
) {
    let function_marker = format!("fn {test_name}(");
    let function_position = test_source
        .find(&function_marker)
        .expect("test existence is checked before its status");
    let declaration_prefix = &test_source[..function_position];
    let attribute_start = declaration_prefix
        .rfind("\n\n")
        .map_or(0, |position| position + 2);
    let attributes = &test_source[attribute_start..function_position];
    let is_ignored = attributes.contains("#[ignore");

    assert_eq!(
        is_ignored,
        status == "ignored",
        "test {test_name} ignore annotation does not match status {status} for {requirement_id}"
    );
}

fn assert_all_primary_tests_are_traced(test_source: &str, primary_tests: &HashSet<String>) {
    for declaration_suffix in test_source.split("fn ").skip(1) {
        let Some(test_name) = declaration_suffix.split('(').next() else {
            continue;
        };
        if test_name.starts_with("ks_") || test_name.starts_with("kl_") {
            assert!(
                primary_tests.contains(test_name),
                "specification test {test_name} is not primary evidence in coverage.toml"
            );
        }
    }
}

fn assert_checkout_revision(checkout: &Path) {
    let output = Command::new("git")
        .args([
            "-C",
            checkout.to_str().expect("checkout path must be UTF-8"),
            "rev-parse",
            "HEAD",
        ])
        .output()
        .expect("git must be available for the manual source audit");
    assert!(
        output.status.success(),
        "kotlin-spec checkout must be readable"
    );
    let revision = String::from_utf8(output.stdout).expect("git revision must be UTF-8");
    assert_eq!(revision.trim(), SPECIFICATION_REVISION);
}

fn assert_kotlin_target_revision(checkout: &Path, language_target: &LanguageTarget) {
    let target_revision = run_pinned_git_command(
        checkout,
        ["rev-parse", "v2.4.10^{commit}"],
        "Kotlin target tag must be readable",
    );
    assert_eq!(target_revision.trim(), language_target.target_revision);

    let baseline_revision = run_pinned_git_command(
        checkout,
        ["rev-parse", "v1.9.0^{commit}"],
        "Kotlin baseline tag must be readable",
    );
    assert_eq!(baseline_revision.trim(), language_target.baseline_revision);

    let preview_revision = run_pinned_git_command(
        checkout,
        ["rev-parse", "v2.4.20-Beta1^{commit}"],
        "Kotlin preview tag must be readable",
    );
    assert_eq!(preview_revision.trim(), PREVIEW_TARGET_REVISION);
}

fn read_pinned_kotlin_source(checkout: &Path, revision: &str, source_path: &str) -> String {
    let object_name = format!("{revision}:{source_path}");
    run_pinned_git_command(
        checkout,
        ["show", object_name.as_str()],
        "pinned Kotlin source must be readable",
    )
}

fn run_pinned_git_command<const ARGUMENT_COUNT: usize>(
    checkout: &Path,
    arguments: [&str; ARGUMENT_COUNT],
    failure_message: &str,
) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(arguments)
        .output()
        .expect("git must be available for the Kotlin evolution audit");
    assert!(output.status.success(), "{failure_message}");
    String::from_utf8(output.stdout).expect("pinned Git output must be UTF-8")
}

fn assert_evolution_source_matches_checkout(
    checkout: &Path,
    source_ledger: &EvolutionSourceLedger,
    audit_items: &[EvolutionAuditItem],
) {
    let source = read_pinned_kotlin_source(
        checkout,
        &source_ledger.source_revision,
        &source_ledger.source_path,
    );
    let source_audit_items: Vec<&EvolutionAuditItem> = audit_items
        .iter()
        .filter(|audit_item| audit_item.release_line == source_ledger.release_line)
        .collect();
    assert_changelog_boundary_exists(source_ledger, &source);
    for audit_item in &source_audit_items {
        assert_evolution_audit_item_matches_source(audit_item, &source);
    }
    if source_ledger.audit_status == "complete" {
        assert_changelog_bullets_have_exact_audit_items(
            source_ledger,
            &source,
            &source_audit_items,
        );
    }
}

fn assert_changelog_boundary_exists(source_ledger: &EvolutionSourceLedger, source: &str) {
    let source_lines: Vec<&str> = source.lines().collect();
    let start_index = source_lines
        .iter()
        .position(|line| line.trim() == source_ledger.source_start_heading)
        .unwrap_or_else(|| {
            panic!(
                "release {} start heading is missing: {}",
                source_ledger.release_line, source_ledger.source_start_heading
            )
        });
    if let Some(end_heading) = source_ledger.source_end_heading.as_deref() {
        let end_index = source_lines
            .iter()
            .position(|line| line.trim() == end_heading)
            .unwrap_or_else(|| {
                panic!(
                    "release {} end heading is missing: {end_heading}",
                    source_ledger.release_line
                )
            });
        assert!(
            end_index > start_index,
            "release {} changelog boundary is reversed",
            source_ledger.release_line
        );
    }
}

fn assert_changelog_bullets_have_exact_audit_items(
    source_ledger: &EvolutionSourceLedger,
    source: &str,
    audit_items: &[&EvolutionAuditItem],
) {
    let bullet_lines = changelog_bullet_lines(source_ledger, source);
    let audit_item_lines: Vec<usize> = audit_items
        .iter()
        .map(|audit_item| audit_item.source_line_start)
        .collect();
    assert_eq!(
        audit_item_lines, bullet_lines,
        "release {} audit items must match changelog bullets in source order",
        source_ledger.release_line
    );
}

fn changelog_bullet_lines(source_ledger: &EvolutionSourceLedger, source: &str) -> Vec<usize> {
    let source_lines: Vec<&str> = source.lines().collect();
    let start_index = source_lines
        .iter()
        .position(|line| line.trim() == source_ledger.source_start_heading)
        .expect("changelog start heading was checked");
    let end_index = source_ledger
        .source_end_heading
        .as_deref()
        .map(|end_heading| {
            source_lines
                .iter()
                .position(|line| line.trim() == end_heading)
                .expect("changelog end heading was checked")
        })
        .unwrap_or(source_lines.len());

    source_lines[start_index..end_index]
        .iter()
        .enumerate()
        .filter_map(|(relative_index, line)| {
            line.starts_with("- ")
                .then_some(start_index + relative_index + 1)
        })
        .collect()
}

fn assert_evolution_audit_item_matches_source(audit_item: &EvolutionAuditItem, source: &str) {
    let source_lines: Vec<&str> = source.lines().collect();
    assert!(
        audit_item.source_line_end <= source_lines.len(),
        "{} cites line {} beyond {} lines",
        audit_item.id,
        audit_item.source_line_end,
        source_lines.len()
    );
    assert!(
        source_lines[audit_item.source_line_start - 1].starts_with("- "),
        "{} must start its citation on a changelog bullet",
        audit_item.id
    );
    let release_heading = preceding_heading(&source_lines, audit_item.source_line_start, "## ");
    assert_eq!(
        release_heading, audit_item.source_heading,
        "{} cites the wrong release heading",
        audit_item.id
    );
    let source_category = preceding_heading(&source_lines, audit_item.source_line_start, "### ");
    assert_eq!(
        source_category, audit_item.source_category,
        "{} cites the wrong changelog category",
        audit_item.id,
    );
    let cited_source =
        source_lines[audit_item.source_line_start - 1..audit_item.source_line_end].join("\n");
    assert!(
        cited_source.contains(audit_item.source_category.as_str()),
        "{} citation does not include category {:?}",
        audit_item.id,
        audit_item.source_category
    );
    if let Some(issue) = audit_item.issue.as_deref() {
        assert!(
            cited_source.contains(issue),
            "{} citation does not include issue {issue}",
            audit_item.id
        );
    }
}

fn preceding_heading(source_lines: &[&str], line_number: usize, prefix: &str) -> String {
    source_lines[..line_number - 1]
        .iter()
        .rev()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("line {line_number} has no preceding {prefix} heading"))
        .trim()
        .to_string()
}

fn assert_kotlin_citation_matches_checkout(
    checkout: &Path,
    citation: &KotlinCitation,
    item_id: &str,
) {
    let source = read_pinned_kotlin_source(checkout, &citation.revision, &citation.source_path);
    assert_pinned_source_citation(
        &source,
        citation.source_heading.as_deref(),
        citation.source_anchor.as_deref(),
        citation.source_line_start,
        citation.source_line_end,
        item_id,
        &citation.source_path,
    );
}

fn assert_kotlin_web_site_revision(checkout: &Path, documentation: &DocumentationIdentity) {
    let revision_object = format!("{}^{{commit}}", documentation.revision);
    let revision = run_pinned_git_command(
        checkout,
        ["rev-parse", revision_object.as_str()],
        "kotlin-web-site revision must be readable",
    );
    assert_eq!(revision.trim(), documentation.revision);
}

fn assert_documentation_topics_match_pinned_toc(checkout: &Path, matrix: &CoverageMatrix) {
    let toc_source = read_pinned_documentation_source(
        checkout,
        &matrix.documentation.revision,
        &matrix.documentation.toc_path,
    );
    let pinned_topic_paths = language_guide_topic_paths(&toc_source, &matrix.documentation);
    let ledger_topic_paths: Vec<&str> = matrix
        .documentation_topics
        .iter()
        .map(|topic| topic.source_path.as_str())
        .collect();
    assert_eq!(
        ledger_topic_paths, pinned_topic_paths,
        "documentation ledgers must exactly match the pinned Language guide TOC"
    );

    for topic in &matrix.documentation_topics {
        read_pinned_documentation_source(
            checkout,
            &matrix.documentation.revision,
            &topic.source_path,
        );
    }
}

fn read_pinned_documentation_source(checkout: &Path, revision: &str, source_path: &str) -> String {
    let object_name = format!("{revision}:{source_path}");
    run_pinned_git_command(
        checkout,
        ["show", object_name.as_str()],
        "pinned kotlin-web-site source must be readable",
    )
}

fn language_guide_topic_paths(
    toc_source: &str,
    documentation: &DocumentationIdentity,
) -> Vec<String> {
    let language_guide_marker = format!("toc-title=\"{}\"", documentation.toc_title);
    let mut inside_language_guide = false;
    let mut nesting_depth = 0usize;
    let mut topic_paths = Vec::new();

    for line in toc_source.lines() {
        let trimmed_line = line.trim();
        if !inside_language_guide {
            let starts_toc_element = trimmed_line.starts_with("<toc-element ");
            let is_language_guide = trimmed_line.contains(&language_guide_marker);
            if starts_toc_element && is_language_guide {
                inside_language_guide = true;
                nesting_depth = 1;
            }
            continue;
        }

        if trimmed_line.starts_with("<toc-element ") {
            if let Some(topic) = xml_attribute(trimmed_line, "topic") {
                topic_paths.push(format!("{}/{topic}", documentation.source_root));
            }
            if !trimmed_line.ends_with("/>") {
                nesting_depth += 1;
            }
        }
        if trimmed_line == "</toc-element>" {
            nesting_depth -= 1;
            if nesting_depth == 0 {
                break;
            }
        }
    }

    assert!(
        inside_language_guide,
        "Language guide TOC subtree is missing"
    );
    assert_eq!(topic_paths.len(), documentation.topic_count);
    topic_paths
}

fn xml_attribute<'line>(line: &'line str, attribute: &str) -> Option<&'line str> {
    let attribute_prefix = format!("{attribute}=\"");
    let value_start = line.find(&attribute_prefix)? + attribute_prefix.len();
    let value_suffix = &line[value_start..];
    let value_end = value_suffix.find('"')?;
    Some(&value_suffix[..value_end])
}

fn assert_documentation_citations_match_checkout(checkout: &Path, matrix: &CoverageMatrix) {
    for audit_item in &matrix.documentation_audit_items {
        let source = read_pinned_documentation_source(
            checkout,
            &matrix.documentation.revision,
            &audit_item.source_path,
        );
        assert_pinned_source_citation(
            &source,
            audit_item.source_heading.as_deref(),
            audit_item.source_anchor.as_deref(),
            audit_item.source_line_start,
            audit_item.source_line_end,
            &audit_item.id,
            &audit_item.source_path,
        );
    }

    for requirement in &matrix.requirements {
        for citation in &requirement.documentation_citations {
            assert_documentation_citation_matches_checkout(checkout, citation, &requirement.id);
        }
        for citation in &requirement.migration_citations {
            assert_documentation_citation_matches_checkout(checkout, citation, &requirement.id);
        }
    }
    for requirement in &matrix.evolution_requirements {
        for citation in &requirement.documentation_citations {
            assert_documentation_citation_matches_checkout(checkout, citation, &requirement.id);
        }
        for citation in &requirement.migration_citations {
            assert_documentation_citation_matches_checkout(checkout, citation, &requirement.id);
        }
    }
}

fn assert_documentation_citation_matches_checkout(
    checkout: &Path,
    citation: &DocumentationCitation,
    requirement_id: &str,
) {
    let source =
        read_pinned_documentation_source(checkout, &citation.revision, &citation.source_path);
    assert_pinned_source_citation(
        &source,
        citation.source_heading.as_deref(),
        citation.source_anchor.as_deref(),
        citation.source_line_start,
        citation.source_line_end,
        requirement_id,
        &citation.source_path,
    );
}

fn assert_pinned_source_citation(
    source: &str,
    source_heading: Option<&str>,
    source_anchor: Option<&str>,
    source_line_start: usize,
    source_line_end: usize,
    item_id: &str,
    source_path: &str,
) {
    let source_lines: Vec<&str> = source.lines().collect();
    assert!(source_line_start > 0);
    assert!(source_line_end >= source_line_start);
    assert!(
        source_line_end <= source_lines.len(),
        "{item_id} cites line {source_line_end} beyond {} lines in {source_path}",
        source_lines.len()
    );
    if let Some(source_heading) = source_heading {
        assert!(
            source_lines
                .iter()
                .any(|line| line.trim() == source_heading.trim()),
            "{item_id} cites missing heading {source_heading:?} in {source_path}"
        );
    }
    if let Some(source_anchor) = source_anchor {
        assert!(
            source.contains(source_anchor),
            "{item_id} cites missing anchor {source_anchor} in {source_path}"
        );
    }
}

fn assert_source_file_exists(normative_root: &Path, source_file: &str) {
    assert!(
        normative_root.join(source_file).is_file(),
        "normative source file is missing: {source_file}"
    );
}

fn requirement_primary_citation(requirement: &Requirement) -> SourceCitation {
    SourceCitation {
        source_file: requirement.source_file.clone(),
        source_heading: requirement.source_heading.clone(),
        source_anchor: requirement.source_anchor.clone(),
        source_line_start: requirement.source_line_start,
        source_line_end: requirement.source_line_end,
    }
}

fn assert_citation_matches_checkout(
    normative_root: &Path,
    citation: &SourceCitation,
    requirement_id: &str,
) {
    let source_path = normative_root.join(&citation.source_file);
    let source = std::fs::read_to_string(&source_path).unwrap_or_else(|error| {
        panic!(
            "cannot read {} for {requirement_id}: {error}",
            source_path.display()
        )
    });
    let line_count = source.lines().count();
    assert!(citation.source_line_start > 0);
    assert!(citation.source_line_end >= citation.source_line_start);
    assert!(
        citation.source_line_end <= line_count,
        "{requirement_id} cites line {} beyond {} lines in {}",
        citation.source_line_end,
        line_count,
        citation.source_file
    );
    if let Some(source_heading) = citation.source_heading.as_deref() {
        assert!(
            source
                .lines()
                .any(|line| line.trim() == source_heading.trim()),
            "{requirement_id} cites missing heading {source_heading:?}"
        );
    }
    if let Some(source_anchor) = citation.source_anchor.as_deref() {
        assert!(
            source.contains(source_anchor),
            "{requirement_id} cites missing anchor {source_anchor}"
        );
    }
}

fn assert_included_syntax_grammar_matches_authoring_source(
    checkout: &Path,
    requirements: &[Requirement],
) {
    let parser_grammar_path = checkout.join("grammar/src/main/antlr/KotlinParser.g4");
    let parser_grammar = std::fs::read_to_string(&parser_grammar_path).unwrap_or_else(|error| {
        panic!(
            "cannot read included syntax-grammar authoring source {}: {error}",
            parser_grammar_path.display()
        )
    });
    let parser_rules = parser_rule_names(&parser_grammar);

    let cited_rules: Vec<&str> = requirements
        .iter()
        .filter(|requirement| {
            requirement
                .previous_ids
                .iter()
                .any(|previous_id| previous_id.starts_with("KS-1.3-"))
        })
        .map(|requirement| {
            requirement
                .oracle
                .split_once("production ")
                .and_then(|(_, production)| production.split(';').next())
                .unwrap_or_else(|| {
                    panic!(
                        "{} must name its pinned KotlinParser.g4 production",
                        requirement.id
                    )
                })
        })
        .collect();

    assert_eq!(cited_rules, parser_rules);
}

fn parser_rule_names(parser_grammar: &str) -> Vec<&str> {
    let lines: Vec<&str> = parser_grammar.lines().collect();
    lines
        .windows(2)
        .filter_map(|line_pair| {
            let candidate = line_pair[0];
            let production = line_pair[1];
            let is_rule_name = !candidate.is_empty()
                && candidate
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
                && candidate
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_ascii_lowercase());
            let starts_production = production.trim_start().starts_with(':');
            (is_rule_name && starts_production).then_some(candidate)
        })
        .collect()
}

fn assert_nonempty(value: Option<&str>, item_id: &str, field_name: &str) {
    assert!(
        value.is_some_and(|text| !text.trim().is_empty()),
        "{item_id} must provide {field_name}"
    );
}

fn specification_test_source() -> String {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/kotlin_spec_tests");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(directory)
        .expect("specification test module directory must exist")
        .map(|entry| {
            entry
                .expect("specification test directory entry must be readable")
                .path()
        })
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect();
    paths.sort();

    paths
        .into_iter()
        .map(|path| {
            std::fs::read_to_string(path).expect("specification test source must be readable")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
