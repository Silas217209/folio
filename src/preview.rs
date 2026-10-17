use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader},
    net::TcpStream,
    path::PathBuf,
    process::{Child, Stdio},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use reflexo::{
    hash::Fingerprint,
    vector::{incr::IncrDocClient, ir::Page, stream::BytesModuleStream},
};
use reflexo_vec2svg::{SvgExportFeature, SvgExporter, SvgText};
use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

pub use crate::runtime::TINYMIST_VERSION;

pub struct VectorPage {
    pub id: Fingerprint,
    pub tree: resvg::usvg::Tree,
}

impl VectorPage {
    pub fn width(&self) -> f32 {
        self.tree.size().width()
    }
    pub fn height(&self) -> f32 {
        self.tree.size().height()
    }
}

pub struct CompiledPreview {
    pub pages: Vec<Arc<VectorPage>>,
}

#[derive(Clone, Debug)]
pub struct EditLocation {
    pub revision: u64,
    pub path: PathBuf,
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct PreviewLocation {
    pub revision: u64,
    pub page: usize,
    pub x: f32,
    pub y: f32,
}

pub enum PreviewEvent {
    Document(CompiledPreview),
    Follow(PreviewLocation),
    Error(String),
    Compiling,
    Compiled,
}

enum Request {
    Update(PathBuf, String, Option<EditLocation>),
    Refresh,
    Stop,
}

/// One long-lived compiler and Reflexo document per workspace. Editing never
/// writes files, starts a compiler process, or resets the vector cache.
pub struct PreviewService {
    sender: mpsc::Sender<Request>,
    events: async_channel::Sender<PreviewEvent>,
}

impl PreviewService {
    #[cfg(test)]
    pub fn start(
        root: PathBuf,
        files: Vec<(PathBuf, String)>,
    ) -> (Self, async_channel::Receiver<PreviewEvent>) {
        Self::start_entry(root, "main.typ".into(), files)
    }

    pub fn start_entry(
        root: PathBuf,
        entry: String,
        files: Vec<(PathBuf, String)>,
    ) -> (Self, async_channel::Receiver<PreviewEvent>) {
        let (sender, requests) = mpsc::channel();
        let (events, receiver) = async_channel::bounded(8);
        let worker_events = events.clone();
        thread::Builder::new()
            .name("tinymist-preview".into())
            .spawn(move || {
                if let Err(error) = run(root, entry, files, requests, &worker_events) {
                    let _ = worker_events.send_blocking(PreviewEvent::Error(error));
                }
            })
            .expect("could not start preview thread");
        (Self { sender, events }, receiver)
    }

    pub fn update(&self, path: PathBuf, text: String, location: Option<EditLocation>) {
        self.send(Request::Update(path, text, location));
    }

    fn send(&self, request: Request) {
        if self.sender.send(request).is_err() {
            let _ = self.events.try_send(PreviewEvent::Error(
                "Preview compiler is unavailable".into(),
            ));
        }
    }

    pub fn refresh(&self) {
        self.send(Request::Refresh);
    }
}

impl Drop for PreviewService {
    fn drop(&mut self) {
        let _ = self.sender.send(Request::Stop);
    }
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

type Socket = WebSocket<TcpStream>;
type PageKey = (Fingerprint, u32, u32);

fn connect(address: &str) -> Result<Socket, String> {
    let stream = TcpStream::connect(address).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let (socket, _) = tungstenite::client(format!("ws://{address}/"), stream)
        .map_err(|error| error.to_string())?;
    socket
        .get_ref()
        .set_read_timeout(None)
        .map_err(|error| error.to_string())?;
    socket
        .get_ref()
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    Ok(socket)
}

fn would_block(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(error) if error.kind() == io::ErrorKind::WouldBlock)
}

fn write(socket: &mut Socket, message: Message) -> Result<(), String> {
    match socket.send(message) {
        Ok(()) => Ok(()),
        Err(error) if would_block(&error) => Ok(()), // tungstenite retains buffered writes
        Err(error) => Err(error.to_string()),
    }
}

fn run(
    root: PathBuf,
    entry: String,
    files: Vec<(PathBuf, String)>,
    requests: mpsc::Receiver<Request>,
    events: &async_channel::Sender<PreviewEvent>,
) -> Result<(), String> {
    let child = crate::runtime::tinymist_command()
        .arg("preview").arg(root.join(entry))
        .arg("--root").arg(&root)
        .args(["--no-open", "--data-plane-host", "127.0.0.1:0", "--control-plane-host", "127.0.0.1:0"])
        .current_dir(&root).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()
        .map_err(|error| format!("Could not start Tinymist: {error}. Run scripts/install-tinymist.sh or set TINYMIST to a {TINYMIST_VERSION} binary."))?;
    let mut server = Server(child);
    let stderr = server.0.stderr.take().unwrap();
    let (logs_tx, logs) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if logs_tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let (mut control_address, mut data_address) = (None, None);
    let mut initial: HashMap<_, _> = files.into_iter().collect();
    let mut pending_location = None;
    while control_address.is_none() || data_address.is_none() {
        while let Ok(request) = requests.try_recv() {
            match request {
                Request::Stop => return Ok(()),
                Request::Update(path, text, location) => {
                    initial.insert(path, text);
                    pending_location = location;
                }
                Request::Refresh => {}
            }
        }
        if Instant::now() > deadline {
            return Err("Tinymist did not start its preview server within 15 seconds.".into());
        }
        if let Some(status) = server.0.try_wait().map_err(|error| error.to_string())? {
            return Err(format!("Tinymist exited during startup: {status}"));
        }
        if let Ok(line) = logs.recv_timeout(Duration::from_millis(20)) {
            if let Some((_, address)) = line.split_once("Control panel server listening on: ") {
                control_address = Some(address.trim().to_owned());
            }
            if let Some((_, address)) = line.split_once("Data plane server listening on: ") {
                data_address = Some(address.trim().to_owned());
            }
        }
    }
    let mut control = connect(&control_address.unwrap())?;
    let mut data = connect(&data_address.unwrap())?;
    let initial: HashMap<_, _> = initial
        .into_iter()
        .map(|(path, text)| (root.join(path), text))
        .collect();
    write(
        &mut control,
        Message::text(json!({"event":"syncMemoryFiles", "files":initial}).to_string()),
    )?;
    write(&mut data, Message::text("current"))?;
    let mut doc = IncrDocClient::default();
    let mut pages: HashMap<PageKey, Arc<VectorPage>> = HashMap::new();
    let mut diagnostics = String::new();
    let mut compiled = false;
    let mut frame_received = false;
    let mut latest_location = pending_location.clone();
    let mut document_signature = Vec::new();
    // Jump replies carry no request ID. Keep one query in flight and tag its
    // result with the originating edit revision so the UI can reject old edits.
    let mut in_flight: Option<(EditLocation, Instant)> = None;
    loop {
        let mut edits = HashMap::new();
        while let Ok(request) = requests.try_recv() {
            match request {
                Request::Stop => return Ok(()),
                Request::Update(path, text, location) => {
                    edits.insert(root.join(path), text);
                    latest_location = location.clone();
                    pending_location = location;
                    compiled = false;
                    frame_received = false;
                }
                Request::Refresh => write(&mut data, Message::text("current"))?,
            }
        }
        if !edits.is_empty() {
            write(
                &mut control,
                Message::text(json!({"event":"updateMemoryFiles", "files":edits}).to_string()),
            )?;
        }
        for socket in [&mut control, &mut data] {
            if let Err(error) = socket.flush()
                && !would_block(&error)
            {
                return Err(error.to_string());
            }
        }
        while let Ok(line) = logs.try_recv() {
            if line.contains("error:") {
                diagnostics = line.replace(&format!("{}/", root.display()), "");
            }
        }
        loop {
            match control.read() {
                Ok(Message::Text(text)) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        match value.get("kind").and_then(Value::as_str) {
                            Some("Compiling") => {
                                compiled = false;
                                let _ = events.try_send(PreviewEvent::Compiling);
                            }
                            Some("CompileSuccess") => {
                                compiled = true;
                                events
                                    .send_blocking(PreviewEvent::Compiled)
                                    .map_err(|error| error.to_string())?;
                            }
                            Some("CompileError") => {
                                compiled = false;
                                let message = if diagnostics.is_empty() {
                                    "Compilation failed.".into()
                                } else {
                                    diagnostics.clone()
                                };
                                events
                                    .send_blocking(PreviewEvent::Error(message))
                                    .map_err(|error| error.to_string())?;
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    return Err("Tinymist closed the control connection.".into());
                }
                Ok(_) => {}
                Err(error) if would_block(&error) => break,
                Err(error) => return Err(error.to_string()),
            }
        }
        loop {
            match data.read() {
                Ok(Message::Binary(bytes)) => {
                    if bytes.starts_with(b"jump,") {
                        if let Some((location, _)) = in_flight.take()
                            && let Some(position) = parse_jump(&bytes, location.revision)
                        {
                            events
                                .send_blocking(PreviewEvent::Follow(position))
                                .map_err(|error| error.to_string())?;
                        }
                    } else if apply_frame(&mut doc, &bytes)? {
                        let next = render_pages(&doc, &mut pages)?;
                        let signature: Vec<_> = next
                            .pages
                            .iter()
                            .map(|page| (page.id, page.width().to_bits(), page.height().to_bits()))
                            .collect();
                        if signature != document_signature {
                            // Compilation can overtake a pending location query
                            // during fast typing. Resolve the latest edit again
                            // when the rendered document actually changes.
                            pending_location = latest_location.clone();
                            document_signature = signature;
                        }
                        frame_received = true;
                        events
                            .send_blocking(PreviewEvent::Document(next))
                            .map_err(|error| error.to_string())?;
                    }
                }
                Ok(Message::Close(_)) => {
                    return Err("Tinymist closed the preview connection.".into());
                }
                Ok(_) => {}
                Err(error) if would_block(&error) => break,
                Err(error) => return Err(error.to_string()),
            }
        }
        // Comments and whitespace can have no rendered position and therefore
        // no reply. Allow a subsequent edit to request a location in that case.
        if in_flight
            .as_ref()
            .is_some_and(|(_, sent)| sent.elapsed() > Duration::from_millis(350))
        {
            in_flight = None;
        }
        if compiled
            && frame_received
            && in_flight.is_none()
            && let Some(location) = pending_location.take()
        {
            write(
                &mut control,
                Message::text(
                    json!({
                        "event": "panelScrollTo", "filepath": root.join(&location.path),
                        "line": location.line, "character": location.character,
                    })
                    .to_string(),
                ),
            )?;
            in_flight = Some((location, Instant::now()));
        }
        if let Some(status) = server.0.try_wait().map_err(|error| error.to_string())? {
            return Err(format!("Tinymist exited: {status}"));
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn parse_jump(bytes: &[u8], revision: u64) -> Option<PreviewLocation> {
    let payload = std::str::from_utf8(bytes.strip_prefix(b"jump,")?).ok()?;
    let mut fields = payload.split(',').next()?.split_whitespace();
    let page = fields.next()?.parse::<usize>().ok()?.checked_sub(1)?;
    let x = fields.next()?.parse::<f32>().ok()?;
    let y = fields.next()?.parse::<f32>().ok()?;
    (x.is_finite() && y.is_finite()).then_some(PreviewLocation {
        revision,
        page,
        x,
        y,
    })
}

fn apply_frame(doc: &mut IncrDocClient, bytes: &[u8]) -> Result<bool, String> {
    let payload = if let Some(payload) = bytes.strip_prefix(b"diff-v1,") {
        payload
    } else if let Some(payload) = bytes.strip_prefix(b"new,") {
        *doc = IncrDocClient::default();
        payload
    } else {
        return Ok(false);
    };
    // Upstream validates rkyv's archived structure before deserializing, but
    // exposes failure as a panic. Turn a protocol mismatch into a visible error.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        doc.merge_delta(BytesModuleStream::from_slice(payload).checkout_owned());
    }))
    .map_err(|_| "Incompatible Tinymist preview protocol. Use Tinymist 0.15.10.".to_owned())?;
    Ok(true)
}

fn render_pages(
    doc: &IncrDocClient,
    cache: &mut HashMap<PageKey, Arc<VectorPage>>,
) -> Result<CompiledPreview, String> {
    let mut selected: Vec<Page> = Vec::new();
    for region in &doc.doc.layouts {
        region.visit_pages(&mut |(_, pages)| {
            if selected.is_empty() {
                selected = pages.clone();
            }
        });
    }
    let mut next = HashMap::new();
    let mut pages = Vec::new();
    for page in selected {
        let key = (
            page.content,
            page.size.x.0.to_bits(),
            page.size.y.0.to_bits(),
        );
        let vector = if let Some(cached) = cache.get(&key) {
            cached.clone()
        } else {
            let svg = SvgText::join(SvgExporter::<SvgExportFeature>::render(
                &doc.doc.module,
                std::slice::from_ref(&page),
                None,
            ));
            let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
                .map_err(|error| error.to_string())?;
            Arc::new(VectorPage {
                id: page.content,
                tree,
            })
        };
        next.insert(key, vector.clone());
        pages.push(vector);
    }
    *cache = next;
    if pages.is_empty() {
        return Err("Tinymist returned no paged layout.".into());
    }
    Ok(CompiledPreview { pages })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_compiler_requests_report_errors() {
        let (sender, requests) = mpsc::channel();
        drop(requests);
        let (events, receiver) = async_channel::unbounded();
        let service = PreviewService { sender, events };
        service.refresh();
        service.update("main.typ".into(), "hello".into(), None);
        for _ in 0..2 {
            assert!(
                matches!(receiver.try_recv().unwrap(), PreviewEvent::Error(message) if message.contains("unavailable"))
            );
        }
    }

    fn wait_for<T>(
        events: &async_channel::Receiver<PreviewEvent>,
        mut check: impl FnMut(PreviewEvent) -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            while let Ok(event) = events.try_recv() {
                if let Some(result) = check(event) {
                    return result;
                }
            }
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for Tinymist preview"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn nested_document_uses_library_root_for_shared_imports_and_assets() {
        let root = tempfile::tempdir().unwrap();
        for folder in ["documents/Rechnung", "lib/assets"] {
            std::fs::create_dir_all(root.path().join(folder)).unwrap();
        }
        let entry = "documents/Rechnung/Rechnung_1_Test.typ";
        let source = "#import \"/lib/layout.typ\": layout\n#show: layout\nFirst invoice";
        std::fs::write(root.path().join(entry), source).unwrap();
        std::fs::write(
            root.path().join("lib/kontakte.typ"),
            "#let customer = [Example customer]",
        )
        .unwrap();
        std::fs::write(root.path().join("lib/layout.typ"), "#import \"/lib/kontakte.typ\": customer\n#let layout(body) = [#customer #image(\"/lib/assets/logo.svg\", width: 10pt) #body]").unwrap();
        std::fs::write(root.path().join("lib/assets/logo.svg"), r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="teal"/></svg>"#).unwrap();
        let (service, events) = PreviewService::start_entry(
            root.path().to_owned(),
            entry.into(),
            vec![(entry.into(), source.into())],
        );
        let first = wait_for(&events, |event| match event {
            PreviewEvent::Document(document) => Some(document),
            PreviewEvent::Error(error) => panic!("{error}"),
            _ => None,
        });
        assert_eq!(first.pages.len(), 1);
        service.update(
            entry.into(),
            source.replace("First invoice", "Edited invoice"),
            None,
        );
        wait_for(&events, |event| match event {
            PreviewEvent::Document(document) if document.pages[0].id != first.pages[0].id => {
                Some(())
            }
            PreviewEvent::Error(error) => panic!("{error}"),
            _ => None,
        });
        assert_eq!(
            std::fs::read_to_string(root.path().join(entry)).unwrap(),
            source
        );
    }

    #[test]
    fn persistent_compiler_applies_memory_edits_and_retains_unchanged_pages() {
        let root = tempfile::tempdir().unwrap();
        let main = "#set page(width: 200pt, height: 200pt)\n#include \"note.typ\"\n#pagebreak()\nUnchanged second page";
        std::fs::write(root.path().join("main.typ"), main).unwrap();
        std::fs::write(root.path().join("note.typ"), "First version").unwrap();
        let (service, events) = PreviewService::start(
            root.path().to_owned(),
            vec![
                ("main.typ".into(), main.into()),
                ("note.typ".into(), "First version".into()),
            ],
        );
        let first = wait_for(&events, |event| match event {
            PreviewEvent::Document(document) => Some(document),
            PreviewEvent::Error(error) => panic!("{error}"),
            _ => None,
        });
        assert_eq!(first.pages.len(), 2);
        service.update(
            "note.typ".into(),
            "Second version, edited in memory".into(),
            Some(EditLocation {
                revision: 1,
                path: "note.typ".into(),
                line: 0,
                character: 8,
            }),
        );
        let second = wait_for(&events, |event| match event {
            PreviewEvent::Document(document) if document.pages[0].id != first.pages[0].id => {
                Some(document)
            }
            PreviewEvent::Error(error) => panic!("{error}"),
            _ => None,
        });
        assert!(Arc::ptr_eq(&first.pages[1], &second.pages[1]));
        let included_location = wait_for(&events, |event| match event {
            PreviewEvent::Follow(location) if location.revision == 1 => Some(location),
            _ => None,
        });
        assert_eq!(included_location.page, 0);
        assert_eq!(
            std::fs::read_to_string(root.path().join("note.typ")).unwrap(),
            "First version"
        );
        service.update("note.typ".into(), "#let broken = (".into(), None);
        wait_for(&events, |event| {
            matches!(event, PreviewEvent::Error(_)).then_some(())
        });
        service.update("note.typ".into(), "Recovered document".into(), None);
        let recovered = wait_for(&events, |event| match event {
            PreviewEvent::Document(document) if document.pages[0].id != second.pages[0].id => {
                Some(document)
            }
            _ => None,
        });
        assert!(Arc::ptr_eq(&second.pages[1], &recovered.pages[1]));

        // Resolve a real source edit on the second page. Locations are 1-based
        // on the wire, zero-based in our continuous document layout.
        let changed = main.replace("Unchanged second page", "Edited second page — αβγ");
        service.update(
            "main.typ".into(),
            changed,
            Some(EditLocation {
                revision: 7,
                path: "main.typ".into(),
                line: 3,
                character: 8,
            }),
        );
        let location = wait_for(&events, |event| match event {
            PreviewEvent::Follow(location) if location.revision == 7 => Some(location),
            PreviewEvent::Error(error) => panic!("{error}"),
            _ => None,
        });
        assert_eq!(location.page, 1);
        assert!(location.x > 0. && location.x < 200.);
        assert!(location.y > 0. && location.y < 200.);

        for revision in 8..=10 {
            service.update(
                "main.typ".into(),
                format!("{main}\n#pagebreak()\nRapid edit {revision}"),
                Some(EditLocation {
                    revision,
                    path: "main.typ".into(),
                    line: 5,
                    character: 8,
                }),
            );
        }
        let latest_location = wait_for(&events, |event| match event {
            PreviewEvent::Follow(location) if location.revision == 10 => Some(location),
            PreviewEvent::Error(error) => panic!("{error}"),
            _ => None,
        });
        assert_eq!(latest_location.page, 2);
    }
}
