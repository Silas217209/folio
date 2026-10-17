use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use lsp_types::{Diagnostic, Position, Range, TextEdit, WorkspaceEdit};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct Snapshot {
    pub text: String,
    pub version: i32,
}

pub enum Event {
    Ready,
    Diagnostics {
        uri: String,
        version: Option<i32>,
        diagnostics: Vec<Diagnostic>,
    },
    ApplyEdit {
        edit: WorkspaceEdit,
        reply: async_channel::Sender<std::result::Result<(), String>>,
    },
    Error(String),
}

enum Request {
    Notify(&'static str, Value),
    Query(
        u64,
        &'static str,
        Value,
        async_channel::Sender<Result<Value, String>>,
    ),
    Cancel(u64),
    Stop,
}

struct ClientInner {
    sender: mpsc::Sender<Request>,
    documents: Arc<Mutex<HashMap<String, Snapshot>>>,
    capabilities: Arc<RwLock<Value>>,
    diagnostics: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>,
    events: async_channel::Sender<Event>,
    next_id: AtomicU64,
}

impl Drop for ClientInner {
    fn drop(&mut self) {
        let _ = self.sender.send(Request::Stop);
    }
}

#[derive(Clone)]
pub struct Client(Arc<ClientInner>);

impl Client {
    #[cfg(test)]
    pub fn start(root: PathBuf) -> (Self, async_channel::Receiver<Event>) {
        Self::start_entry(root, "main.typ".into())
    }

    pub fn start_entry(root: PathBuf, entry: String) -> (Self, async_channel::Receiver<Event>) {
        let (sender, requests) = mpsc::channel();
        let (events, receiver) = async_channel::unbounded();
        let capabilities = Arc::new(RwLock::new(Value::Null));
        let event_sender = events.clone();
        let server_capabilities = capabilities.clone();
        thread::Builder::new()
            .name("tinymist-lsp".into())
            .spawn(move || {
                if let Err(error) =
                    run(&root, &entry, requests, &event_sender, &server_capabilities)
                {
                    let _ = event_sender
                        .try_send(Event::Error(format!("Tinymist language server: {error:#}")));
                }
            })
            .expect("could not start language server thread");
        (
            Self(Arc::new(ClientInner {
                sender,
                documents: Arc::default(),
                capabilities,
                diagnostics: Arc::default(),
                events,
                next_id: AtomicU64::new(1),
            })),
            receiver,
        )
    }

    /// Called by providers too: completion can run before InputEvent::Change.
    /// Keep sync and the request in FIFO order without doing I/O on the UI thread.
    pub fn sync(&self, uri: &str, text: &str) {
        let mut documents = self.0.documents.lock().unwrap();
        if let Some(snapshot) = documents.get_mut(uri) {
            if snapshot.text == text {
                return;
            }
            snapshot.text = text.to_owned();
            snapshot.version += 1;
            self.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": snapshot.version },
                    "contentChanges": [{ "text": text }],
                }),
            );
        } else {
            documents.insert(
                uri.into(),
                Snapshot {
                    text: text.into(),
                    version: 1,
                },
            );
            self.notify("textDocument/didOpen", json!({
                "textDocument": { "uri": uri, "version": 1, "languageId": "typst", "text": text },
            }));
        }
    }

    pub fn snapshot(&self, uri: &str) -> Option<Snapshot> {
        self.0.documents.lock().unwrap().get(uri).cloned()
    }
    pub fn snapshots(&self) -> HashMap<String, Snapshot> {
        self.0.documents.lock().unwrap().clone()
    }
    pub fn capabilities(&self) -> Value {
        self.0.capabilities.read().unwrap().clone()
    }
    pub fn diagnostics(&self, uri: &str) -> Vec<Diagnostic> {
        self.0
            .diagnostics
            .lock()
            .unwrap()
            .get(uri)
            .cloned()
            .unwrap_or_default()
    }
    pub fn set_diagnostics(&self, uri: String, diagnostics: Vec<Diagnostic>) {
        self.0.diagnostics.lock().unwrap().insert(uri, diagnostics);
    }
    pub fn all_diagnostics(&self) -> Vec<(String, Diagnostic)> {
        let mut result: Vec<_> = self
            .0
            .diagnostics
            .lock()
            .unwrap()
            .iter()
            .flat_map(|(uri, entries)| entries.iter().cloned().map(|entry| (uri.clone(), entry)))
            .collect();
        result.sort_by(|(a, x), (b, y)| {
            a.cmp(b)
                .then_with(|| x.range.start.line.cmp(&y.range.start.line))
                .then_with(|| x.range.start.character.cmp(&y.range.start.character))
        });
        result
    }
    pub fn notify(&self, method: &'static str, params: Value) {
        if self.0.sender.send(Request::Notify(method, params)).is_err() {
            let _ = self.0.events.try_send(Event::Error(format!(
                "Language server is unavailable ({method})"
            )));
        }
    }

    pub async fn request(&self, method: &'static str, params: Value) -> Result<Value> {
        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, response) = async_channel::bounded(1);
        self.0
            .sender
            .send(Request::Query(id, method, params, reply))
            .context("language server is unavailable")?;
        struct Cancel {
            id: u64,
            sender: mpsc::Sender<Request>,
            finished: bool,
        }
        impl Drop for Cancel {
            fn drop(&mut self) {
                if !self.finished {
                    let _ = self.sender.send(Request::Cancel(self.id));
                }
            }
        }
        let mut cancel = Cancel {
            id,
            sender: self.0.sender.clone(),
            finished: false,
        };
        let response = response.recv().await.context("language server stopped")?;
        cancel.finished = true;
        response.map_err(|error| anyhow!(error))
    }

    pub async fn apply_edit(&self, edit: WorkspaceEdit) -> Result<()> {
        let (reply, response) = async_channel::bounded(1);
        self.0.events.send(Event::ApplyEdit { edit, reply }).await?;
        response.recv().await?.map_err(|error| anyhow!(error))
    }
}

pub fn file_uri(path: &Path) -> String {
    url::Url::from_file_path(path)
        .expect("absolute document path")
        .to_string()
}

pub fn uri_path(uri: &str) -> Result<PathBuf> {
    url::Url::parse(uri)?
        .to_file_path()
        .map_err(|_| anyhow!("Cannot open non-file location: {uri}"))
}

/// LSP columns are UTF-16; GPUI Kit's Position columns count Unicode scalars.
pub fn byte_position(text: &str, offset: usize) -> Position {
    let prefix = &text[..offset.min(text.len())];
    let line = prefix.bytes().filter(|&byte| byte == b'\n').count() as u32;
    let character = prefix
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .encode_utf16()
        .count() as u32;
    Position::new(line, character)
}

pub fn position_byte(text: &str, position: Position) -> Result<usize> {
    let start = if position.line == 0 {
        0
    } else {
        text.match_indices('\n')
            .nth(position.line as usize - 1)
            .map(|(offset, _)| offset + 1)
            .context("LSP line is out of range")?
    };
    let line = text[start..]
        .split('\n')
        .next()
        .unwrap_or("")
        .trim_end_matches('\r');
    let mut units = 0;
    for (offset, ch) in line.char_indices() {
        if units == position.character {
            return Ok(start + offset);
        }
        units += ch.len_utf16() as u32;
        if units > position.character {
            bail!("LSP position splits a UTF-16 surrogate pair");
        }
    }
    if units == position.character {
        Ok(start + line.len())
    } else {
        bail!("LSP column is out of range")
    }
}

pub fn editor_position(text: &str, position: Position) -> Position {
    let offset = position_byte(text, position).unwrap_or(text.len());
    let column = text[..offset]
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .chars()
        .count() as u32;
    Position::new(position.line, column)
}

pub fn editor_range(text: &str, range: Range) -> Range {
    Range::new(
        editor_position(text, range.start),
        editor_position(text, range.end),
    )
}

/// Validate every edit before mutating any buffer. Apply in reverse source order
/// so multi-edit formatting and renames use the same original document snapshot.
pub fn edited_text(text: &str, edits: &[TextEdit]) -> Result<String> {
    let mut edits = edits
        .iter()
        .map(|edit| {
            Ok((
                position_byte(text, edit.range.start)?,
                position_byte(text, edit.range.end)?,
                edit.new_text.as_str(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    edits.sort_by_key(|edit| (edit.0, edit.1));
    let mut end = 0;
    for &(start, next_end, _) in &edits {
        if start < end || start > next_end {
            bail!("Overlapping or reversed language server edits");
        }
        end = next_end;
    }
    let mut result = text.to_owned();
    for (start, end, replacement) in edits.into_iter().rev() {
        result.replace_range(start..end, replacement);
    }
    Ok(result)
}

fn write_message(writer: &mut impl Write, value: Value) -> Result<()> {
    let bytes = serde_json::to_vec(&value)?;
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    // Tinymist 0.15.10 can retain old imported-file snapshots when document
    // messages arrive in one tight burst. Pace frames on this I/O thread; the
    // editor and preview never wait here, and no editing events are discarded.
    thread::sleep(Duration::from_millis(1));
    Ok(())
}

fn read_message(reader: &mut impl BufRead) -> Result<Value> {
    let mut length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            bail!("language server closed stdout");
        }
        if header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("Content-Length")
        {
            length = Some(value.trim().parse::<usize>()?);
        }
    }
    let length = length.context("Missing LSP Content-Length header")?;
    if length > 32 * 1024 * 1024 {
        bail!("Language server message exceeds 32 MiB");
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    let value = serde_json::from_slice(&bytes)?;
    Ok(value)
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run(
    root: &Path,
    entry: &str,
    requests: mpsc::Receiver<Request>,
    events: &async_channel::Sender<Event>,
    capabilities: &RwLock<Value>,
) -> Result<()> {
    let mut server = Server(
        crate::runtime::tinymist_command()
            .arg("lsp")
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("could not start tinymist lsp")?,
    );
    let mut input = server.0.stdin.take().unwrap();
    let output = server.0.stdout.take().unwrap();
    let (messages, received) = mpsc::channel();
    thread::spawn(move || {
        let mut output = BufReader::new(output);
        loop {
            let message = read_message(&mut output).map_err(|error| error.to_string());
            let failed = message.is_err();
            if messages.send(message).is_err() || failed {
                break;
            }
        }
    });
    let settings = json!({
        "rootPath": root, "exportPdf": "never",
        "formatterMode": "typstyle", "semanticTokens": "disable"
    });
    write_message(
        &mut input,
        json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "processId": std::process::id(), "rootUri": file_uri(root),
            "workspaceFolders": [{ "uri": file_uri(root), "name": root.file_name().unwrap_or_default().to_string_lossy() }],
            "initializationOptions": settings,
            "capabilities": {
                "general": { "positionEncodings": ["utf-16"] },
                "workspace": { "workspaceFolders": true, "configuration": true, "applyEdit": true,
                    "workspaceEdit": { "documentChanges": true } },
                "window": { "showDocument": { "support": false } },
                "textDocument": {
                    "synchronization": { "didSave": true },
                    "completion": { "completionItem": { "snippetSupport": false, "insertReplaceSupport": true,
                        "documentationFormat": ["markdown", "plaintext"] } },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "definition": { "linkSupport": true },
                    "publishDiagnostics": { "relatedInformation": true, "versionSupport": true },
                    "codeAction": { "codeActionLiteralSupport": { "codeActionKind": { "valueSet": ["", "quickfix", "refactor", "source"] } },
                        "resolveSupport": { "properties": ["edit", "command"] } },
                    "rename": { "prepareSupport": true },
                },
            },
        }}),
    )?;
    type Reply = async_channel::Sender<Result<Value, String>>;
    let mut pending: HashMap<u64, (Reply, Instant)> = HashMap::new();
    let mut edits = Vec::<(Value, async_channel::Receiver<Result<(), String>>)>::new();
    let mut initialized = false;
    let mut entry_selected = false;
    let startup = Instant::now();
    loop {
        // Wait for initialize before sending document notifications or queries.
        if initialized {
            if let Ok(request) = requests.try_recv() {
                match request {
                    Request::Stop => {
                        write_message(
                            &mut input,
                            json!({ "jsonrpc": "2.0", "id": -2, "method": "shutdown", "params": null }),
                        )?;
                        write_message(&mut input, json!({ "jsonrpc": "2.0", "method": "exit" }))?;
                        return Ok(());
                    }
                    Request::Notify(method, params) => {
                        write_message(
                            &mut input,
                            json!({ "jsonrpc": "2.0", "method": method, "params": params }),
                        )?;
                    }
                    Request::Query(id, method, params, reply) => {
                        if reply.is_closed() {
                            continue;
                        }
                        write_message(
                            &mut input,
                            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
                        )?;
                        pending.insert(id, (reply, Instant::now()));
                    }
                    Request::Cancel(id) => {
                        pending.remove(&id);
                        write_message(
                            &mut input,
                            json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": id } }),
                        )?;
                    }
                }
            }
            if !entry_selected {
                // Open initial unsaved buffers before selecting the entry point.
                write_message(
                    &mut input,
                    json!({ "jsonrpc": "2.0", "id": -1, "method": "workspace/executeCommand", "params": { "command": "tinymist.focusMain", "arguments": [root.join(entry)] } }),
                )?;
                entry_selected = true;
            }
        } else if !initialized && startup.elapsed() > Duration::from_secs(15) {
            bail!("initialization timed out");
        }
        match received.recv_timeout(Duration::from_millis(2)) {
            Ok(Ok(message)) => {
                if let Some(method) = message.get("method").and_then(Value::as_str) {
                    let params = &message["params"];
                    if let Some(id) = message.get("id") {
                        let response = match method {
                            "workspace/configuration" => Some(
                                json!({ "result": params["items"].as_array().unwrap_or(&vec![]).iter().map(|item| {
                                match item["section"].as_str() { Some("tinymist") => settings.clone(), Some(section) => settings.get(section.strip_prefix("tinymist.").unwrap_or(section)).cloned().unwrap_or(Value::Null), None => settings.clone() }
                            }).collect::<Vec<_>>() }),
                            ),
                            "client/registerCapability"
                            | "client/unregisterCapability"
                            | "window/workDoneProgress/create" => Some(json!({ "result": null })),
                            "workspace/applyEdit" => {
                                let (reply, response) = async_channel::bounded(1);
                                events.try_send(Event::ApplyEdit {
                                    edit: serde_json::from_value(params["edit"].clone())?,
                                    reply,
                                })?;
                                edits.push((id.clone(), response));
                                None
                            }
                            _ => Some(
                                json!({ "error": { "code": -32601, "message": "Client method is not supported" } }),
                            ),
                        };
                        if let Some(mut response) = response {
                            response["jsonrpc"] = json!("2.0");
                            response["id"] = id.clone();
                            write_message(&mut input, response)?;
                        }
                    } else if method == "textDocument/publishDiagnostics" {
                        if let Ok(params) = serde_json::from_value::<
                            lsp_types::PublishDiagnosticsParams,
                        >(params.clone())
                        {
                            let _ = events.try_send(Event::Diagnostics {
                                uri: params.uri.to_string(),
                                version: params.version,
                                diagnostics: params.diagnostics,
                            });
                        }
                    } else if method == "window/showMessage" && params["type"] == 1 {
                        let _ = events.try_send(Event::Error(
                            params["message"]
                                .as_str()
                                .unwrap_or("Language server error")
                                .to_owned(),
                        ));
                    }
                } else if message["id"] == 0 && !initialized {
                    if let Some(error) = message.get("error") {
                        bail!("initialize failed: {error}");
                    }
                    *capabilities.write().unwrap() = message["result"]["capabilities"].clone();
                    write_message(
                        &mut input,
                        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
                    )?;
                    initialized = true;
                } else if message["id"] == -1 {
                    if message.get("error").is_some() {
                        bail!(
                            "Could not select the project entry point: {}",
                            message["error"]
                        );
                    }
                    let _ = events.try_send(Event::Ready);
                } else if let Some(id) = message["id"].as_u64()
                    && let Some((reply, _)) = pending.remove(&id)
                {
                    let result = if let Some(error) = message.get("error") {
                        Err(error["message"]
                            .as_str()
                            .unwrap_or("Language server request failed")
                            .into())
                    } else {
                        Ok(message["result"].clone())
                    };
                    let _ = reply.try_send(result);
                }
            }
            Ok(Err(error)) => bail!("{error}"),
            Err(mpsc::RecvTimeoutError::Disconnected) => bail!("language server transport stopped"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        pending.retain(|_, (reply, sent)| {
            if sent.elapsed() < Duration::from_secs(15) {
                return !reply.is_closed();
            }
            let _ = reply.try_send(Err("Language server request timed out".into()));
            false
        });
        for index in (0..edits.len()).rev() {
            if let Ok(result) = edits[index].1.try_recv() {
                let (id, _) = edits.swap_remove(index);
                write_message(
                    &mut input,
                    json!({ "jsonrpc": "2.0", "id": id, "result": { "applied": result.is_ok(), "failureReason": result.err() } }),
                )?;
            }
        }
        if let Some(status) = server.0.try_wait()? {
            bail!("language server exited: {status}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_lsp_notifications_and_requests_report_errors() {
        let (sender, requests) = mpsc::channel();
        drop(requests);
        let (events, receiver) = async_channel::unbounded();
        let client = Client(Arc::new(ClientInner {
            sender,
            documents: Arc::default(),
            capabilities: Arc::default(),
            diagnostics: Arc::default(),
            events,
            next_id: AtomicU64::new(1),
        }));
        client.sync("file:///test.typ", "hello");
        client.notify("textDocument/didSave", Value::Null);
        for _ in 0..2 {
            assert!(
                matches!(receiver.try_recv().unwrap(), Event::Error(message) if message.contains("unavailable"))
            );
        }
        assert!(futures_lite::future::block_on(client.request("test", Value::Null)).is_err());
    }

    #[test]
    fn unicode_positions_and_multi_edit_validation() {
        let text = "α😀界 name\r\nsecond";
        let offset = text.find("name").unwrap();
        assert_eq!(byte_position(text, offset), Position::new(0, 5));
        assert_eq!(position_byte(text, Position::new(0, 5)).unwrap(), offset);
        assert_eq!(
            editor_position(text, Position::new(0, 5)),
            Position::new(0, 4)
        );
        assert!(position_byte(text, Position::new(0, 2)).is_err());
        assert_eq!(
            position_byte(text, Position::new(1, 2)).unwrap(),
            text.find("second").unwrap() + 2
        );
        let edits = vec![
            TextEdit::new(
                Range::new(Position::new(1, 0), Position::new(1, 6)),
                "next".into(),
            ),
            TextEdit::new(
                Range::new(Position::new(0, 5), Position::new(0, 9)),
                "title".into(),
            ),
        ];
        assert_eq!(edited_text(text, &edits).unwrap(), "α😀界 title\r\nnext");
        assert!(edited_text(text, &[edits[0].clone(), edits[0].clone()]).is_err());
    }

    fn wait_event(
        events: &async_channel::Receiver<Event>,
        mut predicate: impl FnMut(Event) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            while let Ok(event) = events.try_recv() {
                if let Event::Error(error) = &event {
                    panic!("{error}");
                }
                if predicate(event) {
                    return;
                }
            }
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for Tinymist LSP event"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn nested_entry_resolves_shared_imports_from_the_library_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("documents/Rechnung")).unwrap();
        std::fs::create_dir(root.path().join("lib")).unwrap();
        let entry = "documents/Rechnung/Rechnung_1_Test.typ";
        let source = "#import \"/lib/helpers.typ\": greet\n#greet(\"Ada\")\n";
        std::fs::write(root.path().join(entry), source).unwrap();
        std::fs::write(
            root.path().join("lib/helpers.typ"),
            "#let greet(name) = [Hello #name]",
        )
        .unwrap();
        let uri = file_uri(&root.path().join(entry));
        let (client, events) = Client::start_entry(root.path().to_owned(), entry.into());
        client.sync(&uri, source);
        wait_event(&events, |event| matches!(event, Event::Ready));
        let position =
            json!({ "textDocument": { "uri": uri }, "position": { "line": 1, "character": 3 } });
        let hover =
            futures_lite::future::block_on(client.request("textDocument/hover", position.clone()))
                .unwrap();
        assert!(!hover.is_null(), "Shared import was not resolved: {hover}");
        let definition =
            futures_lite::future::block_on(client.request("textDocument/definition", position))
                .unwrap();
        assert!(
            definition.to_string().contains("lib/helpers.typ"),
            "{definition}"
        );
    }

    #[test]
    fn real_tinymist_completes_navigates_renames_formats_and_publishes_diagnostics() {
        let root = tempfile::tempdir().unwrap();
        let helpers = "#let greet(name) = [Hello #name]\n";
        let main = "#import \"helpers.typ\": greet\n#let emoji = \"😀\"\n#greet(\"Ada\")\n";
        std::fs::write(root.path().join("main.typ"), main).unwrap();
        std::fs::write(root.path().join("helpers.typ"), "Original disk source").unwrap();
        let uri = file_uri(&root.path().join("main.typ"));
        let helper_uri = file_uri(&root.path().join("helpers.typ"));
        let (client, events) = Client::start(root.path().to_owned());
        client.sync(&uri, main);
        client.sync(&helper_uri, helpers);
        wait_event(&events, |event| matches!(event, Event::Ready));
        let query = |method, params| {
            futures_lite::future::block_on(client.request(method, params)).unwrap()
        };
        client.sync(&uri, &format!("{main}#tex"));
        let completions = query(
            "textDocument/completion",
            json!({ "textDocument": { "uri": uri }, "position": { "line": 3, "character": 4 }, "context": { "triggerKind": 1 } }),
        );
        let response: lsp_types::CompletionResponse = serde_json::from_value(completions).unwrap();
        let items = match response {
            lsp_types::CompletionResponse::Array(items) => items,
            lsp_types::CompletionResponse::List(list) => list.items,
        };
        assert!(
            items.iter().any(|item| item.label == "text"),
            "Missing text completion: {items:?}"
        );
        client.sync(&uri, main);
        let position =
            json!({ "textDocument": { "uri": uri }, "position": { "line": 2, "character": 3 } });
        let hover = query("textDocument/hover", position.clone());
        assert!(
            !hover.is_null(),
            "Missing hover for an unsaved imported function"
        );
        let definition = query("textDocument/definition", position.clone());
        assert!(
            definition.to_string().contains("helpers.typ"),
            "{definition}"
        );
        client.sync(
            &helper_uri,
            "#let greet(name, punctuation: \"!\") = [Hello #name#punctuation]\n",
        );
        let updated_hover = query("textDocument/hover", position.clone());
        assert!(
            updated_hover.to_string().contains("punctuation"),
            "Unsaved imported module did not update: {updated_hover}"
        );
        let references = query(
            "textDocument/references",
            json!({ "textDocument": { "uri": uri }, "position": { "line": 2, "character": 3 }, "context": { "includeDeclaration": true } }),
        );
        assert!(!references.as_array().unwrap().is_empty());
        let prepare = query("textDocument/prepareRename", position.clone());
        assert!(!prepare.is_null());
        let rename: WorkspaceEdit = serde_json::from_value(query("textDocument/rename", json!({ "textDocument": { "uri": uri }, "position": { "line": 2, "character": 3 }, "newName": "salute" }))).unwrap();
        assert!(serde_json::to_string(&rename).unwrap().contains("salute"));
        client.sync(&uri, &format!("{main}#text(size: 12pt)[Hello]"));
        let help = query(
            "textDocument/signatureHelp",
            json!({ "textDocument": { "uri": uri }, "position": { "line": 3, "character": 10 } }),
        );
        assert!(!help.is_null());
        let bad = format!("{main}#this_symbol_does_not_exist\n");
        client.sync(&uri, &bad);
        let version = client.snapshot(&uri).unwrap().version;
        wait_event(&events, |event| {
            matches!(event, Event::Diagnostics { uri: diagnostic_uri, version: diagnostic_version, diagnostics }
            if diagnostic_uri == uri && diagnostic_version.is_none_or(|value| value == version) && diagnostics.iter().any(|diagnostic| diagnostic.message.contains("this_symbol_does_not_exist")))
        });
        client.sync(&uri, main);
        let version = client.snapshot(&uri).unwrap().version;
        wait_event(&events, |event| {
            matches!(event, Event::Diagnostics { uri: diagnostic_uri, version: diagnostic_version, diagnostics }
            if diagnostic_uri == uri && diagnostic_version.is_none_or(|value| value == version) && diagnostics.is_empty())
        });
        let unformatted = "#let   value= (1,2)\n#value\n";
        client.sync(&uri, unformatted);
        let formatting = query(
            "textDocument/formatting",
            json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 2, "insertSpaces": true } }),
        );
        let edits: Vec<TextEdit> = serde_json::from_value(formatting).unwrap();
        let formatted = edited_text(unformatted, &edits).unwrap();
        assert_ne!(unformatted, formatted);
        assert!(formatted.contains("#let value ="), "{formatted}");
        assert_eq!(
            std::fs::read_to_string(root.path().join("main.typ")).unwrap(),
            main
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("helpers.typ")).unwrap(),
            "Original disk source"
        );
    }
}
