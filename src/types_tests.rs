use super::{ImportEntry, JarId, JarTable};

#[test]
fn covers_direct_import() {
    let import = ImportEntry {
        full_path: "com.example.Config".to_owned(),
        local_name: "Config".to_owned(),
        is_star: false,
    };

    assert!(import.covers("com.example", "Config"));
}

#[test]
fn covers_nested_import() {
    let import = ImportEntry {
        full_path: "com.example.Outer.Config".to_owned(),
        local_name: "Config".to_owned(),
        is_star: false,
    };

    assert!(import.covers("com.example", "Config"));
}

#[test]
fn covers_deeply_nested_import() {
    let import = ImportEntry {
        full_path: "com.example.Outer.Inner.Config".to_owned(),
        local_name: "Config".to_owned(),
        is_star: false,
    };

    assert!(import.covers("com.example", "Config"));
}

#[test]
fn covers_star_import_for_package_members() {
    let import = ImportEntry {
        full_path: "com.example".to_owned(),
        local_name: "*".to_owned(),
        is_star: true,
    };

    assert!(import.covers("com.example", "Config"));
}

#[test]
fn does_not_cover_other_package() {
    let import = ImportEntry {
        full_path: "com.other.Outer.Config".to_owned(),
        local_name: "Config".to_owned(),
        is_star: false,
    };

    assert!(!import.covers("com.example", "Config"));
}

#[test]
fn jar_table_intern_is_idempotent() {
    let table = JarTable::new();
    let id_a = table.intern("/gradle/caches/foo-1.0.jar");
    let id_b = table.intern("/gradle/caches/foo-1.0.jar");
    assert_eq!(
        id_a, id_b,
        "interning the same path twice must return the same JarId"
    );
    let id_c = table.intern("/gradle/caches/bar-2.0.jar");
    assert_ne!(id_a, id_c, "different paths must get different JarIds");
    assert_eq!(
        table.path(id_a).as_deref(),
        Some("/gradle/caches/foo-1.0.jar")
    );
}

#[test]
fn jar_table_intern_concurrent_same_path_yields_one_id() {
    use std::sync::Arc;
    let table = Arc::new(JarTable::new());
    let mut handles = Vec::new();
    for _ in 0..8 {
        let table = Arc::clone(&table);
        handles.push(std::thread::spawn(move || {
            table.intern("/gradle/caches/shared.jar")
        }));
    }
    let ids: Vec<JarId> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let first = ids[0];
    assert!(
        ids.iter().all(|id| *id == first),
        "concurrent interning of the same path must never mint two ids"
    );
}
