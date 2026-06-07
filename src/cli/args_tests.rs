//! Unit tests for `cli::args`.

use super::*;

fn parse(argv: &[&str]) -> Result<Option<CliArgs>, String> {
    // lexopt expects argv[0] to be the binary name; prepend it.
    let owned: Vec<std::ffi::OsString> = std::iter::once("kotlin-lsp".into())
        .chain(argv.iter().map(|s| (*s).into()))
        .collect();
    CliArgs::parse_from(lexopt::Parser::from_args(
        owned.iter().skip(1).map(|s| s.as_os_str()),
    ))
}

fn find_filters(args: &CliArgs) -> &ResultFilters {
    match &args.subcommand {
        Subcommand::Find { filters, .. } | Subcommand::Refs { filters, .. } => filters,
        other => panic!("expected find/refs, got {other:?}"),
    }
}

#[test]
fn find_with_no_filter_flags_yields_default_filters() {
    let args = parse(&["find", "Foo"]).unwrap().unwrap();
    let filters = find_filters(&args);
    assert!(!filters.relative);
    assert!(filters.limit.is_none());
    assert!(filters.module.is_none());
    assert!(filters.source_sets.is_empty());
}

#[test]
fn find_parses_relative_flag() {
    let args = parse(&["find", "Foo", "--relative"]).unwrap().unwrap();
    assert!(find_filters(&args).relative);
}

#[test]
fn find_parses_absolute_flag() {
    let args = parse(&["find", "Foo", "--absolute"]).unwrap().unwrap();
    // `--absolute` doesn't go through ResultFilters; it lives on CliArgs so the
    // run-time TTY-default resolver can see it. The filter stays relative=false.
    assert!(args.absolute);
    assert!(!find_filters(&args).relative);
}

#[test]
fn absolute_defaults_to_false() {
    let args = parse(&["find", "Foo"]).unwrap().unwrap();
    assert!(!args.absolute);
}

#[test]
fn find_parses_limit_flag() {
    let args = parse(&["find", "Foo", "--limit", "20"]).unwrap().unwrap();
    assert_eq!(find_filters(&args).limit, Some(20));
}

#[test]
fn find_rejects_non_numeric_limit() {
    let err = parse(&["find", "Foo", "--limit", "abc"]).unwrap_err();
    assert!(err.contains("--limit"), "got: {err}");
}

#[test]
fn find_parses_module_filter() {
    let args = parse(&["find", "Foo", "--module", "features/play"])
        .unwrap()
        .unwrap();
    assert_eq!(find_filters(&args).module.as_deref(), Some("features/play"));
}

#[test]
fn refs_parses_single_source_set() {
    let args = parse(&["refs", "Foo", "--source-set", "commonMain"])
        .unwrap()
        .unwrap();
    assert_eq!(find_filters(&args).source_sets, vec!["commonMain"]);
}

#[test]
fn refs_parses_comma_separated_source_sets() {
    let args = parse(&[
        "refs",
        "Foo",
        "--source-set",
        "commonMain,androidMain,iosMain",
    ])
    .unwrap()
    .unwrap();
    assert_eq!(
        find_filters(&args).source_sets,
        vec!["commonMain", "androidMain", "iosMain"]
    );
}

#[test]
fn refs_dedupes_whitespace_in_source_set_csv() {
    let args = parse(&["refs", "Foo", "--source-set", " commonMain , androidMain "])
        .unwrap()
        .unwrap();
    assert_eq!(
        find_filters(&args).source_sets,
        vec!["commonMain", "androidMain"]
    );
}

#[test]
fn refs_accepts_repeated_source_set_flag() {
    // `--source-set commonMain --source-set androidMain` should also work as OR.
    let args = parse(&[
        "refs",
        "Foo",
        "--source-set",
        "commonMain",
        "--source-set",
        "androidMain",
    ])
    .unwrap()
    .unwrap();
    assert_eq!(
        find_filters(&args).source_sets,
        vec!["commonMain", "androidMain"]
    );
}

#[test]
fn find_combines_all_filter_flags() {
    let args = parse(&[
        "find",
        "Foo",
        "--json",
        "--relative",
        "--limit",
        "5",
        "--module",
        "play",
        "--source-set",
        "commonMain",
    ])
    .unwrap()
    .unwrap();
    assert!(matches!(args.fmt, OutputFmt::Json));
    let f = find_filters(&args);
    assert!(f.relative);
    assert_eq!(f.limit, Some(5));
    assert_eq!(f.module.as_deref(), Some("play"));
    assert_eq!(f.source_sets, vec!["commonMain"]);
}

#[test]
fn sources_explain_parses_first_positional_arg() {
    let args = parse(&["sources", "explain"]).unwrap().unwrap();
    match args.subcommand {
        Subcommand::Sources { explain } => assert!(explain),
        other => panic!("expected sources, got {other:?}"),
    }
}

#[test]
fn cache_stats_parses_first_positional_arg() {
    let args = parse(&["cache", "stats"]).unwrap().unwrap();
    match args.subcommand {
        Subcommand::Cache { sub } => assert_eq!(sub, "stats"),
        other => panic!("expected cache, got {other:?}"),
    }
}

#[test]
fn code_action_subcommand_is_reachable() {
    let args = parse(&["code-action", "Foo.kt", "2", "3", "--apply"])
        .unwrap()
        .unwrap();
    match args.subcommand {
        Subcommand::CodeAction {
            file,
            line,
            col,
            apply,
            ..
        } => {
            assert_eq!(file, std::path::PathBuf::from("Foo.kt"));
            assert_eq!(line, 2);
            assert_eq!(col, 3);
            assert!(apply);
        }
        other => panic!("expected code-action, got {other:?}"),
    }
}

#[test]
fn batch_imports_subcommand_is_reachable() {
    let args = parse(&[
        "batch-imports",
        "Foo.kt",
        "--dry-run",
        "--output",
        "out.json",
    ])
    .unwrap()
    .unwrap();
    match args.subcommand {
        Subcommand::Batch {
            file,
            dry_run,
            imports,
            output,
        } => {
            assert_eq!(file, std::path::PathBuf::from("Foo.kt"));
            assert!(dry_run);
            assert!(imports);
            assert_eq!(output.as_deref(), Some("out.json"));
        }
        other => panic!("expected batch-imports, got {other:?}"),
    }
}

#[test]
fn new_file_parses_template_and_name_from_first_two_args() {
    let args = parse(&[
        "new-file",
        "viewmodel",
        "LoginViewModel",
        "--package",
        "com.example",
        "--dir",
        "src/main/kotlin",
    ])
    .unwrap()
    .unwrap();
    match args.subcommand {
        Subcommand::NewFile {
            template,
            name,
            package_name,
            directory,
        } => {
            assert_eq!(template, "viewmodel");
            assert_eq!(name, "LoginViewModel");
            assert_eq!(package_name.as_deref(), Some("com.example"));
            assert_eq!(directory, Some(std::path::PathBuf::from("src/main/kotlin")));
        }
        other => panic!("expected new-file, got {other:?}"),
    }
}

#[test]
fn insert_parses_direction_content_and_in_place() {
    let args = parse(&[
        "insert",
        "Foo.kt",
        "10",
        "--after",
        "--content",
        "println(\"hi\")",
        "--in-place",
    ])
    .unwrap()
    .unwrap();
    match args.subcommand {
        Subcommand::Insert {
            file,
            line,
            before,
            after,
            content,
            in_place,
        } => {
            assert_eq!(file, std::path::PathBuf::from("Foo.kt"));
            assert_eq!(line, 10);
            assert!(!before);
            assert!(after);
            assert_eq!(content, "println(\"hi\")");
            assert!(in_place);
        }
        other => panic!("expected insert, got {other:?}"),
    }
}

#[test]
fn insert_requires_one_direction() {
    let err = parse(&["insert", "Foo.kt", "10", "--content", "println()"]).unwrap_err();
    assert!(err.contains("exactly one"), "got: {err}");
}

#[test]
fn insert_requires_content() {
    let err = parse(&["insert", "Foo.kt", "10", "--before"]).unwrap_err();
    assert!(err.contains("--content"), "got: {err}");
}

#[test]
fn batch_parses_rule_file_and_dry_run() {
    let args = parse(&["batch", "rules.json", "--dry-run"])
        .unwrap()
        .unwrap();
    match args.subcommand {
        Subcommand::Batch {
            file,
            dry_run,
            imports,
            output,
        } => {
            assert_eq!(file, std::path::PathBuf::from("rules.json"));
            assert!(dry_run);
            assert!(!imports);
            assert!(output.is_none());
        }
        other => panic!("expected batch, got {other:?}"),
    }
}

#[test]
fn index_jars_subcommand_is_reachable() {
    let args = parse(&["index-jars", "build/libs"]).unwrap().unwrap();
    match args.subcommand {
        Subcommand::IndexJars { root } => {
            assert_eq!(root, Some(std::path::PathBuf::from("build/libs")));
        }
        other => panic!("expected index-jars, got {other:?}"),
    }
}

#[test]
fn benchmark_subcommand_is_reachable() {
    let args = parse(&["benchmark"]).unwrap().unwrap();
    match args.subcommand {
        Subcommand::Benchmark => {}
        other => panic!("expected benchmark, got {other:?}"),
    }
}

#[test]
fn type_hierarchy_defaults_to_subtypes() {
    let args = parse(&["type-hierarchy", "Base"]).unwrap().unwrap();
    match args.subcommand {
        Subcommand::TypeHierarchy {
            name,
            subtypes,
            supertypes,
        } => {
            assert_eq!(name, "Base");
            assert!(subtypes);
            assert!(!supertypes);
        }
        other => panic!("expected type-hierarchy, got {other:?}"),
    }
}

#[test]
fn type_hierarchy_supertypes_flag_is_reachable() {
    let args = parse(&["type-hierarchy", "Child", "--supertypes"])
        .unwrap()
        .unwrap();
    match args.subcommand {
        Subcommand::TypeHierarchy {
            name,
            subtypes,
            supertypes,
        } => {
            assert_eq!(name, "Child");
            assert!(!subtypes);
            assert!(supertypes);
        }
        other => panic!("expected type-hierarchy, got {other:?}"),
    }
}

#[test]
fn type_hierarchy_can_request_both_directions() {
    let args = parse(&["type-hierarchy", "Node", "--subtypes", "--supertypes"])
        .unwrap()
        .unwrap();
    match args.subcommand {
        Subcommand::TypeHierarchy {
            name,
            subtypes,
            supertypes,
        } => {
            assert_eq!(name, "Node");
            assert!(subtypes);
            assert!(supertypes);
        }
        other => panic!("expected type-hierarchy, got {other:?}"),
    }
}
