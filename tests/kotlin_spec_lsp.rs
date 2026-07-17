//! Specification-oriented end-to-end tests for the advertised LSP contract.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const BINARY_PATH: &str = env!("CARGO_BIN_EXE_kmp-lsp");
const INDEXING_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

struct SpecificationLspClient {
    standard_input: ChildStdin,
    messages: Receiver<Value>,
    next_request_id: u64,
    _child_process: Child,
}

impl SpecificationLspClient {
    fn spawn(workspace_root: &Path) -> Self {
        let canonical_workspace_root = canonical_path(workspace_root);
        let mut child_process = Command::new(BINARY_PATH)
            .arg("--stdio")
            .env("KMP_LSP_WORKSPACE_ROOT", &canonical_workspace_root)
            .current_dir(&canonical_workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("kmp-lsp must start in stdio mode");

        let standard_input = child_process
            .stdin
            .take()
            .expect("kmp-lsp stdin must be piped");
        let standard_output = child_process
            .stdout
            .take()
            .expect("kmp-lsp stdout must be piped");
        let (message_sender, messages) = mpsc::channel();

        std::thread::spawn(move || {
            let mut reader = BufReader::new(standard_output);
            while let Some(message) = read_lsp_message(&mut reader) {
                if message_sender.send(message).is_err() {
                    break;
                }
            }
        });

        Self {
            standard_input,
            messages,
            next_request_id: 1,
            _child_process: child_process,
        }
    }

    fn initialize(&mut self, workspace_root: &Path) -> Value {
        let root_uri = file_uri(workspace_root);
        let response = self.request(
            "initialize",
            json!({
                "rootUri": root_uri,
                "capabilities": {
                    "textDocument": {
                        "completion": {"completionItem": {"snippetSupport": false}},
                    },
                    "window": {"workDoneProgress": true},
                },
            }),
        );
        assert!(
            response.get("result").is_some(),
            "initialize must return a result: {response}"
        );
        self.notify("initialized", json!({}));
        response
    }

    fn wait_for_indexing(&mut self) {
        let deadline = Instant::now() + INDEXING_TIMEOUT;
        loop {
            let message = self.next_message(deadline, "workspace indexing completion");
            if self.acknowledge_server_request(&message) {
                continue;
            }

            let is_indexing_progress = message.get("method") == Some(&json!("$/progress"));
            let is_indexing_token = message["params"]["token"] == "kmp-lsp/indexing";
            let is_end_event = message["params"]["value"]["kind"] == "end";
            if is_indexing_progress && is_indexing_token && is_end_event {
                return;
            }
        }
    }

    fn request(&mut self, method: &str, parameters: Value) -> Value {
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": parameters,
        }));

        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            let message = self.next_message(deadline, method);
            if self.acknowledge_server_request(&message) {
                continue;
            }
            if message.get("id") == Some(&json!(request_id)) {
                assert!(
                    message.get("error").is_none(),
                    "{method} returned an error: {message}"
                );
                return message;
            }
        }
    }

    fn notify(&mut self, method: &str, parameters: Value) {
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": parameters,
        }));
    }

    fn open_document(&mut self, uri: &str, contents: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "kotlin",
                    "version": 1,
                    "text": contents,
                },
            }),
        );
    }

    fn write_message(&mut self, message: &Value) {
        let body = serde_json::to_string(message).expect("JSON-RPC message must serialize");
        write!(
            self.standard_input,
            "Content-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("JSON-RPC message must be written");
        self.standard_input
            .flush()
            .expect("JSON-RPC message must be flushed");
    }

    fn next_message(&mut self, deadline: Instant, awaited_operation: &str) -> Value {
        let remaining = deadline.saturating_duration_since(Instant::now());
        self.messages.recv_timeout(remaining).unwrap_or_else(|_| {
            panic!("timed out waiting for {awaited_operation} after {RESPONSE_TIMEOUT:?}")
        })
    }

    fn acknowledge_server_request(&mut self, message: &Value) -> bool {
        let is_server_request = message.get("method").is_some() && message.get("id").is_some();
        if !is_server_request {
            return false;
        }

        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": null,
        }));
        true
    }
}

impl Drop for SpecificationLspClient {
    fn drop(&mut self) {
        let shutdown_request_id = self.next_request_id;
        let shutdown_body = json!({
            "jsonrpc": "2.0",
            "id": shutdown_request_id,
            "method": "shutdown",
            "params": null,
        });
        let exit_notification = json!({
            "jsonrpc": "2.0",
            "method": "exit",
            "params": null,
        });
        let _ = write_lsp_message(&mut self.standard_input, &shutdown_body);
        let _ = write_lsp_message(&mut self.standard_input, &exit_notification);
    }
}

fn read_lsp_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut content_length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            return None;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some(length) = header.strip_prefix("Content-Length: ") {
            content_length = length.parse::<usize>().ok();
        }
    }

    let mut body = vec![0; content_length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn write_lsp_message(writer: &mut impl Write, message: &Value) -> std::io::Result<()> {
    let body = serde_json::to_string(message)?;
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    writer.flush()
}

fn canonical_path(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let canonical_text = canonical.to_string_lossy();
    canonical_text
        .strip_prefix("\\\\?\\")
        .map(PathBuf::from)
        .unwrap_or(canonical)
}

fn file_uri(path: &Path) -> String {
    tower_lsp::lsp_types::Url::from_file_path(canonical_path(path))
        .expect("fixture path must convert to a file URI")
        .to_string()
}

fn write_fixture_file(workspace_root: &Path, relative_path: &str, contents: &str) {
    let path = workspace_root.join(relative_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture directory must be created");
    }
    std::fs::write(path, contents).expect("fixture file must be written");
}

fn position_of(contents: &str, needle: &str, occurrence: usize) -> Value {
    let byte_offset = contents
        .match_indices(needle)
        .nth(occurrence)
        .map(|(offset, _)| offset)
        .unwrap_or_else(|| panic!("fixture must contain occurrence {occurrence} of {needle:?}"));
    let preceding_text = &contents[..byte_offset];
    let line = preceding_text.bytes().filter(|byte| *byte == b'\n').count();
    let line_start = preceding_text.rfind('\n').map_or(0, |offset| offset + 1);
    let character = contents[line_start..byte_offset].encode_utf16().count();
    json!({"line": line, "character": character})
}

fn completion_items(response: &Value) -> Vec<Value> {
    let result = &response["result"];
    result
        .as_array()
        .or_else(|| result["items"].as_array())
        .cloned()
        .unwrap_or_default()
}

#[test]
fn advertised_capabilities_match_the_stdio_contract() {
    let temporary_directory = tempfile::tempdir().expect("temporary workspace must be created");
    let workspace_root = temporary_directory.path();
    write_fixture_file(workspace_root, "workspace.json", r#"{"sourcePaths":[]}"#);

    let mut client = SpecificationLspClient::spawn(workspace_root);
    let initialize_response = client.initialize(workspace_root);
    let capabilities = &initialize_response["result"]["capabilities"];

    assert_eq!(capabilities["textDocumentSync"]["openClose"], true);
    assert_eq!(capabilities["textDocumentSync"]["change"], 1);
    assert_eq!(
        capabilities["textDocumentSync"]["save"]["includeText"],
        false
    );
    assert_eq!(
        capabilities["completionProvider"]["triggerCharacters"],
        json!([".", ":", "@"])
    );
    assert_eq!(capabilities["completionProvider"]["resolveProvider"], true);

    for capability_name in [
        "hoverProvider",
        "definitionProvider",
        "declarationProvider",
        "implementationProvider",
        "referencesProvider",
        "documentHighlightProvider",
        "documentSymbolProvider",
        "inlayHintProvider",
        "workspaceSymbolProvider",
        "foldingRangeProvider",
        "codeActionProvider",
    ] {
        assert_eq!(
            capabilities[capability_name], true,
            "{capability_name} must be advertised"
        );
    }

    assert_eq!(capabilities["renameProvider"]["prepareProvider"], true);
    assert_eq!(
        capabilities["signatureHelpProvider"]["triggerCharacters"],
        json!(["(", ","])
    );
    assert_eq!(
        capabilities["signatureHelpProvider"]["retriggerCharacters"],
        json!(["(", ","])
    );
    assert_eq!(
        capabilities["documentOnTypeFormattingProvider"]["firstTriggerCharacter"],
        "\n"
    );
    assert_eq!(capabilities["semanticTokensProvider"]["full"], true);
    assert_eq!(capabilities["semanticTokensProvider"]["range"], true);
    assert_eq!(
        capabilities["executeCommandProvider"]["commands"],
        json!(["kmp-lsp/reindex", "kmp-lsp/clearCache"])
    );
}

#[test]
fn navigation_capabilities_resolve_competing_android_shaped_symbols() {
    let temporary_directory = tempfile::tempdir().expect("temporary workspace must be created");
    let workspace_root = temporary_directory.path();
    write_fixture_file(workspace_root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture_file(
        workspace_root,
        "src/main/kotlin/sample/render/Renderer.kt",
        "package sample.render\n\ninterface Renderer {\n    fun render(): String\n}\n\nclass CardRenderer : Renderer {\n    override fun render(): String = \"card\"\n}\n",
    );
    write_fixture_file(
        workspace_root,
        "src/main/kotlin/sample/other/Renderer.kt",
        "package sample.other\n\nclass Renderer\n",
    );
    let usage_contents = "package sample.screen\n\nimport sample.render.Renderer\n\nfun display(renderer: Renderer): String = renderer.render()\n";
    let usage_path = workspace_root.join("src/main/kotlin/sample/screen/Screen.kt");
    write_fixture_file(
        workspace_root,
        "src/main/kotlin/sample/screen/Screen.kt",
        usage_contents,
    );

    let mut client = SpecificationLspClient::spawn(workspace_root);
    client.initialize(workspace_root);
    client.wait_for_indexing();
    let usage_uri = file_uri(&usage_path);
    client.open_document(&usage_uri, usage_contents);

    let definition_response = client.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": usage_uri},
            "position": position_of(usage_contents, "Renderer", 1),
        }),
    );
    let definition_result = &definition_response["result"];
    let definition_location = definition_result
        .as_array()
        .and_then(|locations| locations.first())
        .unwrap_or(definition_result);
    assert_eq!(
        definition_location["uri"],
        file_uri(&workspace_root.join("src/main/kotlin/sample/render/Renderer.kt"))
    );

    let declaration_response = client.request(
        "textDocument/declaration",
        json!({
            "textDocument": {"uri": usage_uri},
            "position": position_of(usage_contents, "Renderer", 1),
        }),
    );
    assert_eq!(
        declaration_response["result"],
        definition_response["result"]
    );

    let hover_response = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": usage_uri},
            "position": position_of(usage_contents, "Renderer", 1),
        }),
    );
    let hover_text = hover_response["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(
        hover_text.contains("Renderer"),
        "hover must describe the imported Renderer: {hover_response}"
    );

    let references_response = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": usage_uri},
            "position": position_of(usage_contents, "Renderer", 1),
            "context": {"includeDeclaration": true},
        }),
    );
    let reference_locations = references_response["result"]
        .as_array()
        .expect("references must return locations");
    assert!(
        reference_locations
            .iter()
            .any(|location| location["uri"] == usage_uri),
        "references must include the imported usage: {references_response}"
    );

    let highlight_response = client.request(
        "textDocument/documentHighlight",
        json!({
            "textDocument": {"uri": usage_uri},
            "position": position_of(usage_contents, "renderer", 0),
        }),
    );
    assert_eq!(
        highlight_response["result"]
            .as_array()
            .expect("document highlights must be returned")
            .len(),
        2
    );
}

#[test]
fn symbol_capabilities_report_nested_and_workspace_declarations() {
    let temporary_directory = tempfile::tempdir().expect("temporary workspace must be created");
    let workspace_root = temporary_directory.path();
    write_fixture_file(workspace_root, "workspace.json", r#"{"sourcePaths":[]}"#);
    let repository_contents = "package sample.data\n\nclass AccountRepository {\n    fun loadAccount(): String = \"ready\"\n}\n";
    let repository_path = workspace_root.join("src/main/kotlin/sample/data/AccountRepository.kt");
    write_fixture_file(
        workspace_root,
        "src/main/kotlin/sample/data/AccountRepository.kt",
        repository_contents,
    );

    let mut client = SpecificationLspClient::spawn(workspace_root);
    client.initialize(workspace_root);
    client.wait_for_indexing();
    let repository_uri = file_uri(&repository_path);
    client.open_document(&repository_uri, repository_contents);

    let document_response = client.request(
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": repository_uri}}),
    );
    let document_symbols = document_response["result"]
        .as_array()
        .expect("document symbols must return an array");
    assert!(
        document_symbols
            .iter()
            .any(|symbol| symbol["name"] == "AccountRepository"),
        "document symbols must contain AccountRepository: {document_response}"
    );

    let workspace_response =
        client.request("workspace/symbol", json!({"query": "AccountRepository"}));
    let workspace_symbols = workspace_response["result"]
        .as_array()
        .expect("workspace symbols must return an array");
    assert_eq!(workspace_symbols.len(), 1);
    assert_eq!(workspace_symbols[0]["name"], "AccountRepository");
    assert_eq!(workspace_symbols[0]["location"]["uri"], repository_uri);
}

#[test]
fn completion_resolve_and_signature_help_preserve_callable_details() {
    let temporary_directory = tempfile::tempdir().expect("temporary workspace must be created");
    let workspace_root = temporary_directory.path();
    write_fixture_file(workspace_root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture_file(
        workspace_root,
        "src/main/kotlin/sample/api/PaymentService.kt",
        "package sample.api\n\n/** Processes a neutral fixture payment. */\nclass PaymentService\n\nfun submitPayment(amount: Int, label: String): Boolean = true\n",
    );
    let usage_contents = "package sample.screen\n\nimport sample.api.submitPayment\n\nfun screen() {\n    Pay\n    submitPayment(1, \"demo\")\n}\n";
    let usage_path = workspace_root.join("src/main/kotlin/sample/screen/PaymentScreen.kt");
    write_fixture_file(
        workspace_root,
        "src/main/kotlin/sample/screen/PaymentScreen.kt",
        usage_contents,
    );

    let mut client = SpecificationLspClient::spawn(workspace_root);
    client.initialize(workspace_root);
    client.wait_for_indexing();
    let usage_uri = file_uri(&usage_path);
    client.open_document(&usage_uri, usage_contents);

    let completion_position = position_of(usage_contents, "Pay", 1);
    let completion_response = client.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": usage_uri},
            "position": {
                "line": completion_position["line"],
                "character": completion_position["character"].as_u64().unwrap() + 3,
            },
        }),
    );
    let payment_item = completion_items(&completion_response)
        .into_iter()
        .find(|item| item["label"] == "PaymentService")
        .unwrap_or_else(|| panic!("completion must include PaymentService: {completion_response}"));
    let resolved_response = client.request("completionItem/resolve", payment_item);
    assert_eq!(resolved_response["result"]["label"], "PaymentService");
    assert_eq!(resolved_response["result"]["detail"], "sample.api");
    assert_eq!(
        resolved_response["result"]["additionalTextEdits"][0]["newText"],
        "import sample.api.PaymentService\n"
    );

    let call_position = position_of(usage_contents, "submitPayment", 1);
    let signature_response = client.request(
        "textDocument/signatureHelp",
        json!({
            "textDocument": {"uri": usage_uri},
            "position": {
                "line": call_position["line"],
                "character": call_position["character"].as_u64().unwrap() + 16,
            },
        }),
    );
    let signatures = signature_response["result"]["signatures"]
        .as_array()
        .expect("signature help must return signatures");
    assert_eq!(signatures.len(), 1);
    assert!(
        signatures[0]["label"]
            .as_str()
            .is_some_and(|label| label.contains("amount: Int") && label.contains("label: String")),
        "signature help must expose both parameters: {signature_response}"
    );
}
