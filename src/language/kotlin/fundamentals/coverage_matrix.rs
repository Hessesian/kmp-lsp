use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

const COVERAGE_MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/kotlin_spec/coverage.toml"
));
const KOTLIN_SPECIFICATION_FRAGMENTS: [&str; 20] = [
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
const LANGUAGE_REQUIREMENTS_FRAGMENT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/kotlin_spec/coverage/kotlin.language.toml"
));
const SPECIFICATION_REPOSITORY: &str = "Kotlin/kotlin-spec";
const SPECIFICATION_REVISION: &str = "2f7aa0524ec27e788dfacd550f144809f2e0254c";
const NORMATIVE_ROOT: &str = "docs/src/md";
const LANGUAGE_TARGET_VERSION: &str = "2.4";
const LANGUAGE_TARGET_RELEASE: &str = "v2.4.10";
const LANGUAGE_TARGET_REVISION: &str = "5687445832cd835b4509b9fbc264cdf1a8201093";
const DOCUMENTATION_REPOSITORY: &str = "JetBrains/kotlin-web-site";
const DOCUMENTATION_REVISION: &str = "7c270c2ac320fbee4884927f056b89d32f2a002e";
const DOCUMENTATION_SOURCE_ROOT: &str = "docs/topics";
const DOCUMENTATION_TOC_PATH: &str = "docs/kr.tree";
const DOCUMENTATION_TOC_TITLE: &str = "Language guide";
const DOCUMENTATION_TOPIC_COUNT: usize = 49;
const KOTLIN_SPECIFICATION_SOURCES: [&str; 20] = [
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
    specification: SpecificationIdentity,
    language_target: LanguageTarget,
    coverage: CoverageSummary,
    language_requirements_ledger: LanguageRequirementsLedger,
    documentation: DocumentationIdentity,
    documentation_topics: Vec<DocumentationTopic>,
    sources: Vec<SourceLedger>,
    specification_requirements: Vec<SpecificationRequirement>,
    language_requirements: Vec<LanguageRequirement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoverageManifest {
    specification: SpecificationIdentity,
    language_target: LanguageTarget,
    coverage: CoverageSummary,
    language_requirements: LanguageRequirementsLedger,
    documentation: DocumentationIdentity,
    documentation_topics: Vec<DocumentationTopic>,
    sources: Vec<SourceLedger>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpecificationRequirementFragment {
    #[serde(default)]
    requirements: Vec<SpecificationRequirement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LanguageRequirementFragment {
    #[serde(default)]
    requirements: Vec<LanguageRequirement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpecificationIdentity {
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
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct CoverageCounts {
    exact_active: usize,
    exact_ignored: usize,
    heuristic_active: usize,
    heuristic_ignored: usize,
    out_of_scope_excluded: usize,
}

impl CoverageCounts {
    fn total(self) -> usize {
        self.exact_active
            + self.exact_ignored
            + self.heuristic_active
            + self.heuristic_ignored
            + self.out_of_scope_excluded
    }

    fn combined_with(self, other: Self) -> Self {
        Self {
            exact_active: self.exact_active + other.exact_active,
            exact_ignored: self.exact_ignored + other.exact_ignored,
            heuristic_active: self.heuristic_active + other.heuristic_active,
            heuristic_ignored: self.heuristic_ignored + other.heuristic_ignored,
            out_of_scope_excluded: self.out_of_scope_excluded + other.out_of_scope_excluded,
        }
    }

    fn record(&mut self, requirement: RequirementView<'_>) {
        match (requirement.classification, requirement.status) {
            ("exact", "active") => self.exact_active += 1,
            ("exact", "ignored") => self.exact_ignored += 1,
            ("heuristic", "active") => self.heuristic_active += 1,
            ("heuristic", "ignored") => self.heuristic_ignored += 1,
            ("out-of-scope", "excluded") => self.out_of_scope_excluded += 1,
            _ => panic!(
                "{} has invalid classification/status {}/{}",
                requirement.requirement_id, requirement.classification, requirement.status
            ),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoverageSummary {
    requirement_count: usize,
    primary_test_count: usize,
    ignored_test_count: usize,
    #[serde(flatten)]
    counts: CoverageCounts,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LanguageRequirementsLedger {
    path: String,
    requirement_count: usize,
    #[serde(flatten)]
    counts: CoverageCounts,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentationIdentity {
    repository: String,
    revision: String,
    source_root: String,
    #[serde(rename = "toc_path")]
    table_of_contents_path: String,
    #[serde(rename = "toc_title")]
    table_of_contents_title: String,
    topic_count: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentationTopic {
    #[serde(rename = "toc_order")]
    table_of_contents_order: usize,
    source_path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceLedger {
    path: String,
    #[serde(flatten)]
    counts: CoverageCounts,
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
struct SpecificationRequirement {
    #[serde(rename = "id")]
    requirement_id: String,
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
    #[serde(default)]
    documentation_citations: Vec<DocumentationCitation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LanguageRequirement {
    #[serde(rename = "id")]
    requirement_id: String,
    maturity: String,
    required_compiler_flag: Option<String>,
    required_opt_in: Option<String>,
    statement: String,
    capabilities: Vec<String>,
    classification: String,
    status: String,
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
    compiler_citations: Vec<KotlinCitation>,
}

#[derive(Clone, Copy)]
struct RequirementView<'requirement> {
    requirement_id: &'requirement str,
    statement: &'requirement str,
    classification: &'requirement str,
    capabilities: &'requirement [String],
    status: &'requirement str,
    tests: &'requirement [String],
    fixture: Option<&'requirement str>,
    sample_evidence: Option<&'requirement str>,
    oracle: &'requirement str,
    fallback_oracle: Option<&'requirement str>,
    ignore_reason: Option<&'requirement str>,
    observed_failure: Option<&'requirement str>,
    expected_behavior: Option<&'requirement str>,
    heuristic_limitations: Option<&'requirement str>,
    exclusion_kind: Option<&'requirement str>,
    exclusion_rationale: Option<&'requirement str>,
}

impl SpecificationRequirement {
    fn view(&self) -> RequirementView<'_> {
        RequirementView {
            requirement_id: &self.requirement_id,
            statement: &self.statement,
            classification: &self.classification,
            capabilities: &self.capabilities,
            status: &self.status,
            tests: &self.tests,
            fixture: self.fixture.as_deref(),
            sample_evidence: self.sample_evidence.as_deref(),
            oracle: &self.oracle,
            fallback_oracle: self.fallback_oracle.as_deref(),
            ignore_reason: self.ignore_reason.as_deref(),
            observed_failure: self.observed_failure.as_deref(),
            expected_behavior: self.expected_behavior.as_deref(),
            heuristic_limitations: self.heuristic_limitations.as_deref(),
            exclusion_kind: self.exclusion_kind.as_deref(),
            exclusion_rationale: self.exclusion_rationale.as_deref(),
        }
    }
}

impl LanguageRequirement {
    fn view(&self) -> RequirementView<'_> {
        RequirementView {
            requirement_id: &self.requirement_id,
            statement: &self.statement,
            classification: &self.classification,
            capabilities: &self.capabilities,
            status: &self.status,
            tests: &self.tests,
            fixture: self.fixture.as_deref(),
            sample_evidence: self.sample_evidence.as_deref(),
            oracle: &self.oracle,
            fallback_oracle: self.fallback_oracle.as_deref(),
            ignore_reason: self.ignore_reason.as_deref(),
            observed_failure: self.observed_failure.as_deref(),
            expected_behavior: self.expected_behavior.as_deref(),
            heuristic_limitations: self.heuristic_limitations.as_deref(),
            exclusion_kind: self.exclusion_kind.as_deref(),
            exclusion_rationale: self.exclusion_rationale.as_deref(),
        }
    }
}

#[test]
fn coverage_matrix_has_valid_traceability_entries() {
    let matrix = parse_coverage_matrix();
    assert_matrix_identities(&matrix);
    let source_ledgers = assert_source_ledgers(&matrix);
    let test_source = specification_test_source();
    let mut requirement_ids = HashSet::new();
    let mut primary_tests = HashSet::new();
    let mut ignored_tests = HashSet::new();

    for requirement in &matrix.specification_requirements {
        assert_unique_requirement_id(&mut requirement_ids, &requirement.requirement_id);
        assert_specification_requirement(requirement, &source_ledgers, &matrix);
        assert_primary_tests(requirement.view(), &test_source, &mut primary_tests);
        record_ignored_tests(requirement.view(), &mut ignored_tests);
    }

    for requirement in &matrix.language_requirements {
        assert_unique_requirement_id(&mut requirement_ids, &requirement.requirement_id);
        assert_language_requirement(requirement, &matrix);
        assert_primary_tests(requirement.view(), &test_source, &mut primary_tests);
        record_ignored_tests(requirement.view(), &mut ignored_tests);
    }

    assert_coverage_counts(&matrix);
    assert_eq!(requirement_ids.len(), matrix.coverage.requirement_count);
    assert_eq!(primary_tests.len(), matrix.coverage.primary_test_count);
    assert_eq!(ignored_tests.len(), matrix.coverage.ignored_test_count);
    assert_all_primary_tests_are_traced(&test_source, &primary_tests);
}

#[test]
#[ignore = "requires the read-only kotlin-spec authoring checkout"]
fn coverage_matrix_matches_pinned_kotlin_spec_checkout() {
    let matrix = parse_coverage_matrix();
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("kotlin-spec");
    assert_checkout_revision(&checkout, &matrix.specification.revision, "kotlin-spec");

    let normative_root = checkout.join(&matrix.specification.normative_root);
    for source_ledger in &matrix.sources {
        assert_source_file_exists(&normative_root, &source_ledger.path);
    }
    for requirement in &matrix.specification_requirements {
        let citation = specification_requirement_citation(requirement);
        assert_specification_citation_matches_checkout(
            &normative_root,
            &citation,
            &requirement.requirement_id,
        );
        for related_source in &requirement.related_sources {
            assert_specification_citation_matches_checkout(
                &normative_root,
                related_source,
                &requirement.requirement_id,
            );
        }
    }
    assert_included_syntax_grammar_matches_authoring_source(
        &checkout,
        &matrix.specification_requirements,
    );
}

#[test]
#[ignore = "requires the read-only Kotlin authoring checkout"]
fn language_requirements_match_pinned_kotlin_checkout() {
    let matrix = parse_coverage_matrix();
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("kotlin");
    assert_kotlin_target_revision(&checkout, &matrix.language_target);

    for requirement in &matrix.language_requirements {
        for citation in &requirement.compiler_citations {
            assert_kotlin_citation_matches_checkout(
                &checkout,
                citation,
                &requirement.requirement_id,
            );
        }
    }
}

#[test]
#[ignore = "requires the read-only kotlin-web-site authoring checkout"]
fn documentation_citations_match_pinned_kotlin_web_site_checkout() {
    let matrix = parse_coverage_matrix();
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("kotlin-web-site");
    assert_checkout_revision(&checkout, &matrix.documentation.revision, "kotlin-web-site");
    assert_documentation_topics_match_pinned_toc(&checkout, &matrix);
    assert_documentation_citations_match_checkout(&checkout, &matrix);
}

fn parse_coverage_matrix() -> CoverageMatrix {
    let manifest: CoverageManifest =
        toml::from_str(COVERAGE_MANIFEST).expect("coverage.toml must be valid TOML");
    assert_eq!(
        manifest.sources.len(),
        KOTLIN_SPECIFICATION_FRAGMENTS.len(),
        "coverage manifest and Kotlin/Core fragment counts differ"
    );

    let mut specification_requirements = Vec::new();
    for ((source_ledger, expected_path), fragment_document) in manifest
        .sources
        .iter()
        .zip(KOTLIN_SPECIFICATION_SOURCES)
        .zip(KOTLIN_SPECIFICATION_FRAGMENTS)
    {
        assert_eq!(
            source_ledger.path, expected_path,
            "source ledger order changed"
        );
        let fragment: SpecificationRequirementFragment = toml::from_str(fragment_document)
            .unwrap_or_else(|error| {
                panic!("coverage fragment for {expected_path} is invalid: {error}")
            });
        for requirement in &fragment.requirements {
            assert_eq!(
                requirement.source_file, source_ledger.path,
                "{} is stored outside its source fragment",
                requirement.requirement_id
            );
        }
        specification_requirements.extend(fragment.requirements);
    }

    let language_fragment: LanguageRequirementFragment =
        toml::from_str(LANGUAGE_REQUIREMENTS_FRAGMENT)
            .expect("kotlin.language.toml must be valid TOML");

    CoverageMatrix {
        specification: manifest.specification,
        language_target: manifest.language_target,
        coverage: manifest.coverage,
        language_requirements_ledger: manifest.language_requirements,
        documentation: manifest.documentation,
        documentation_topics: manifest.documentation_topics,
        sources: manifest.sources,
        specification_requirements,
        language_requirements: language_fragment.requirements,
    }
}

fn assert_matrix_identities(matrix: &CoverageMatrix) {
    assert_eq!(matrix.specification.version, "1.9-rfc+0.1");
    assert_eq!(matrix.specification.repository, SPECIFICATION_REPOSITORY);
    assert_eq!(matrix.specification.revision, SPECIFICATION_REVISION);
    assert_eq!(matrix.specification.normative_root, NORMATIVE_ROOT);
    assert_eq!(
        matrix.language_target.language_version,
        LANGUAGE_TARGET_VERSION
    );
    assert_eq!(
        matrix.language_target.compiler_release,
        LANGUAGE_TARGET_RELEASE
    );
    assert_eq!(
        matrix.language_target.target_revision,
        LANGUAGE_TARGET_REVISION
    );
    assert_eq!(
        matrix.language_requirements_ledger.path,
        "kotlin.language.toml"
    );
    assert_eq!(matrix.documentation.repository, DOCUMENTATION_REPOSITORY);
    assert_eq!(matrix.documentation.revision, DOCUMENTATION_REVISION);
    assert_eq!(matrix.documentation.source_root, DOCUMENTATION_SOURCE_ROOT);
    assert_eq!(
        matrix.documentation.table_of_contents_path,
        DOCUMENTATION_TOC_PATH
    );
    assert_eq!(
        matrix.documentation.table_of_contents_title,
        DOCUMENTATION_TOC_TITLE
    );
    assert_eq!(matrix.documentation.topic_count, DOCUMENTATION_TOPIC_COUNT);
    assert_documentation_topics(matrix);
}

fn assert_documentation_topics(matrix: &CoverageMatrix) {
    assert_eq!(
        matrix.documentation_topics.len(),
        matrix.documentation.topic_count
    );
    let mut source_paths = HashSet::new();
    for (topic_index, topic) in matrix.documentation_topics.iter().enumerate() {
        assert_eq!(topic.table_of_contents_order, topic_index + 1);
        assert!(
            source_paths.insert(topic.source_path.as_str()),
            "duplicate documentation topic {}",
            topic.source_path
        );
        assert!(topic.source_path.starts_with(DOCUMENTATION_SOURCE_ROOT));
    }
}

fn assert_source_ledgers(matrix: &CoverageMatrix) -> HashMap<&str, &SourceLedger> {
    assert_eq!(matrix.sources.len(), KOTLIN_SPECIFICATION_SOURCES.len());
    let mut source_ledgers = HashMap::new();

    for (source_ledger, expected_path) in matrix.sources.iter().zip(KOTLIN_SPECIFICATION_SOURCES) {
        assert_eq!(source_ledger.path, expected_path);
        assert!(
            source_ledgers
                .insert(source_ledger.path.as_str(), source_ledger)
                .is_none(),
            "duplicate source ledger {}",
            source_ledger.path
        );
        let actual_counts = counts_for_specification_source(
            &source_ledger.path,
            &matrix.specification_requirements,
        );
        assert_eq!(
            source_ledger.counts, actual_counts,
            "source ledger counts differ for {}",
            source_ledger.path
        );
    }

    source_ledgers
}

fn assert_coverage_counts(matrix: &CoverageMatrix) {
    let specification_counts =
        counts_for_specification_requirements(&matrix.specification_requirements);
    let language_counts = counts_for_language_requirements(&matrix.language_requirements);
    assert_eq!(
        matrix.language_requirements_ledger.requirement_count,
        matrix.language_requirements.len()
    );
    assert_eq!(matrix.language_requirements_ledger.counts, language_counts);

    let combined_counts = specification_counts.combined_with(language_counts);
    assert_eq!(matrix.coverage.counts, combined_counts);
    assert_eq!(matrix.coverage.requirement_count, combined_counts.total());
}

fn counts_for_specification_source(
    source_path: &str,
    requirements: &[SpecificationRequirement],
) -> CoverageCounts {
    let mut counts = CoverageCounts::default();
    for requirement in requirements {
        if requirement.source_file == source_path {
            counts.record(requirement.view());
        }
    }
    counts
}

fn counts_for_specification_requirements(
    requirements: &[SpecificationRequirement],
) -> CoverageCounts {
    let mut counts = CoverageCounts::default();
    for requirement in requirements {
        counts.record(requirement.view());
    }
    counts
}

fn counts_for_language_requirements(requirements: &[LanguageRequirement]) -> CoverageCounts {
    let mut counts = CoverageCounts::default();
    for requirement in requirements {
        counts.record(requirement.view());
    }
    counts
}

fn assert_unique_requirement_id<'requirement>(
    requirement_ids: &mut HashSet<&'requirement str>,
    requirement_id: &'requirement str,
) {
    assert!(
        requirement_ids.insert(requirement_id),
        "duplicate requirement ID {requirement_id}"
    );
}

fn assert_specification_requirement(
    requirement: &SpecificationRequirement,
    source_ledgers: &HashMap<&str, &SourceLedger>,
    matrix: &CoverageMatrix,
) {
    let requirement_view = requirement.view();
    assert_requirement_metadata(requirement_view);
    assert!(
        source_ledgers.contains_key(requirement.source_file.as_str()),
        "{} cites a source outside the Kotlin/Core ledger",
        requirement.requirement_id
    );
    assert_source_location(
        requirement.source_heading.as_deref(),
        requirement.source_anchor.as_deref(),
        requirement.source_line_start,
        requirement.source_line_end,
        &requirement.requirement_id,
    );
    assert_specification_requirement_id(requirement);
    assert!(
        !requirement.oracle.to_ascii_lowercase().contains("pdf"),
        "{} retains a PDF oracle",
        requirement.requirement_id
    );
    for related_source in &requirement.related_sources {
        assert!(source_ledgers.contains_key(related_source.source_file.as_str()));
        assert_source_location(
            related_source.source_heading.as_deref(),
            related_source.source_anchor.as_deref(),
            related_source.source_line_start,
            related_source.source_line_end,
            &requirement.requirement_id,
        );
    }
    assert_documentation_citations(
        &requirement.documentation_citations,
        &requirement.requirement_id,
        matrix,
    );
    for duplicate in &requirement.duplicates {
        assert!(!requirement.tests.contains(duplicate));
    }
}

fn assert_language_requirement(requirement: &LanguageRequirement, matrix: &CoverageMatrix) {
    assert_requirement_metadata(requirement.view());
    assert_language_requirement_id(&requirement.requirement_id);
    assert_language_maturity(requirement);
    assert!(
        !requirement.compiler_citations.is_empty(),
        "{} must cite pinned Kotlin 2.4 compiler evidence",
        requirement.requirement_id
    );
    for citation in &requirement.compiler_citations {
        assert_eq!(citation.revision, LANGUAGE_TARGET_REVISION);
        assert_source_location(
            citation.source_heading.as_deref(),
            citation.source_anchor.as_deref(),
            citation.source_line_start,
            citation.source_line_end,
            &requirement.requirement_id,
        );
        assert!(!citation.source_path.trim().is_empty());
    }
    assert_documentation_citations(
        &requirement.documentation_citations,
        &requirement.requirement_id,
        matrix,
    );
}

fn assert_requirement_metadata(requirement: RequirementView<'_>) {
    assert_nonempty(requirement.requirement_id, "requirement ID");
    assert_nonempty(requirement.statement, "statement");
    assert!(!requirement.capabilities.is_empty());
    assert_nonempty(requirement.oracle, "oracle");
    if let Some(fallback_oracle) = requirement.fallback_oracle {
        assert!(
            !fallback_oracle.trim_start().starts_with("Not used"),
            "{} must omit unused fallback metadata",
            requirement.requirement_id
        );
    }

    match requirement.classification {
        "exact" | "heuristic" => assert_testable_requirement(requirement),
        "out-of-scope" => assert_excluded_requirement(requirement),
        classification => panic!(
            "{} has invalid classification {classification}",
            requirement.requirement_id
        ),
    }
}

fn assert_testable_requirement(requirement: RequirementView<'_>) {
    assert!(matches!(requirement.status, "active" | "ignored"));
    assert!(!requirement.tests.is_empty());
    assert_optional_nonempty(requirement.fixture, requirement.requirement_id, "fixture");
    assert_optional_nonempty(
        requirement.sample_evidence,
        requirement.requirement_id,
        "sample evidence",
    );
    assert!(requirement.exclusion_kind.is_none());
    assert!(requirement.exclusion_rationale.is_none());

    if requirement.classification == "heuristic" {
        assert_optional_nonempty(
            requirement.heuristic_limitations,
            requirement.requirement_id,
            "heuristic limitations",
        );
    } else {
        assert!(requirement.heuristic_limitations.is_none());
    }

    if requirement.status == "ignored" {
        assert_optional_nonempty(
            requirement.ignore_reason,
            requirement.requirement_id,
            "ignore reason",
        );
        assert_optional_nonempty(
            requirement.observed_failure,
            requirement.requirement_id,
            "observed failure",
        );
        assert_optional_nonempty(
            requirement.expected_behavior,
            requirement.requirement_id,
            "expected behavior",
        );
    } else {
        assert!(requirement.ignore_reason.is_none());
        assert!(requirement.observed_failure.is_none());
        assert!(requirement.expected_behavior.is_none());
    }
}

fn assert_excluded_requirement(requirement: RequirementView<'_>) {
    assert_eq!(requirement.status, "excluded");
    assert!(requirement.tests.is_empty());
    assert!(requirement.fixture.is_none());
    assert!(requirement.sample_evidence.is_none());
    assert!(requirement.ignore_reason.is_none());
    assert!(requirement.observed_failure.is_none());
    assert!(requirement.expected_behavior.is_none());
    assert!(requirement.heuristic_limitations.is_none());
    assert!(matches!(
        requirement.exclusion_kind,
        Some(
            "compiler-semantics"
                | "runtime"
                | "platform-defined"
                | "standard-library"
                | "unspecified"
        )
    ));
    assert_optional_nonempty(
        requirement.exclusion_rationale,
        requirement.requirement_id,
        "exclusion rationale",
    );
}

fn assert_specification_requirement_id(requirement: &SpecificationRequirement) {
    let source_stem = Path::new(&requirement.source_file)
        .file_stem()
        .and_then(|file_stem| file_stem.to_str())
        .expect("Kotlin/Core source path must have a UTF-8 file stem")
        .to_ascii_uppercase();
    let expected_prefix = format!("KS-{source_stem}-");
    let ordinal = requirement
        .requirement_id
        .strip_prefix(&expected_prefix)
        .unwrap_or_else(|| {
            panic!(
                "{} must start with {expected_prefix}",
                requirement.requirement_id
            )
        });
    assert_four_digit_ordinal(ordinal, &requirement.requirement_id);
}

fn assert_language_requirement_id(requirement_id: &str) {
    let components: Vec<&str> = requirement_id.split('-').collect();
    assert_eq!(
        components.len(),
        4,
        "{requirement_id} must use KL-<major>-<minor>-<ordinal>"
    );
    assert_eq!(components[0], "KL");
    assert!(components[1]
        .chars()
        .all(|character| character.is_ascii_digit()));
    assert!(components[2]
        .chars()
        .all(|character| character.is_ascii_digit()));
    assert_four_digit_ordinal(components[3], requirement_id);
}

fn assert_four_digit_ordinal(ordinal: &str, requirement_id: &str) {
    assert_eq!(
        ordinal.len(),
        4,
        "{requirement_id} must use a four-digit ordinal"
    );
    assert!(ordinal.chars().all(|character| character.is_ascii_digit()));
}

fn assert_language_maturity(requirement: &LanguageRequirement) {
    assert!(matches!(
        requirement.maturity.as_str(),
        "preview" | "experimental" | "beta" | "stable"
    ));
    if requirement.maturity == "stable" {
        assert!(requirement.required_compiler_flag.is_none());
        assert!(requirement.required_opt_in.is_none());
        return;
    }

    let has_compiler_flag = requirement
        .required_compiler_flag
        .as_deref()
        .is_some_and(|compiler_flag| !compiler_flag.trim().is_empty());
    let has_opt_in = requirement
        .required_opt_in
        .as_deref()
        .is_some_and(|opt_in| !opt_in.trim().is_empty());
    assert!(
        has_compiler_flag || has_opt_in,
        "{} must name its Kotlin 2.4 feature gate",
        requirement.requirement_id
    );
}

fn assert_documentation_citations(
    citations: &[DocumentationCitation],
    requirement_id: &str,
    matrix: &CoverageMatrix,
) {
    let topic_paths: HashSet<&str> = matrix
        .documentation_topics
        .iter()
        .map(|topic| topic.source_path.as_str())
        .collect();
    for citation in citations {
        assert_eq!(citation.repository, DOCUMENTATION_REPOSITORY);
        assert_eq!(citation.revision, DOCUMENTATION_REVISION);
        assert!(
            topic_paths.contains(citation.source_path.as_str()),
            "{requirement_id} cites a page outside the Language guide: {}",
            citation.source_path
        );
        assert_source_location(
            citation.source_heading.as_deref(),
            citation.source_anchor.as_deref(),
            citation.source_line_start,
            citation.source_line_end,
            requirement_id,
        );
    }
}

fn assert_source_location(
    source_heading: Option<&str>,
    source_anchor: Option<&str>,
    source_line_start: usize,
    source_line_end: usize,
    requirement_id: &str,
) {
    assert!(
        source_heading.is_some_and(|heading| !heading.trim().is_empty())
            || source_anchor.is_some_and(|anchor| !anchor.trim().is_empty()),
        "{requirement_id} must cite a source heading or anchor"
    );
    assert!(source_line_start > 0);
    assert!(source_line_end >= source_line_start);
}

fn assert_primary_tests(
    requirement: RequirementView<'_>,
    test_source: &str,
    primary_tests: &mut HashSet<String>,
) {
    let expected_prefix = requirement
        .requirement_id
        .to_ascii_lowercase()
        .replace('-', "_");
    for test_name in requirement.tests {
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
            requirement.requirement_id
        );
        assert_test_status(requirement, test_name, test_source);
    }
}

fn assert_test_status(requirement: RequirementView<'_>, test_name: &str, test_source: &str) {
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
        requirement.status == "ignored",
        "test {test_name} ignore annotation differs from {} status {}",
        requirement.requirement_id,
        requirement.status
    );
}

fn record_ignored_tests(requirement: RequirementView<'_>, ignored_tests: &mut HashSet<String>) {
    if requirement.status != "ignored" {
        return;
    }
    for test_name in requirement.tests {
        ignored_tests.insert(test_name.clone());
    }
}

fn assert_all_primary_tests_are_traced(test_source: &str, primary_tests: &HashSet<String>) {
    for declaration_suffix in test_source.split("fn ").skip(1) {
        let Some(test_name) = declaration_suffix.split('(').next() else {
            continue;
        };
        if test_name.starts_with("ks_") || test_name.starts_with("kl_") {
            assert!(
                primary_tests.contains(test_name),
                "specification test {test_name} is not traced by the Kotlin 2.4 matrix"
            );
        }
    }
}

fn assert_checkout_revision(checkout: &Path, revision: &str, checkout_name: &str) {
    let revision_object = format!("{revision}^{{commit}}");
    let actual_revision = run_git_command(
        checkout,
        ["rev-parse", revision_object.as_str()],
        &format!("{checkout_name} revision must be readable"),
    );
    assert_eq!(actual_revision.trim(), revision);
}

fn assert_kotlin_target_revision(checkout: &Path, language_target: &LanguageTarget) {
    let release_object = format!("{}^{{commit}}", language_target.compiler_release);
    let target_revision = run_git_command(
        checkout,
        ["rev-parse", release_object.as_str()],
        "Kotlin target tag must be readable",
    );
    assert_eq!(target_revision.trim(), language_target.target_revision);
}

fn run_git_command<const ARGUMENT_COUNT: usize>(
    checkout: &Path,
    arguments: [&str; ARGUMENT_COUNT],
    failure_message: &str,
) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(arguments)
        .output()
        .expect("git must be available for pinned source verification");
    assert!(output.status.success(), "{failure_message}");
    String::from_utf8(output.stdout).expect("pinned Git output must be UTF-8")
}

fn assert_kotlin_citation_matches_checkout(
    checkout: &Path,
    citation: &KotlinCitation,
    requirement_id: &str,
) {
    let source = read_pinned_source(checkout, &citation.revision, &citation.source_path);
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

fn read_pinned_source(checkout: &Path, revision: &str, source_path: &str) -> String {
    let object_name = format!("{revision}:{source_path}");
    run_git_command(
        checkout,
        ["show", object_name.as_str()],
        "pinned source must be readable",
    )
}

fn assert_documentation_topics_match_pinned_toc(checkout: &Path, matrix: &CoverageMatrix) {
    let table_of_contents_source = read_pinned_source(
        checkout,
        &matrix.documentation.revision,
        &matrix.documentation.table_of_contents_path,
    );
    let pinned_topic_paths =
        language_guide_topic_paths(&table_of_contents_source, &matrix.documentation);
    let matrix_topic_paths: Vec<&str> = matrix
        .documentation_topics
        .iter()
        .map(|topic| topic.source_path.as_str())
        .collect();
    assert_eq!(
        matrix_topic_paths, pinned_topic_paths,
        "documentation topics must exactly match the pinned Language guide TOC"
    );
    for topic in &matrix.documentation_topics {
        read_pinned_source(checkout, &matrix.documentation.revision, &topic.source_path);
    }
}

fn language_guide_topic_paths(
    table_of_contents_source: &str,
    documentation: &DocumentationIdentity,
) -> Vec<String> {
    let language_guide_marker = format!("toc-title=\"{}\"", documentation.table_of_contents_title);
    let mut inside_language_guide = false;
    let mut nesting_depth = 0usize;
    let mut topic_paths = Vec::new();

    for line in table_of_contents_source.lines() {
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
    for requirement in &matrix.specification_requirements {
        for citation in &requirement.documentation_citations {
            assert_documentation_citation_matches_checkout(
                checkout,
                citation,
                &requirement.requirement_id,
            );
        }
    }
    for requirement in &matrix.language_requirements {
        for citation in &requirement.documentation_citations {
            assert_documentation_citation_matches_checkout(
                checkout,
                citation,
                &requirement.requirement_id,
            );
        }
    }
}

fn assert_documentation_citation_matches_checkout(
    checkout: &Path,
    citation: &DocumentationCitation,
    requirement_id: &str,
) {
    let source = read_pinned_source(checkout, &citation.revision, &citation.source_path);
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
    requirement_id: &str,
    source_path: &str,
) {
    let source_lines: Vec<&str> = source.lines().collect();
    assert!(source_line_start > 0);
    assert!(source_line_end >= source_line_start);
    assert!(
        source_line_end <= source_lines.len(),
        "{requirement_id} cites line {source_line_end} beyond {} lines in {source_path}",
        source_lines.len()
    );
    if let Some(source_heading) = source_heading {
        assert!(
            source_lines
                .iter()
                .any(|line| line.trim() == source_heading.trim()),
            "{requirement_id} cites missing heading {source_heading:?} in {source_path}"
        );
    }
    if let Some(source_anchor) = source_anchor {
        assert!(
            source.contains(source_anchor),
            "{requirement_id} cites missing anchor {source_anchor} in {source_path}"
        );
    }
}

fn assert_source_file_exists(normative_root: &Path, source_file: &str) {
    assert!(
        normative_root.join(source_file).is_file(),
        "normative source file is missing: {source_file}"
    );
}

fn specification_requirement_citation(requirement: &SpecificationRequirement) -> SourceCitation {
    SourceCitation {
        source_file: requirement.source_file.clone(),
        source_heading: requirement.source_heading.clone(),
        source_anchor: requirement.source_anchor.clone(),
        source_line_start: requirement.source_line_start,
        source_line_end: requirement.source_line_end,
    }
}

fn assert_specification_citation_matches_checkout(
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
    assert_pinned_source_citation(
        &source,
        citation.source_heading.as_deref(),
        citation.source_anchor.as_deref(),
        citation.source_line_start,
        citation.source_line_end,
        requirement_id,
        &citation.source_file,
    );
}

fn assert_included_syntax_grammar_matches_authoring_source(
    checkout: &Path,
    requirements: &[SpecificationRequirement],
) {
    let parser_grammar_path = checkout.join("grammar/src/main/antlr/KotlinParser.g4");
    let parser_grammar = std::fs::read_to_string(&parser_grammar_path).unwrap_or_else(|error| {
        panic!(
            "cannot read included syntax grammar {}: {error}",
            parser_grammar_path.display()
        )
    });
    let parser_rules = parser_rule_names(&parser_grammar);
    let cited_rules: Vec<&str> = requirements
        .iter()
        .filter_map(|requirement| {
            requirement
                .oracle
                .split_once("KotlinParser.g4 production ")
                .and_then(|(_, production)| production.split(';').next())
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

fn assert_nonempty(value: &str, field_name: &str) {
    assert!(!value.trim().is_empty(), "must provide {field_name}");
}

fn assert_optional_nonempty(value: Option<&str>, requirement_id: &str, field_name: &str) {
    assert!(
        value.is_some_and(|text| !text.trim().is_empty()),
        "{requirement_id} must provide {field_name}"
    );
}

fn specification_test_source() -> String {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/language/kotlin/fundamentals");
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
