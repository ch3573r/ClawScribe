//! Local, read-only Model Context Protocol server on the launching client's pipes.
mod library;
pub(crate) mod settings;

use futures_util::{
    future::{abortable, AbortHandle, BoxFuture},
    stream::FuturesUnordered,
    FutureExt, StreamExt,
};
use serde_json::{json, Map, Value};
use std::{ffi::OsStr, io, path::PathBuf, time::Duration};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MODERN_VERSION: &str = "2026-07-28";
const LEGACY_VERSION: &str = "2025-11-25";
const VERSIONS: [&str; 3] = [MODERN_VERSION, LEGACY_VERSION, "2025-06-18"];
const MAX_REQUEST: usize = 64 * 1024;
const MAX_RESPONSE: usize = 1024 * 1024;
const DISABLED: &str = "Meeting access is off. Enable it in ClawScribe settings.";

/// Dispatch before any desktop plugin, single-instance guard or stdout logger starts.
pub fn dispatch() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new("--mcp-stdio")) {
        return None;
    }
    if args.next().is_some() {
        return Some(2);
    }
    let result = (|| {
        // Matches Tauri's desktop PathResolver::app_data_dir without starting a webview.
        let config: Value = serde_json::from_str(include_str!("../../tauri.conf.json"))?;
        let identifier = config["identifier"]
            .as_str()
            .ok_or(io::ErrorKind::InvalidData)?;
        let app_data = dirs::data_dir()
            .ok_or(io::ErrorKind::NotFound)?
            .join(identifier);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let result = runtime.block_on(serve(
            app_data,
            tokio::io::BufReader::new(tokio::io::stdin()),
            tokio::io::stdout(),
        ));
        // A lost output pipe can leave Tokio's background stdin reader blocked.
        // main exits this worker immediately after dispatch returns.
        runtime.shutdown_background();
        result
    })();
    Some(if result.is_ok() { 0 } else { 1 })
}

struct Tool {
    name: &'static str,
    description: &'static str,
    required_text: Option<&'static str>,
    paged: bool,
    text_offset: bool,
}

static TOOLS: [Tool; 8] = [
    Tool {
        name: "list_meetings",
        description: "List saved meetings, newest first. Use next_offset for the next page.",
        required_text: None,
        paged: true,
        text_offset: false,
    },
    Tool {
        name: "search_meetings",
        description: "Search saved meeting titles and transcripts for literal text. Use next_offset for the next page.",
        required_text: Some("query"),
        paged: true,
        text_offset: false,
    },
    Tool {
        name: "search_passages",
        description: "Find up to 25 literal matches in current saved transcripts, optionally limited by inclusive dates and project tags. Results include meeting and segment IDs, speaker and available recording time.",
        required_text: Some("query"),
        paged: false,
        text_offset: false,
    },
    Tool {
        name: "get_meeting",
        description: "Read a saved meeting's details, summary and notes. Use next_text_offset to continue long text.",
        required_text: Some("meeting_id"),
        paged: false,
        text_offset: true,
    },
    Tool {
        name: "get_transcript",
        description: "Read saved transcript segments in order. Copy next_offset and next_text_offset into the next call to continue.",
        required_text: Some("meeting_id"),
        paged: true,
        text_offset: true,
    },
    Tool {
        name: "get_audio",
        description: "Find the verified local recording path for a saved meeting. A null path means no file is available.",
        required_text: Some("meeting_id"),
        paged: false,
        text_offset: false,
    },
    Tool {
        name: "get_meeting_metadata",
        description: "Read limited metadata for one saved meeting by its ID. Uses the reviewed follow-through permission.",
        required_text: Some("meeting_id"),
        paged: false,
        text_offset: false,
    },
    Tool {
        name: "list_reviewed_actions",
        description: "Read open, user-reviewed actions from one saved meeting by its ID. Uses the reviewed follow-through permission.",
        required_text: Some("meeting_id"),
        paged: true,
        text_offset: false,
    },
];

impl Tool {
    fn follow_through(&self) -> bool {
        matches!(self.name, "get_meeting_metadata" | "list_reviewed_actions")
    }

    fn schema(&self) -> Value {
        let mut properties = Map::new();
        if let Some(name) = self.required_text {
            properties.insert(
                name.into(),
                json!({"type":"string","minLength":1,"maxLength":256,"pattern":"\\S"}),
            );
        }
        let offset = json!({"type":"integer","minimum":0,"maximum":i32::MAX,"default":0});
        if self.paged {
            properties.insert("offset".into(), offset.clone());
            properties.insert(
                "limit".into(),
                json!({"type":"integer","minimum":1,"maximum":100,"default":25}),
            );
        }
        if self.text_offset {
            properties.insert("text_offset".into(), offset);
        }
        if self.name == "search_passages" {
            for key in ["from", "to"] {
                properties.insert(key.into(), json!({"type":"string","format":"date","description":"Inclusive date (YYYY-MM-DD)."}));
            }
            properties.insert("tags".into(), json!({"type":"array","maxItems":20,"items":{"type":"string","minLength":1,"maxLength":60,"pattern":"\\S"}}));
            properties.insert(
                "tag_mode".into(),
                json!({"type":"string","enum":["any","all"],"default":"any"}),
            );
            properties.insert("untagged".into(), json!({"type":"boolean","default":false,"description":"Only meetings without project tags."}));
        }
        json!({
            "name":self.name, "description":self.description,
            "inputSchema":{"type":"object","properties":properties,"required":self.required_text.iter().collect::<Vec<_>>(),"additionalProperties":false},
            "annotations":{"readOnlyHint":true,"openWorldHint":false}
        })
    }

    fn arguments(&self, supplied: &Map<String, Value>) -> Result<Map<String, Value>, &'static str> {
        let mut args = Map::new();
        if let Some(key) = self.required_text {
            let text = supplied
                .get(key)
                .and_then(Value::as_str)
                .filter(|v| !v.trim().is_empty() && v.chars().count() <= 256)
                .ok_or("Supply the required query or meeting ID as 1 to 256 characters.")?;
            args.insert(key.into(), Value::from(text));
        }
        for (key, allowed, min, max, default) in [
            ("offset", self.paged, 0, i32::MAX as u64, 0),
            ("limit", self.paged, 1, 100, 25),
            ("text_offset", self.text_offset, 0, i32::MAX as u64, 0),
        ] {
            if allowed {
                let n = match supplied.get(key) {
                    None => default,
                    Some(v) => v.as_u64().filter(|n| (min..=max).contains(n))
                        .ok_or("Use a limit from 1 to 100 and nonnegative integer offsets up to 2147483647.")?,
                };
                args.insert(key.into(), Value::from(n));
            }
        }
        if self.name == "search_passages" {
            for key in ["from", "to", "tags", "tag_mode", "untagged"] {
                if let Some(value) = supplied.get(key) {
                    args.insert(key.into(), value.clone());
                }
            }
        }
        if supplied.keys().any(|key| !args.contains_key(key)) {
            return Err("An argument is unknown. Check the tool's input schema.");
        }
        Ok(args)
    }
}

fn server_info() -> Value {
    json!({"name":"clawscribe","title":"ClawScribe","version":env!("CARGO_PKG_VERSION")})
}

fn result(id: &Value, modern: bool, mut body: Value) -> Value {
    if modern {
        body["resultType"] = json!("complete");
        body["_meta"] = json!({"io.modelcontextprotocol/serverInfo":server_info()});
    }
    json!({"jsonrpc":"2.0","id":id,"result":body})
}

fn error(id: &Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn tool_error(message: &str) -> Value {
    json!({"content":[{"type":"text","text":message}],"isError":true})
}

#[derive(PartialEq)]
enum Handshake {
    New,
    Initializing,
    Ready,
}

// Bounded pending reads allow pings and cancellation while SQL is running.
// An aborted future yields None and never sends its tool result.
type ReadResult = (Value, bool, bool, Option<Result<Value, &'static str>>);
struct Server {
    app_data: PathBuf,
    handshake: Handshake,
    reads: FuturesUnordered<BoxFuture<'static, ReadResult>>,
    cancellations: Vec<(Value, AbortHandle)>,
}

impl Server {
    fn new(app_data: PathBuf) -> Self {
        Self {
            app_data,
            handshake: Handshake::New,
            reads: FuturesUnordered::new(),
            cancellations: Vec::new(),
        }
    }

    fn handle(&mut self, message: Value) -> Option<Value> {
        let id = &message["id"];
        let valid_id = id.is_string() || id.is_i64() || id.is_u64();
        if message["jsonrpc"] != "2.0" || (message.get("id").is_some() && !valid_id) {
            return Some(error(&Value::Null, -32600, "Invalid request"));
        }
        let Some(method) = message["method"].as_str() else {
            return Some(error(id, -32600, "Invalid request"));
        };
        let params = &message["params"];
        if id.is_null() {
            match method {
                "notifications/initialized"
                    if self.handshake == Handshake::Initializing
                        && (message.get("params").is_none() || params.is_object()) =>
                {
                    self.handshake = Handshake::Ready
                }
                "notifications/cancelled" => {
                    if let Some((_, cancel)) = self
                        .cancellations
                        .iter()
                        .find(|(v, _)| v == &params["requestId"])
                    {
                        cancel.abort();
                    }
                }
                _ => {}
            }
            return None;
        }
        if message.get("params").is_some() && !params.is_object() {
            return Some(error(id, -32602, "Expected a params object"));
        }
        let metadata = &params["_meta"];
        let requested = metadata.get("io.modelcontextprotocol/protocolVersion");
        let modern = requested.is_some();
        if modern
            || method == "server/discover"
            || metadata
                .get("io.modelcontextprotocol/clientCapabilities")
                .is_some()
        {
            let Some(version) = requested.and_then(Value::as_str) else {
                return Some(error(
                    id,
                    -32602,
                    "Missing protocol version in request metadata",
                ));
            };
            if version != MODERN_VERSION {
                let mut response = error(id, -32022, "Unsupported protocol version");
                response["error"]["data"] = json!({"supported":VERSIONS,"requested":version});
                return Some(response);
            }
            if !metadata["io.modelcontextprotocol/clientCapabilities"].is_object() {
                return Some(error(
                    id,
                    -32602,
                    "Missing client capabilities in request metadata",
                ));
            }
        }
        let body = match method {
            "server/discover" => json!({"supportedVersions":VERSIONS,"capabilities":{"tools":{}}}),
            "initialize" if !modern && self.handshake == Handshake::New => {
                let Some(version) = params["protocolVersion"].as_str() else {
                    return Some(error(id, -32602, "Missing protocol version"));
                };
                if !params["capabilities"].is_object()
                    || !params["clientInfo"]["name"].is_string()
                    || !params["clientInfo"]["version"].is_string()
                {
                    return Some(error(id, -32602, "Missing client capabilities or identity"));
                }
                self.handshake = Handshake::Initializing;
                let version = if VERSIONS[1..].contains(&version) {
                    version
                } else {
                    LEGACY_VERSION
                };
                json!({"protocolVersion":version,"serverInfo":server_info(),"capabilities":{"tools":{}}})
            }
            "ping" => json!({}),
            _ if !modern && self.handshake != Handshake::Ready => {
                return Some(error(id, -32600, "Initialize the MCP session first"))
            }
            "tools/list" => {
                if params.get("cursor").is_some() {
                    return Some(error(id, -32602, "Unknown tool list cursor"));
                }
                let follow_through = settings::allowed(&self.app_data, true);
                let library = settings::enabled(&self.app_data);
                json!({"tools":TOOLS.iter().filter(|tool| {
                    if tool.follow_through() { follow_through } else { library || !follow_through }
                }).map(Tool::schema).collect::<Vec<_>>()})
            }
            "tools/call" => return self.call(id, modern, params),
            _ => return Some(error(id, -32601, "Method not found")),
        };
        Some(result(id, modern, body))
    }

    fn call(&mut self, id: &Value, modern: bool, params: &Value) -> Option<Value> {
        let Some(name) = params["name"].as_str() else {
            return Some(error(id, -32602, "Missing tool name"));
        };
        let Some(tool) = TOOLS.iter().find(|t| t.name == name) else {
            return Some(error(id, -32602, "Unknown tool"));
        };
        let empty = Map::new();
        let input = match params.get("arguments") {
            None => &empty,
            Some(Value::Object(args)) => args,
            _ => return Some(error(id, -32602, "Expected an arguments object")),
        };
        let follow_through = tool.follow_through();
        if !settings::allowed(&self.app_data, follow_through) {
            return Some(result(id, modern, tool_error(DISABLED)));
        }
        let args = match tool.arguments(input) {
            Ok(args) => args,
            Err(msg) => return Some(result(id, modern, tool_error(msg))),
        };
        if self.cancellations.iter().any(|(key, _)| key == id) {
            return Some(error(id, -32600, "Duplicate active request ID"));
        }
        if self.reads.len() >= 4 {
            return Some(result(
                id,
                modern,
                tool_error("Four meeting reads are in progress. Retry shortly."),
            ));
        }
        let app_data = self.app_data.clone();
        let (read, cancel) = abortable(async move {
            if !settings::allowed(&app_data, follow_through) {
                return Err(DISABLED);
            }
            tokio::time::timeout(
                Duration::from_secs(15),
                library::call(&app_data, tool.name, &args),
            )
            .await
            .unwrap_or(Err(
                "The library read timed out. Retry with a smaller query or page.",
            ))
        });
        self.cancellations.push((id.clone(), cancel));
        let id = id.clone();
        self.reads
            .push(async move { (id, modern, follow_through, read.await.ok()) }.boxed());
        None
    }

    fn finish(&mut self, (id, modern, follow_through, read): ReadResult) -> Option<Value> {
        self.cancellations.retain(|(key, _)| key != &id);
        let read = read?;
        let body = if !settings::allowed(&self.app_data, follow_through) {
            tool_error(DISABLED)
        } else {
            match read {
                Ok(data) => json!({"content":[{"type":"text","text":data.to_string()}]}),
                Err(msg) => tool_error(msg),
            }
        };
        Some(result(&id, modern, body))
    }
}

async fn serve(
    app_data: PathBuf,
    mut input: impl AsyncBufRead + Unpin,
    mut output: impl AsyncWrite + Unpin,
) -> io::Result<()> {
    let mut server = Server::new(app_data);
    let mut line = Vec::new();
    loop {
        // read_until is cancellation-safe. Retain its partial line across completed reads.
        let mut limited = (&mut input).take((MAX_REQUEST + 1 - line.len()) as u64);
        let response = tokio::select! {
            Some(done) = server.reads.next(), if !server.reads.is_empty() => server.finish(done),
            count = limited.read_until(b'\n', &mut line) => {
                if count? == 0 && line.is_empty() { break; }
                if line.len() > MAX_REQUEST {
                    write_response(&mut output, error(&Value::Null,-32600,"Request too large")).await?;
                    return Ok(());
                }
                let response = match serde_json::from_slice(&line) {
                    Ok(request) => server.handle(request),
                    Err(_) => Some(error(&Value::Null,-32700,"Parse error")),
                };
                line.clear();
                response
            }
        };
        if let Some(response) = response {
            write_response(&mut output, response).await?;
        }
    }
    // Closing stdin finishes submission. A client can still be reading stdout.
    while let Some(done) = server.reads.next().await {
        if let Some(response) = server.finish(done) {
            write_response(&mut output, response).await?;
        }
    }
    Ok(())
}

async fn write_response(output: &mut (impl AsyncWrite + Unpin), response: Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(&response)?;
    if bytes.len() > MAX_RESPONSE {
        bytes = serde_json::to_vec(&error(
            &response["id"],
            -32603,
            "Response too large. Request a smaller page.",
        ))?;
    }
    bytes.push(b'\n');
    output.write_all(&bytes).await?;
    output.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    fn request(id: u32, method: &str, params: Value) -> Value {
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
    }

    fn initialize() -> Value {
        request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY_VERSION, "capabilities": {},
                "clientInfo": {"name":"fixture","version":"1"}
            }),
        )
    }

    fn ready(server: &mut Server) {
        let init = server.handle(initialize()).unwrap();
        assert_eq!(init["result"]["protocolVersion"], LEGACY_VERSION);
        assert!(server
            .handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .is_none());
    }

    #[test]
    fn discovery_and_initialization_support_both_stdio_protocols() {
        let root = tempfile::tempdir().unwrap();
        let mut server = Server::new(root.path().into());
        assert!(server
            .handle(request(1, "tools/list", json!({})))
            .unwrap()
            .get("error")
            .is_some());
        let metadata = json!({"_meta":{
            "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
            "io.modelcontextprotocol/clientCapabilities":{}
        }});
        let discover = server
            .handle(request(2, "server/discover", metadata.clone()))
            .unwrap();
        assert_eq!(discover["result"]["supportedVersions"][0], MODERN_VERSION);
        let tools = server.handle(request(3, "tools/list", metadata)).unwrap();
        assert_eq!(tools["result"]["resultType"], "complete");
        ready(&mut server);
        let listed = server.handle(request(4, "tools/list", json!({}))).unwrap();
        let tools = listed["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 6);
        assert!(tools
            .iter()
            .all(|t| t["annotations"]["readOnlyHint"] == true));
        assert!(server
            .handle(json!({"jsonrpc":"2.0","method":"notifications/unknown"}))
            .is_none());
        assert_eq!(
            server.handle(request(5, "unknown", json!({}))).unwrap()["error"]["code"],
            -32601
        );
    }

    #[tokio::test]
    async fn existing_client_loses_access_after_revocation() {
        let root = tempfile::tempdir().unwrap();
        let mut server = Server::new(root.path().into());
        ready(&mut server);
        let call = |id| request(id, "tools/call", json!({"name":"list_meetings"}));
        let denied = server.handle(call(2)).unwrap();
        assert_eq!(denied["result"]["isError"], true);
        settings::set_enabled(root.path(), true).unwrap();
        assert!(server.handle(call(3)).is_none());
        settings::set_enabled(root.path(), false).unwrap();
        let outstanding = server.reads.next().await.unwrap();
        assert_eq!(
            server.finish(outstanding).unwrap()["result"],
            denied["result"]
        );
        assert_eq!(server.handle(call(4)).unwrap()["result"], denied["result"]);
    }

    #[tokio::test]
    async fn follow_through_permission_is_independent_requires_a_reference_and_revokes_pending_reads(
    ) {
        let root = tempfile::tempdir().unwrap();
        let mut server = Server::new(root.path().into());
        ready(&mut server);
        let metadata = |id| {
            request(
                id,
                "tools/call",
                json!({
                    "name":"get_meeting_metadata", "arguments":{"meeting_id":"selected"}
                }),
            )
        };
        settings::set_enabled(root.path(), true).unwrap();
        let denied = server.handle(metadata(2)).unwrap();
        assert_eq!(denied["result"]["isError"], true);
        settings::set_enabled(root.path(), false).unwrap();
        settings::set_follow_through(root.path(), true).unwrap();
        let tools = server.handle(request(3, "tools/list", json!({}))).unwrap();
        let names: Vec<_> = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["get_meeting_metadata", "list_reviewed_actions"]);
        assert_eq!(
            server
                .handle(request(4, "tools/call", json!({"name":"list_meetings"})))
                .unwrap()["result"]["isError"],
            true
        );
        assert_eq!(
            server
                .handle(request(
                    5,
                    "tools/call",
                    json!({"name":"list_reviewed_actions"})
                ))
                .unwrap()["result"]["isError"],
            true
        );
        assert!(server.handle(metadata(6)).is_none());
        settings::set_follow_through(root.path(), false).unwrap();
        let outstanding = server.reads.next().await.unwrap();
        assert_eq!(
            server.finish(outstanding).unwrap()["result"],
            denied["result"]
        );
    }

    #[tokio::test]
    async fn half_close_drains_an_accepted_read_from_a_synthetic_library() {
        use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};
        let root = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(root.path().join("meeting_minutes.sqlite"))
            .create_if_missing(true);
        let mut db = SqliteConnection::connect_with(&options).await.unwrap();
        for sql in [
            "CREATE TABLE meetings(id TEXT, title TEXT, created_at TEXT, updated_at TEXT)",
            "CREATE TABLE transcripts(meeting_id TEXT, transcript TEXT)",
            "INSERT INTO meetings VALUES('test-id','Synthetic meeting','2026-01-01','2026-01-01')",
        ] {
            sqlx::query(sql).execute(&mut db).await.unwrap();
        }
        db.close().await.unwrap();
        settings::set_enabled(root.path(), true).unwrap();
        let input = [
            initialize(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            request(2, "tools/call", json!({"name":"list_meetings"})),
        ]
        .map(|v| v.to_string())
        .join("\n")
            + "\n";
        let mut output = Vec::new();
        serve(root.path().into(), input.as_bytes(), &mut output)
            .await
            .unwrap();
        let last = std::str::from_utf8(&output)
            .unwrap()
            .lines()
            .last()
            .unwrap();
        let response: Value = serde_json::from_str(last).unwrap();
        assert_eq!(response["id"], 2);
        let data: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(data["meetings"][0]["title"], "Synthetic meeting");
    }

    #[tokio::test]
    async fn cancelled_read_sends_no_result() {
        let root = tempfile::tempdir().unwrap();
        settings::set_enabled(root.path(), true).unwrap();
        let mut server = Server::new(root.path().into());
        ready(&mut server);
        assert!(server
            .handle(request(2, "tools/call", json!({"name":"list_meetings"})))
            .is_none());
        assert!(server
            .handle(json!({
                "jsonrpc":"2.0", "method":"notifications/cancelled","params":{"requestId":2}
            }))
            .is_none());
        let read = server.reads.next().await.unwrap();
        assert!(server.finish(read).is_none());
    }

    #[test]
    fn tool_inputs_enforce_schema_types_bounds_and_defaults() {
        let list = &TOOLS[0];
        let args = list.arguments(&Map::new()).unwrap();
        assert_eq!(args["limit"], 25);
        assert_eq!(args["offset"], 0);
        for invalid in [
            json!({"limit":0}),
            json!({"limit":101}),
            json!({"offset":-1}),
            json!({"offset":2.2}),
            json!({"path":"."}),
        ] {
            assert!(list.arguments(invalid.as_object().unwrap()).is_err());
        }
        let search = TOOLS.iter().find(|t| t.name == "search_meetings").unwrap();
        assert!(search
            .arguments(json!({"query":" "}).as_object().unwrap())
            .is_err());
        assert!(search
            .arguments(json!({"query":"Ä".repeat(256)}).as_object().unwrap())
            .is_ok());
    }

    #[tokio::test]
    async fn wire_flushes_responses_skips_notifications_and_bounds_frames() {
        let root = tempfile::tempdir().unwrap();
        let (client, server) = tokio::io::duplex(2048);
        let (input, output) = tokio::io::split(server);
        let worker = tokio::spawn(serve(root.path().into(), BufReader::new(input), output));
        let (receiver, mut sender) = tokio::io::split(client);
        let mut lines = BufReader::new(receiver).lines();
        sender.write_all(b"{\n").await.unwrap();
        let parse_error: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(parse_error["error"]["code"], -32700);
        sender
            .write_all(format!("{}\n", initialize()).as_bytes())
            .await
            .unwrap();
        assert!(lines
            .next_line()
            .await
            .unwrap()
            .unwrap()
            .contains(LEGACY_VERSION));
        sender.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n").await.unwrap();
        let ping: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(ping["id"], 2);
        assert_eq!(ping["result"], json!({}));
        sender
            .write_all(&vec![b' '; MAX_REQUEST + 1])
            .await
            .unwrap();
        assert!(lines
            .next_line()
            .await
            .unwrap()
            .unwrap()
            .contains("too large"));
        assert!(lines.next_line().await.unwrap().is_none());
        worker.await.unwrap().unwrap();
    }
}
