use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const COVERAGE_MATRIX: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/kotlin_spec/coverage.toml"
));

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoverageMatrix {
    specification: Specification,
    requirements: Vec<Requirement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Specification {
    version: String,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Requirement {
    id: String,
    section: String,
    printed_page: u16,
    statement: String,
    classification: String,
    capabilities: Vec<String>,
    status: String,
    tests: Vec<String>,
    #[serde(default)]
    duplicates: Vec<String>,
    fixture: String,
    sample_evidence: String,
    oracle: String,
    fallback_oracle: String,
    ignore_reason: Option<String>,
    expected_behavior: Option<String>,
    heuristic_limitations: Option<String>,
    exclusion_rationale: Option<String>,
}

#[test]
fn coverage_matrix_has_valid_traceability_entries() {
    let matrix: CoverageMatrix =
        toml::from_str(COVERAGE_MATRIX).expect("coverage.toml must be valid TOML");

    assert_eq!(matrix.specification.version, "1.9-rfc+0.1");
    assert_eq!(matrix.specification.source, "kotlin-spec.pdf");
    assert!(!matrix.requirements.is_empty());

    let test_source = specification_test_source();
    let mut requirement_ids = HashSet::new();
    let mut primary_tests = HashSet::new();

    for requirement in &matrix.requirements {
        assert!(
            requirement_ids.insert(requirement.id.as_str()),
            "duplicate specification requirement ID: {}",
            requirement.id
        );
        assert_required_fields(requirement);
        assert_classification_and_status(requirement);

        for test_name in &requirement.tests {
            assert!(
                test_name.starts_with("ks_"),
                "primary specification test {test_name} must use the ks_ prefix"
            );
            assert!(
                primary_tests.insert(test_name.as_str()),
                "test {test_name} is primary evidence for more than one requirement"
            );
            assert!(
                test_source.contains(&format!("fn {test_name}(")),
                "test {test_name} named by {} does not exist in the specification suite",
                requirement.id
            );
            assert_test_status(requirement, test_name, &test_source);
        }

        for duplicate in &requirement.duplicates {
            assert!(
                !requirement.tests.contains(duplicate),
                "{} treats duplicate test {duplicate} as primary evidence",
                requirement.id
            );
        }
    }

    for declaration_suffix in test_source.split("fn ").skip(1) {
        let Some(test_name) = declaration_suffix.split('(').next() else {
            continue;
        };
        if test_name.starts_with("ks_") {
            assert!(
                primary_tests.contains(test_name),
                "specification test {test_name} is not primary evidence for a coverage.toml entry"
            );
        }
    }

    assert!(
        !COVERAGE_MATRIX.to_ascii_lowercase().contains("uncertain"),
        "coverage.toml must not contain uncertain entries"
    );
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

fn assert_required_fields(requirement: &Requirement) {
    assert!(!requirement.id.trim().is_empty());
    assert!(!requirement.section.trim().is_empty());
    assert!(requirement.printed_page > 0);
    assert!(!requirement.statement.trim().is_empty());
    assert!(!requirement.capabilities.is_empty());
    assert!(!requirement.fixture.trim().is_empty());
    assert!(!requirement.sample_evidence.trim().is_empty());
    assert!(!requirement.oracle.trim().is_empty());
    assert!(!requirement.fallback_oracle.trim().is_empty());
}

fn assert_classification_and_status(requirement: &Requirement) {
    match requirement.classification.as_str() {
        "exact" | "heuristic" => {
            assert!(
                matches!(requirement.status.as_str(), "active" | "ignored"),
                "{} has invalid testable status {}",
                requirement.id,
                requirement.status
            );
            assert!(
                !requirement.tests.is_empty(),
                "{} must name new specification-suite tests",
                requirement.id
            );
            assert!(
                requirement.exclusion_rationale.is_none(),
                "testable {} must not carry a compiler-only exclusion rationale",
                requirement.id
            );

            if requirement.classification == "heuristic" {
                assert_nonempty(
                    requirement.heuristic_limitations.as_deref(),
                    &requirement.id,
                    "heuristic limitations",
                );
            } else {
                assert!(
                    requirement.heuristic_limitations.is_none(),
                    "exact {} must not carry heuristic limitations",
                    requirement.id
                );
            }

            if requirement.status == "ignored" {
                assert_nonempty(
                    requirement.ignore_reason.as_deref(),
                    &requirement.id,
                    "ignore reason",
                );
                assert_nonempty(
                    requirement.expected_behavior.as_deref(),
                    &requirement.id,
                    "expected behavior",
                );
            } else {
                assert!(
                    requirement.ignore_reason.is_none() && requirement.expected_behavior.is_none(),
                    "active {} must not retain ignored-test metadata",
                    requirement.id
                );
            }
        }
        "compiler-only" => {
            assert_eq!(
                requirement.status, "not-applicable",
                "compiler-only {} must be not-applicable",
                requirement.id
            );
            assert!(
                requirement.tests.is_empty(),
                "compiler-only {} must not have an artificial test",
                requirement.id
            );
            assert_nonempty(
                requirement.exclusion_rationale.as_deref(),
                &requirement.id,
                "exclusion rationale",
            );
            assert!(
                requirement.ignore_reason.is_none()
                    && requirement.expected_behavior.is_none()
                    && requirement.heuristic_limitations.is_none(),
                "compiler-only {} must not carry testable-status metadata",
                requirement.id
            );
        }
        classification => panic!(
            "{} has invalid classification {classification}",
            requirement.id
        ),
    }
}

fn assert_nonempty(value: Option<&str>, requirement_id: &str, field_name: &str) {
    assert!(
        value.is_some_and(|text| !text.trim().is_empty()),
        "{requirement_id} must provide {field_name}"
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
