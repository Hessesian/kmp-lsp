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
const SPECIFICATION_REPOSITORY: &str = "Kotlin/kotlin-spec";
const SPECIFICATION_REVISION: &str = "2f7aa0524ec27e788dfacd550f144809f2e0254c";
const NORMATIVE_ROOT: &str = "docs/src/md";
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
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoverageManifest {
    specification: Specification,
    sources: Vec<SourceLedger>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequirementFragment {
    #[serde(default)]
    requirements: Vec<Requirement>,
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
struct SourceCitation {
    source_file: String,
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
}

#[test]
fn coverage_matrix_has_valid_traceability_entries() {
    let matrix = parse_coverage_matrix();
    assert_specification_identity(&matrix.specification);
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

    for requirement_id in requirement_ids {
        assert!(
            !previous_ids.contains(requirement_id),
            "requirement ID {requirement_id} is both current and previous"
        );
    }

    assert_all_primary_tests_are_traced(&test_source, &primary_tests);
    for coverage_document in std::iter::once(COVERAGE_MANIFEST).chain(COVERAGE_FRAGMENTS) {
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

    CoverageMatrix {
        specification: manifest.specification,
        sources: manifest.sources,
        requirements,
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
        let count_index = match (
            requirement.classification.as_str(),
            requirement.status.as_str(),
        ) {
            ("exact", "active") => 0,
            ("exact", "ignored") => 1,
            ("heuristic", "active") => 2,
            ("heuristic", "ignored") => 3,
            ("out-of-scope", "excluded") => 4,
            _ => panic!(
                "{} has an invalid migrated classification/status",
                requirement.id
            ),
        };
        counts[count_index] += 1;
    }
    counts
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
    if let Some(fallback_oracle) = requirement.fallback_oracle.as_deref() {
        assert!(
            !fallback_oracle.trim_start().starts_with("Not used"),
            "migrated {} must omit unused fallback metadata",
            requirement.id
        );
    }
    assert_migrated_classification(requirement);
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
    assert!(
        matches!(
            requirement.exclusion_kind.as_deref(),
            Some(
                "compiler-semantics"
                    | "runtime"
                    | "platform-defined"
                    | "standard-library"
                    | "unspecified"
            )
        ),
        "{} must provide a valid exclusion kind",
        requirement.id
    );
    assert_nonempty(
        requirement.exclusion_rationale.as_deref(),
        &requirement.id,
        "exclusion rationale",
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
        "test {test_name} ignore annotation does not match status {} for {}",
        requirement.status,
        requirement.id
    );
}

fn assert_all_primary_tests_are_traced(test_source: &str, primary_tests: &HashSet<String>) {
    for declaration_suffix in test_source.split("fn ").skip(1) {
        let Some(test_name) = declaration_suffix.split('(').next() else {
            continue;
        };
        if test_name.starts_with("ks_") {
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
