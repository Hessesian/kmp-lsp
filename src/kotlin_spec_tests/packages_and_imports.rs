use super::{assert_source_has_syntax_error, assert_source_parses};

#[test]
fn ks_10_001_file_accepts_zero_or_one_simple_or_qualified_package_header() {
    assert_source_parses("val rootSpec = 1\n");
    assert_source_parses("package sample\nval simpleSpec = 1\n");
    assert_source_parses("package sample.feature;\nval qualifiedSpec = 1\n");
}

#[test]
fn ks_10_002_file_cannot_have_multiple_package_headers() {
    assert_source_parses("package sample\nval validSpec = 1\n");
    assert_source_has_syntax_error(
        "package first.sample\npackage second.sample\nval invalidSpec = 1\n",
    );
}

#[test]
fn ks_10_1_001_import_directives_accept_regular_star_and_renaming_forms() {
    assert_source_parses(
        "package usage.sample\nimport source.sample.valueSpec\nimport source.sample.*\nimport source.sample.otherSpec as renamedSpec\nval resultSpec = renamedSpec\n",
    );
}

#[test]
#[ignore = "KS-10.1-002: kmp-lsp does not reject star imports from objects"]
fn ks_10_1_002_object_star_import_is_forbidden() {
    assert_source_parses(
        "package usage.sample\nimport source.sample.ContainerSpec.memberSpec\nval validSpec = memberSpec\n",
    );
    assert_source_has_syntax_error(
        "package usage.sample\nimport source.sample.ContainerSpec.*\nval invalidSpec = memberSpec\n",
    );
}
