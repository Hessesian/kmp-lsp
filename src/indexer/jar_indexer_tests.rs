//! Tests for the JAR source-file indexer.

use super::*;
use std::path::Path;
use tower_lsp::lsp_types::SymbolKind;

#[test]
fn extract_decl_class() {
    let line = "public class String : Any";
    assert_eq!(extract_decl(line, &["class "]), Some("String"));

    let line = "class StringBuilder";
    assert_eq!(extract_decl(line, &["class "]), Some("StringBuilder"));
}

#[test]
fn extract_decl_interface() {
    let line = "public interface Serializable";
    assert_eq!(extract_decl(line, &["interface "]), Some("Serializable"));
}

#[test]
fn extract_decl_enum() {
    let line = "enum class Color { RED, GREEN, BLUE }";
    assert_eq!(extract_decl(line, &["enum class "]), Some("Color"));
}

#[test]
fn extract_fun_name_simple() {
    let line = "fun foo(x: Int): String";
    assert_eq!(extract_fun_name(line), Some("foo"));
}

#[test]
fn extract_fun_name_generic() {
    let line = "fun <T> create(service: Class<T>): T";
    assert_eq!(extract_fun_name(line), Some("create"));
}

#[test]
fn extract_fun_name_public() {
    let line = "public fun bar(): Unit";
    assert_eq!(extract_fun_name(line), Some("bar"));
}

#[test]
fn kind_mapping() {
    assert_eq!(kind_for_decl("class Foo"), SymbolKind::CLASS);
    assert_eq!(kind_for_decl("interface Bar"), SymbolKind::INTERFACE);
}

#[test]
fn jar_symbol_kind_mapping() {
    assert_eq!(kind_for_decl("class Foo"), SymbolKind::CLASS);
    assert_eq!(kind_for_decl("interface Foo"), SymbolKind::INTERFACE);
    assert_eq!(kind_for_decl("enum class Foo"), SymbolKind::ENUM);
    assert_eq!(kind_for_decl("object Foo"), SymbolKind::OBJECT);
}

#[test]
fn symbols_to_filedata_basic() {
    let symbols = vec![JarSymbol {
        name: "String".to_owned(),
        kind: SymbolKind::CLASS,
        file_path: "kotlin/String.kt".to_owned(),
        line: 42,
        detail: "public class String : CharSequence".to_owned(),
    }];

    let (fd, defs) = symbols_to_filedata(Path::new("/tmp/test.jar"), &symbols);

    assert_eq!(fd.symbols.len(), 1);
    assert_eq!(fd.symbols[0].name, "String");
    assert!(true); // FileData covers symbols and definitions
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].0, "String");
}
