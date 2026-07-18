use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::indexer::Indexer;
use crate::viewbinding::watcher::spawn_databinding_watcher_with_interval;
use crate::viewbinding::{DatabindingWatcherHandle, DatabindingWatcherState};
use crate::workspace::Event;

const SAMPLE_BINDING_JAVA: &str = r#"package com.example.app.databinding;

import android.view.LayoutInflater;
import android.view.View;

public final class FooBarBinding {
    private FooBarBinding() {}
}
"#;

const WRONG_PACKAGE_BINDING_JAVA: &str = r#"package com.example.app.ui;

public final class FooBarBinding {
}
"#;

fn write_binding_java(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir binding dir");
    }
    fs::write(path, content).expect("write binding java");
}

fn binding_path(module_root: &Path) -> PathBuf {
    module_root.join(
        "build/generated/data_binding_base_class_source_out/debug/out/com/example/app/databinding/FooBarBinding.java",
    )
}

async fn poll_until<F: Fn() -> bool>(condition: F, timeout: Duration) {
    tokio::time::timeout(timeout, async {
        loop {
            if condition() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("condition not met within timeout");
}

fn spawn_test_watcher(
    indexer: Arc<Indexer>,
    republish_tx: mpsc::Sender<Event>,
) -> DatabindingWatcherHandle {
    spawn_databinding_watcher_with_interval(indexer, republish_tx, Duration::from_millis(50))
}

#[tokio::test]
async fn watch_module_is_idempotent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let binding_path = binding_path(&module_root);
    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);

    let indexer = Arc::new(Indexer::new());
    let (republish_tx, _republish_rx) = mpsc::channel(4);
    let handle = spawn_test_watcher(Arc::clone(&indexer), republish_tx);

    indexer.set_databinding_watcher_handle(handle.clone());
    indexer.index_generated_bindings(&module_root, None);
    let parse_count_after_first = indexer.parse_count.load(Ordering::Relaxed);

    indexer.index_generated_bindings(&module_root, None);
    let parse_count_after_second = indexer.parse_count.load(Ordering::Relaxed);

    assert_eq!(
        parse_count_after_first, parse_count_after_second,
        "duplicate watch_module registration must not trigger extra discovery"
    );
}

#[tokio::test]
async fn set_watcher_handle_registers_modules_discovered_before_install() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    write_binding_java(&binding_path(&module_root), SAMPLE_BINDING_JAVA);

    let indexer = Arc::new(Indexer::new());

    // Discovery runs against the default noop handle — exactly what happens
    // during early workspace indexing, before `initialized` installs the real
    // watcher. The module is discovered but not yet in any watcher's watched set.
    indexer.index_generated_bindings(&module_root, None);

    let state = Arc::new(DatabindingWatcherState::new());
    assert!(
        state.registered_module_roots().is_empty(),
        "a fresh watcher state must start with no registered module roots"
    );

    indexer.set_databinding_watcher_handle(DatabindingWatcherHandle::new(Arc::clone(&state)));

    assert!(
        state.registered_module_roots().contains(&module_root),
        "a module discovered before the handle was installed must be re-registered on install"
    );
}

#[tokio::test]
async fn watcher_indexes_new_binding_file_end_to_end() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let binding_path = binding_path(&module_root);

    let indexer = Arc::new(Indexer::new());
    let (republish_tx, _republish_rx) = mpsc::channel(4);
    let handle = spawn_test_watcher(Arc::clone(&indexer), republish_tx);
    indexer.set_databinding_watcher_handle(handle.clone());
    handle.watch_module(&module_root);

    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);

    let qualified_key = "com.example.app.databinding.FooBarBinding";
    let indexer = Arc::clone(&indexer);
    poll_until(
        move || indexer.qualified.contains_key(qualified_key),
        Duration::from_secs(5),
    )
    .await;
}

#[tokio::test]
async fn watcher_ignores_non_binding_and_outside_databinding_paths() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    fs::create_dir_all(module_root.join("build/tmp")).expect("mkdir tmp");

    let indexer = Arc::new(Indexer::new());
    let (republish_tx, _republish_rx) = mpsc::channel(4);
    let handle = spawn_test_watcher(Arc::clone(&indexer), republish_tx);
    indexer.set_databinding_watcher_handle(handle.clone());
    handle.watch_module(&module_root);

    let parse_count_before = indexer.parse_count.load(Ordering::Relaxed);

    fs::write(module_root.join("build/tmp/whatever.txt"), "not a binding").expect("write txt");
    write_binding_java(
        &module_root.join("build/generated/source/FooBarBinding.java"),
        WRONG_PACKAGE_BINDING_JAVA,
    );

    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(
        indexer.parse_count.load(Ordering::Relaxed),
        parse_count_before,
        "non-binding files must not trigger re-discovery"
    );
    assert!(!indexer
        .qualified
        .contains_key("com.example.app.databinding.FooBarBinding"));
}

#[tokio::test]
async fn watcher_triggers_when_build_dir_appears_after_registration() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    fs::create_dir_all(&module_root).expect("mkdir module");

    let indexer = Arc::new(Indexer::new());
    let (republish_tx, _republish_rx) = mpsc::channel(4);
    let handle = spawn_test_watcher(Arc::clone(&indexer), republish_tx);
    indexer.set_databinding_watcher_handle(handle.clone());
    handle.watch_module(&module_root);

    write_binding_java(&binding_path(&module_root), SAMPLE_BINDING_JAVA);

    let qualified_key = "com.example.app.databinding.FooBarBinding";
    let indexer = Arc::clone(&indexer);
    poll_until(
        move || indexer.qualified.contains_key(qualified_key),
        Duration::from_secs(5),
    )
    .await;
}

#[tokio::test]
async fn rapid_binding_writes_coalesce_to_one_rediscovery() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let binding_path = binding_path(&module_root);
    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);

    let indexer = Arc::new(Indexer::new());
    let (republish_tx, _republish_rx) = mpsc::channel(4);
    let handle = spawn_test_watcher(Arc::clone(&indexer), republish_tx);
    indexer.set_databinding_watcher_handle(handle.clone());
    indexer.index_generated_bindings(&module_root, None);

    tokio::time::sleep(Duration::from_millis(120)).await;
    let parse_count_after_initial = indexer.parse_count.load(Ordering::Relaxed);
    assert!(parse_count_after_initial >= 1);

    // Advance to a new whole-second mtime bucket on coarse filesystems.
    tokio::time::sleep(Duration::from_millis(1100)).await;

    for suffix in 1..=5 {
        let content = format!("{SAMPLE_BINDING_JAVA}\n// touch {suffix}\n");
        write_binding_java(&binding_path, &content);
    }

    tokio::time::sleep(Duration::from_millis(200)).await;

    let parse_count_after_burst = indexer.parse_count.load(Ordering::Relaxed);
    assert_eq!(
        parse_count_after_burst,
        parse_count_after_initial + 1,
        "rapid writes within one poll interval should coalesce to a single re-discovery"
    );
}

#[tokio::test]
async fn watcher_clears_build_required_import_diagnostic_after_discovery() {
    use tower_lsp::lsp_types::Url;

    use crate::viewbinding::viewbinding_import_diagnostics;

    const FOO_BAR_LAYOUT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <TextView
        android:id="@+id/title"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />
</LinearLayout>
"#;

    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let layout_dir = module_root.join("src/main/res/layout");
    fs::create_dir_all(&layout_dir).expect("mkdir layout");
    let layout_path = layout_dir.join("foo_bar.xml");
    fs::write(&layout_path, FOO_BAR_LAYOUT).expect("write layout");

    let kotlin_path = module_root.join("src/main/kotlin/com/example/MainActivity.kt");
    fs::create_dir_all(kotlin_path.parent().unwrap()).expect("mkdir kotlin");
    let kotlin_source = r#"package com.example

import com.example.app.databinding.FooBarBinding

class MainActivity {
    fun demo(binding: FooBarBinding) {
        binding.title
    }
}
"#;
    fs::write(&kotlin_path, kotlin_source).expect("write kotlin");

    let indexer = Arc::new(Indexer::new());
    let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");
    let kotlin_uri = Url::from_file_path(&kotlin_path).expect("kotlin uri");
    indexer.index_layout_content(&layout_uri, FOO_BAR_LAYOUT);
    indexer.index_content(&kotlin_uri, kotlin_source);
    indexer.set_live_lines(&kotlin_uri, kotlin_source);
    indexer.store_live_tree(&kotlin_uri, kotlin_source);

    assert_eq!(
        viewbinding_import_diagnostics(&indexer, &kotlin_uri).len(),
        1,
        "build-required warning expected before generated class exists"
    );

    let (republish_tx, mut republish_rx) = mpsc::channel(4);
    let handle = spawn_test_watcher(Arc::clone(&indexer), republish_tx);
    indexer.set_databinding_watcher_handle(handle.clone());
    handle.watch_module(&module_root);

    write_binding_java(&binding_path(&module_root), SAMPLE_BINDING_JAVA);

    let qualified_key = "com.example.app.databinding.FooBarBinding";
    let indexer_for_poll = Arc::clone(&indexer);
    poll_until(
        move || indexer_for_poll.qualified.contains_key(qualified_key),
        Duration::from_secs(5),
    )
    .await;

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                republish_rx.recv().await,
                Some(Event::RepublishOpenFileDiagnostics)
            ) {
                break;
            }
        }
    })
    .await
    .expect("watcher must request diagnostic republish after binding discovery");

    let diags = viewbinding_import_diagnostics(&indexer, &kotlin_uri);
    assert!(
        diags.is_empty(),
        "build-required diagnostic must clear after watcher discovery: {diags:?}"
    );
}

/// Deterministic: empty discovery is never cached, so a later `build/` tree is found.
#[tokio::test]
async fn watcher_finds_bindings_under_intermediates_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let binding_path = module_root.join(
        "build/intermediates/data_binding/debug/com/example/app/databinding/FooBarBinding.java",
    );
    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);

    let indexer = Arc::new(Indexer::new());
    let (republish_tx, _republish_rx) = mpsc::channel(4);
    let handle = spawn_test_watcher(Arc::clone(&indexer), republish_tx);
    indexer.set_databinding_watcher_handle(handle.clone());
    handle.watch_module(&module_root);

    let qualified_key = "com.example.app.databinding.FooBarBinding";
    let indexer_for_poll = Arc::clone(&indexer);
    poll_until(
        move || indexer_for_poll.qualified.contains_key(qualified_key),
        Duration::from_secs(5),
    )
    .await;
}

#[tokio::test]
async fn watcher_detects_gradle_clean_and_clears_index() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let binding_path = binding_path(&module_root);
    write_binding_java(&binding_path, SAMPLE_BINDING_JAVA);

    let indexer = Arc::new(Indexer::new());
    let (republish_tx, _republish_rx) = mpsc::channel(4);
    let handle = spawn_test_watcher(Arc::clone(&indexer), republish_tx);
    indexer.set_databinding_watcher_handle(handle.clone());
    indexer.index_generated_bindings(&module_root, None);

    let qualified_key = "com.example.app.databinding.FooBarBinding";
    assert!(indexer.qualified.contains_key(qualified_key));

    tokio::time::sleep(Duration::from_millis(120)).await;

    fs::remove_file(&binding_path).expect("simulate gradle clean");

    let indexer_for_poll = Arc::clone(&indexer);
    poll_until(
        move || !indexer_for_poll.qualified.contains_key(qualified_key),
        Duration::from_secs(5),
    )
    .await;
}

#[test]
fn resolve_databinding_dirs_rediscovers_after_build_dir_appears() {
    use super::resolve_databinding_dirs;

    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    fs::create_dir_all(&module_root).expect("mkdir module");

    let mut databinding_dirs: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

    let before_build = resolve_databinding_dirs(&mut databinding_dirs, &module_root);
    assert!(before_build.is_empty(), "no build/ yet");
    assert!(
        !databinding_dirs.contains_key(&module_root),
        "empty discovery must not be cached"
    );

    write_binding_java(&binding_path(&module_root), SAMPLE_BINDING_JAVA);

    let after_build = resolve_databinding_dirs(&mut databinding_dirs, &module_root);
    assert!(
        !after_build.is_empty(),
        "build/ and binding file exist — dirs must be discovered"
    );
    assert!(
        databinding_dirs.contains_key(&module_root),
        "non-empty discovery must be cached"
    );

    let cached = resolve_databinding_dirs(&mut databinding_dirs, &module_root);
    assert_eq!(cached, after_build, "subsequent polls reuse cached dirs");
}
