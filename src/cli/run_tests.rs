//! Tests for CLI-path Android SDK jar wiring (`find`/`diagnose` bypassing
//! `ScanHandler`'s own `detect_android_sdk_jar_path` wiring — see
//! `compiled_jar_paths_for_cli`).

use super::compiled_jar_paths_for_cli;
use crate::indexer::test_helpers::ENV_VAR_LOCK;
use std::fs;
use tempfile::TempDir;

/// Run `f` with both `ANDROID_HOME` and `ANDROID_SDK_ROOT` unset — CI
/// runners (GitHub-hosted `ubuntu-latest`/`macos-latest`/`windows-latest`)
/// ship a real, pre-installed Android SDK with these set, which would
/// otherwise leak into `resolve_android_sdk_root`'s env-var fallback and
/// make `detect_android_sdk_jar_path` find a real SDK even for a fixture
/// with no `local.properties`. Locks `ENV_VAR_LOCK` once (rather than
/// nesting two `with_env_var_unset` calls against the same non-reentrant
/// mutex, which would deadlock) and restores both vars on the way out.
fn without_host_android_sdk<F: FnOnce()>(f: F) {
    let _guard = ENV_VAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev_home = std::env::var("ANDROID_HOME").ok();
    let prev_sdk_root = std::env::var("ANDROID_SDK_ROOT").ok();
    std::env::remove_var("ANDROID_HOME");
    std::env::remove_var("ANDROID_SDK_ROOT");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    match prev_home {
        Some(v) => std::env::set_var("ANDROID_HOME", v),
        None => std::env::remove_var("ANDROID_HOME"),
    }
    match prev_sdk_root {
        Some(v) => std::env::set_var("ANDROID_SDK_ROOT", v),
        None => std::env::remove_var("ANDROID_SDK_ROOT"),
    }
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

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
    without_host_android_sdk(|| {
        let tmp = TempDir::new().unwrap();
        let expected_jar = write_fake_android_sdk(tmp.path(), "android-34");

        // Empty Gradle-cache scan result — proves the SDK jar is added on top
        // of whatever `scan_gradle_jars` finds, not derived from it.
        let jars = compiled_jar_paths_for_cli(tmp.path(), Vec::new());

        assert!(
            jars.contains(&expected_jar),
            "CLI find/diagnose jar list must include the detected android.jar, got: {jars:?}"
        );
    });
}

#[test]
fn compiled_jar_paths_for_cli_does_not_duplicate_an_already_present_sdk_jar() {
    without_host_android_sdk(|| {
        let tmp = TempDir::new().unwrap();
        let expected_jar = write_fake_android_sdk(tmp.path(), "android-34");

        let jars = compiled_jar_paths_for_cli(tmp.path(), vec![expected_jar.clone()]);

        assert_eq!(
            jars.iter().filter(|p| **p == expected_jar).count(),
            1,
            "the SDK jar must not be duplicated when the Gradle-cache scan already found it"
        );
    });
}

#[test]
fn compiled_jar_paths_for_cli_is_a_no_op_without_an_android_sdk() {
    without_host_android_sdk(|| {
        let tmp = TempDir::new().unwrap();
        // No local.properties / SDK — nothing to detect.
        let jars = compiled_jar_paths_for_cli(tmp.path(), Vec::new());
        assert!(
            jars.is_empty(),
            "no SDK jar should be added when none is detected, got: {jars:?}"
        );
    });
}
