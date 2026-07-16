//! Layout side-index updates via the same path as `did_change_watched_files`.

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use tower_lsp::lsp_types::{FileChangeType, Url};

use crate::indexer::Indexer;

const INITIAL_LAYOUT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <TextView
        android:id="@+id/title"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />
</LinearLayout>
"#;

const UPDATED_LAYOUT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<LinearLayout xmlns:android="http://schemas.android.com/apk/res/android"
    android:layout_width="match_parent"
    android:layout_height="match_parent">

    <TextView
        android:id="@+id/title"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />

    <TextView
        android:id="@+id/subtitle"
        android:layout_width="wrap_content"
        android:layout_height="wrap_content" />
</LinearLayout>
"#;

/// Mirrors `Backend::did_change_watched_files` layout routing for tests.
async fn apply_layout_watched_file_change(
    indexer: Arc<Indexer>,
    uri: Url,
    path: std::path::PathBuf,
    change_type: FileChangeType,
) {
    if change_type == FileChangeType::DELETED {
        indexer.remove_layout(&uri);
        return;
    }
    let semaphore = indexer.parse_sem();
    if let Ok(content) = tokio::fs::read_to_string(&path).await {
        if let Ok(permit) = semaphore.acquire_owned().await {
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                indexer.index_layout_content(&uri, &content);
            })
            .await
            .ok();
        }
    }
}

async fn wait_for_layout_field(indexer: &Indexer, uri: &str, field_id: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if indexer
                .layout_data_for_uri(uri)
                .is_some_and(|data| data.view_ids.iter().any(|view_id| view_id.id == field_id))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("layout side index not updated in time");
}

#[tokio::test]
async fn watched_layout_create_change_delete_updates_side_index() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module_root = temp.path().join("app");
    let layout_path = module_root.join("src/main/res/layout/foo_bar.xml");
    fs::create_dir_all(layout_path.parent().unwrap()).expect("mkdir layout");

    let indexer = Arc::new(Indexer::new());
    let layout_uri = Url::from_file_path(&layout_path).expect("layout uri");

    assert!(indexer.layout_data_for_uri(layout_uri.as_str()).is_none());

    std::fs::write(&layout_path, INITIAL_LAYOUT).expect("write initial layout");
    apply_layout_watched_file_change(
        Arc::clone(&indexer),
        layout_uri.clone(),
        layout_path.clone(),
        FileChangeType::CREATED,
    )
    .await;
    wait_for_layout_field(&indexer, layout_uri.as_str(), "title").await;

    let initial_data = indexer
        .layout_data_for_uri(layout_uri.as_str())
        .expect("layout indexed after create");
    assert_eq!(initial_data.layout_name, "foo_bar");
    assert!(!initial_data
        .view_ids
        .iter()
        .any(|view_id| view_id.id == "subtitle"));

    std::fs::write(&layout_path, UPDATED_LAYOUT).expect("write updated layout");
    apply_layout_watched_file_change(
        Arc::clone(&indexer),
        layout_uri.clone(),
        layout_path.clone(),
        FileChangeType::CHANGED,
    )
    .await;
    wait_for_layout_field(&indexer, layout_uri.as_str(), "subtitle").await;

    let updated_data = indexer
        .layout_data_for_uri(layout_uri.as_str())
        .expect("layout still indexed after change");
    assert!(updated_data
        .view_ids
        .iter()
        .any(|view_id| view_id.id == "subtitle"));

    apply_layout_watched_file_change(
        Arc::clone(&indexer),
        layout_uri.clone(),
        layout_path,
        FileChangeType::DELETED,
    )
    .await;

    assert!(indexer.layout_data_for_uri(layout_uri.as_str()).is_none());
}
