use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tower_lsp::lsp_types::{Position, Range, SymbolKind, Url};

use super::{IndexRead, WorkspaceRead};
use crate::indexer::Location;
use crate::types::{FileData, SymbolEntry, Visibility};

#[derive(Default)]
struct TestWorkspace {
    definitions: HashMap<String, Vec<Location>>,
    files: HashMap<String, Arc<FileData>>,
    workspace_root: Option<PathBuf>,
}

impl TestWorkspace {
    fn with_definition(mut self, name: &str, location: Location) -> Self {
        self.definitions.insert(name.to_owned(), vec![location]);
        self
    }

    fn with_file(mut self, uri: &Url, symbols: Vec<SymbolEntry>) -> Self {
        self.files.insert(
            uri.as_str().to_owned(),
            Arc::new(FileData {
                symbols,
                lines: Arc::new(vec!["class Stub".to_owned()]),
                ..FileData::default()
            }),
        );
        self
    }

    fn with_workspace_root(mut self, path: &str) -> Self {
        self.workspace_root = Some(PathBuf::from(path));
        self
    }
}

impl IndexRead for TestWorkspace {
    fn get_definitions(&self, name: &str) -> Option<Vec<Location>> {
        self.definitions.get(name).cloned()
    }

    fn get_file_data(&self, uri: &str) -> Option<Arc<FileData>> {
        self.files.get(uri).cloned()
    }
}

impl WorkspaceRead for TestWorkspace {
    fn workspace_root(&self) -> Option<PathBuf> {
        self.workspace_root.clone()
    }
}

#[test]
fn definition_locations_returns_stubbed_locations() {
    let location = location("/workspace/src/Foo.kt", 3);
    let workspace = TestWorkspace::default().with_definition("Foo", location.clone());

    assert_eq!(workspace.definition_locations("Foo"), vec![location]);
}

#[test]
fn file_symbols_filters_by_uri() {
    let foo_uri = file_url("/workspace/src/Foo.kt");
    let bar_uri = file_url("/workspace/src/Bar.kt");
    let workspace = TestWorkspace::default()
        .with_file(&foo_uri, vec![symbol("Foo")])
        .with_file(&bar_uri, vec![symbol("Bar")]);

    let foo_symbols: Vec<String> = workspace
        .file_symbols(&foo_uri)
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    let bar_symbols: Vec<String> = workspace
        .file_symbols(&bar_uri)
        .into_iter()
        .map(|entry| entry.name)
        .collect();

    assert_eq!(foo_symbols, vec!["Foo"]);
    assert_eq!(bar_symbols, vec!["Bar"]);
}

#[test]
fn workspace_root_returns_set_value() {
    let workspace = TestWorkspace::default().with_workspace_root("/workspace/project");

    assert_eq!(
        workspace.workspace_root(),
        Some(PathBuf::from("/workspace/project"))
    );
}

fn file_url(path: &str) -> Url {
    Url::parse(&format!("file://{path}")).expect("valid file URL")
}

fn location(path: &str, line: u32) -> Location {
    Location {
        uri: file_url(path),
        range: Range::new(Position::new(line, 0), Position::new(line, 3)),
    }
}

fn symbol(name: &str) -> SymbolEntry {
    let range = Range::new(Position::new(0, 0), Position::new(0, name.len() as u32));
    SymbolEntry {
        name: name.to_owned(),
        kind: SymbolKind::CLASS,
        visibility: Visibility::Public,
        range,
        selection_range: range,
        detail: format!("class {name}"),
        type_params: Vec::new(),
        extension_receiver: String::new(),
    }
}
