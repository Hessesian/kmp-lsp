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
