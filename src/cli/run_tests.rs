//! Tests for CLI-path Android SDK jar wiring (`find`/`diagnose` bypassing
//! `ScanHandler`'s own `detect_android_sdk_jar_path` wiring — see
//! `compiled_jar_paths_for_cli`).

use super::compiled_jar_paths_for_cli;
use std::fs;
use tempfile::TempDir;

/// Build a fake Android SDK install under `dir` with one platform containing
/// `android.jar`, and a `local.properties` pointing the workspace at it —
/// mirrors the fixture `workspace_json_tests.rs` uses for
/// `detect_android_sdk_jar_path` itself.
fn write_fake_android_sdk(dir: &std::path::Path, api_level: &str) -> std::path::PathBuf {
    let fake_sdk = dir.join("sdk");
    let platform_dir = fake_sdk.join("platforms").join(api_level);
    fs::create_dir_all(&platform_dir).unwrap();
    let jar_path = platform_dir.join("android.jar");
    fs::write(&jar_path, b"fake jar").unwrap();
    fs::write(
        dir.join("local.properties"),
        format!("sdk.dir={}\n", fake_sdk.display()),
    )
    .unwrap();
    jar_path
}

#[test]
fn compiled_jar_paths_for_cli_includes_the_detected_android_sdk_jar() {
    let tmp = TempDir::new().unwrap();
    let expected_jar = write_fake_android_sdk(tmp.path(), "android-34");

    // Empty Gradle-cache scan result — proves the SDK jar is added on top
    // of whatever `scan_gradle_jars` finds, not derived from it.
    let jars = compiled_jar_paths_for_cli(tmp.path(), Vec::new());

    assert!(
        jars.contains(&expected_jar),
        "CLI find/diagnose jar list must include the detected android.jar, got: {jars:?}"
    );
}

#[test]
fn compiled_jar_paths_for_cli_does_not_duplicate_an_already_present_sdk_jar() {
    let tmp = TempDir::new().unwrap();
    let expected_jar = write_fake_android_sdk(tmp.path(), "android-34");

    let jars = compiled_jar_paths_for_cli(tmp.path(), vec![expected_jar.clone()]);

    assert_eq!(
        jars.iter().filter(|p| **p == expected_jar).count(),
        1,
        "the SDK jar must not be duplicated when the Gradle-cache scan already found it"
    );
}

#[test]
fn compiled_jar_paths_for_cli_is_a_no_op_without_an_android_sdk() {
    let tmp = TempDir::new().unwrap();
    // No local.properties / SDK — nothing to detect.
    let jars = compiled_jar_paths_for_cli(tmp.path(), Vec::new());
    assert!(
        jars.is_empty(),
        "no SDK jar should be added when none is detected, got: {jars:?}"
    );
}
