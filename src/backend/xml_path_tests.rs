use std::path::PathBuf;

use tower_lsp::lsp_types::Url;

use super::{is_xml_path, is_xml_uri};

#[test]
fn is_xml_path_matches_common_xml_files() {
    assert!(is_xml_path(&PathBuf::from(
        "app/src/main/res/layout/foo_bar.xml"
    )));
    assert!(is_xml_path(&PathBuf::from(
        "app/src/main/AndroidManifest.xml"
    )));
    assert!(is_xml_path(&PathBuf::from("pom.xml")));
    assert!(is_xml_path(&PathBuf::from(
        "app/src/main/res/values/colors.xml"
    )));
    assert!(is_xml_path(&PathBuf::from("Foo.XML")));
}

#[test]
fn is_xml_path_rejects_non_xml() {
    assert!(!is_xml_path(&PathBuf::from("app/src/main/kotlin/Main.kt")));
    assert!(!is_xml_path(&PathBuf::from("notes.xml.bak")));
}

#[test]
fn is_xml_uri_matches_file_uri() {
    let layout_uri =
        Url::from_file_path("/proj/app/src/main/res/layout/foo.xml").expect("layout uri");
    let manifest_uri =
        Url::from_file_path("/proj/app/src/main/AndroidManifest.xml").expect("manifest uri");
    assert!(is_xml_uri(&layout_uri));
    assert!(is_xml_uri(&manifest_uri));
}

#[test]
fn is_xml_uri_matches_uri_path_suffix() {
    let uri = Url::parse("file:///workspace/pom.xml").expect("pom uri");
    assert!(is_xml_uri(&uri));
}

#[test]
fn is_xml_uri_rejects_kotlin() {
    let uri = Url::parse("file:///workspace/Main.kt").expect("kotlin uri");
    assert!(!is_xml_uri(&uri));
}
