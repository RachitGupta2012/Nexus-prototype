//! Background MCP connection workers and the UI-facing registry.
//!
//! Each explicit connection owns one OS thread with a current-thread Tokio
//! runtime. Nothing starts at launch, and every worker has a hard connect and
//! discovery timeout. Events are bounded; the UI only polls them.

use std::collections::{BTreeMap, HashSet};
use std::process::Stdio;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rmcp::ServiceExt;
use rmcp::model::{PaginatedRequestParams, ServerCapabilities, Tool};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::{
    StreamableHttpClientTransport, TokioChildProcess,
    streamable_http_client::StreamableHttpClientTransportConfig,
};

use crate::config::{McpServerConfig, McpTransport};

use super::{
    Capabilities, MAX_DISCOVERY_PAGES, MAX_TOOL_DESCRIPTION_BYTES, MAX_TOOL_NAME_BYTES,
    MAX_TOOL_SCHEMA_BYTES, MAX_TOOLS_PER_SERVER, McpTool, ServerState, ServerStatus,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const EVENT_QUEUE: usize = 64;
const COMMAND_QUEUE: usize = 4;
const MAX_STDERR_DRAIN: usize = 64 * 1024;
const HTTP_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

const ERROR_SPAWN: &str = "Could not start the MCP server process.";
const ERROR_UNREACHABLE: &str = "Could not reach the MCP server endpoint.";
const ERROR_HANDSHAKE: &str = "The server did not complete a compatible MCP handshake.";
const ERROR_TIMEOUT: &str = "The MCP server did not finish connecting in time.";
const ERROR_DISCOVERY: &str = "The server accepted the connection but tool discovery failed.";
const ERROR_CLOSED: &str = "The MCP server closed the connection.";

#[derive(Debug)]
enum WorkerCommand {
    Refresh,
    Disconnect,
}

#[derive(Debug)]
enum McpEvent {
    Connected {
        id: u64,
        protocol: String,
        server_name: String,
        server_version: String,
        capabilities: Capabilities,
    },
    Discovering {
        id: u64,
    },
    Tools {
        id: u64,
        tools: Vec<McpTool>,
        omitted: usize,
        at: u64,
    },
    DiscoverFailed {
        id: u64,
        message: &'static str,
    },
    Failed {
        id: u64,
        message: &'static str,
    },
    Disconnected {
        id: u64,
    },
}

struct Worker {
    commands: tokio::sync::mpsc::Sender<WorkerCommand>,
}

/// Owns every MCP worker and the last known read-only status per server.
pub struct McpManager {
    statuses: BTreeMap<u64, ServerStatus>,
    workers: BTreeMap<u64, Worker>,
    tx: SyncSender<McpEvent>,
    rx: Receiver<McpEvent>,
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpManager {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel(EVENT_QUEUE);
        Self {
            statuses: BTreeMap::new(),
            workers: BTreeMap::new(),
            tx,
            rx,
        }
    }

    /// Adds missing defaults and drops status for servers no longer configured.
    pub fn sync(&mut self, servers: &[McpServerConfig]) {
        let ids: HashSet<u64> = servers.iter().map(|server| server.id).collect();
        self.statuses.retain(|id, _| ids.contains(id));
        for server in servers {
            self.statuses.entry(server.id).or_default();
        }
    }

    pub fn status(&self, id: u64) -> Option<&ServerStatus> {
        self.statuses.get(&id)
    }

    pub fn is_busy(&self) -> bool {
        self.statuses.values().any(|status| status.state.is_busy())
    }

    /// Starts one explicit connection attempt. Repeated calls while connected
    /// are rejected instead of silently reconnecting.
    pub fn connect(&mut self, server: &McpServerConfig) -> Result<(), &'static str> {
        if !server.enabled {
            return Err("Enable the server before connecting.");
        }
        if let Some(status) = self.statuses.get(&server.id)
            && (status.state.is_connected() || status.state.is_busy())
        {
            return Err("This server is already connected or connecting.");
        }
        super::config::validate(server)?;
        let mut status = ServerStatus {
            state: ServerState::Connecting,
            ..ServerStatus::default()
        };
        status.error = None;
        self.statuses.insert(server.id, status);

        let worker = spawn_worker(server.clone(), self.tx.clone())?;
        self.workers.insert(server.id, worker);
        Ok(())
    }

    pub fn refresh(&mut self, id: u64) -> Result<(), &'static str> {
        let worker = self
            .workers
            .get(&id)
            .ok_or("Connect the server before refreshing metadata.")?;
        worker
            .commands
            .try_send(WorkerCommand::Refresh)
            .map_err(|_| "The MCP worker is busy; try again shortly.")?;
        if let Some(status) = self.statuses.get_mut(&id) {
            status.state = ServerState::Discovering;
            status.error = None;
        }
        Ok(())
    }

    /// Requests a graceful disconnect. The final state is published by the
    /// worker after the transport closes.
    pub fn disconnect(&mut self, id: u64) {
        if let Some(worker) = self.workers.get(&id) {
            if worker.commands.try_send(WorkerCommand::Disconnect).is_ok()
                && let Some(status) = self.statuses.get_mut(&id)
            {
                status.state = ServerState::Disconnecting;
            }
        } else if let Some(status) = self.statuses.get_mut(&id) {
            status.state = ServerState::Disconnected;
            status.stale = !status.tools.is_empty();
        }
    }

    pub fn forget(&mut self, id: u64) {
        self.disconnect(id);
        self.workers.remove(&id);
        self.statuses.remove(&id);
    }

    /// Drains worker events without blocking. Returns true when the UI must repaint.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.rx.try_recv() {
                Ok(event) => {
                    changed = true;
                    self.apply(event);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        changed
    }

    fn apply(&mut self, event: McpEvent) {
        match event {
            McpEvent::Connected {
                id,
                protocol,
                server_name,
                server_version,
                capabilities,
            } => {
                if let Some(status) = self.statuses.get_mut(&id) {
                    status.state = ServerState::Connected;
                    status.protocol_version = Some(protocol);
                    status.server_name = Some(server_name);
                    status.server_version = Some(server_version);
                    status.capabilities = capabilities;
                    status.error = None;
                }
            }
            McpEvent::Discovering { id } => {
                if let Some(status) = self.statuses.get_mut(&id) {
                    status.state = ServerState::Discovering;
                }
            }
            McpEvent::Tools {
                id,
                tools,
                omitted,
                at,
            } => {
                if let Some(status) = self.statuses.get_mut(&id) {
                    status.state = ServerState::Ready;
                    status.tools = tools;
                    status.omitted_tools = omitted;
                    status.last_discovery = Some(at);
                    status.stale = false;
                    status.error = None;
                }
                self.workers.remove(&id);
            }
            McpEvent::DiscoverFailed { id, message } => {
                if let Some(status) = self.statuses.get_mut(&id) {
                    status.state = ServerState::Degraded;
                    status.error = Some(message.to_owned());
                    status.stale = true;
                }
            }
            McpEvent::Failed { id, message } => {
                if let Some(status) = self.statuses.get_mut(&id) {
                    status.state = ServerState::Error;
                    status.error = Some(message.to_owned());
                    status.stale = !status.tools.is_empty();
                }
                self.workers.remove(&id);
            }
            McpEvent::Disconnected { id } => {
                if let Some(status) = self.statuses.get_mut(&id) {
                    status.state = ServerState::Disconnected;
                    status.error = None;
                    status.stale = !status.tools.is_empty();
                }
                self.workers.remove(&id);
            }
        }
    }
}

fn spawn_worker(
    server: McpServerConfig,
    events: SyncSender<McpEvent>,
) -> Result<Worker, &'static str> {
    let (commands, command_rx) = tokio::sync::mpsc::channel(COMMAND_QUEUE);
    std::thread::Builder::new()
        .name(format!("mcp-server-{}", server.id))
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                let _ = events.send(McpEvent::Failed {
                    id: server.id,
                    message: ERROR_SPAWN,
                });
                return;
            };
            runtime.block_on(run_worker(server, command_rx, events));
        })
        .map_err(|_| "Could not start the MCP background worker.")?;
    Ok(Worker { commands })
}

async fn run_worker(
    server: McpServerConfig,
    mut commands: tokio::sync::mpsc::Receiver<WorkerCommand>,
    events: SyncSender<McpEvent>,
) {
    let id = server.id;
    let mut service = match connect(&server).await {
        Ok(service) => service,
        Err(message) => {
            let _ = events.send(McpEvent::Failed { id, message });
            return;
        }
    };
    let Some(info) = service.peer_info() else {
        let _ = events.send(McpEvent::Failed {
            id,
            message: ERROR_HANDSHAKE,
        });
        let _ = service.close_with_timeout(DISCONNECT_TIMEOUT).await;
        return;
    };
    let capabilities = convert_capabilities(&info.capabilities);
    let _ = events.send(McpEvent::Connected {
        id,
        protocol: info.protocol_version.to_string(),
        server_name: info
            .server_info
            .as_ref()
            .map(|identity| identity.name.clone())
            .unwrap_or_else(|| "Unknown server".to_owned()),
        server_version: info
            .server_info
            .as_ref()
            .map(|identity| identity.version.clone())
            .unwrap_or_default(),
        capabilities,
    });

    if capabilities.tools {
        let _ = events.send(McpEvent::Discovering { id });
        match discovery(&service).await {
            Ok((tools, omitted)) => {
                let _ = events.send(McpEvent::Tools {
                    id,
                    tools,
                    omitted,
                    at: unix_now(),
                });
            }
            Err(()) => {
                let _ = events.send(McpEvent::DiscoverFailed {
                    id,
                    message: ERROR_DISCOVERY,
                });
            }
        }
    } else {
        let _ = events.send(McpEvent::Tools {
            id,
            tools: Vec::new(),
            omitted: 0,
            at: unix_now(),
        });
    }

    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None | Some(WorkerCommand::Disconnect) => {
                    let _ = service.close_with_timeout(DISCONNECT_TIMEOUT).await;
                    let _ = events.send(McpEvent::Disconnected { id });
                    return;
                }
                Some(WorkerCommand::Refresh) => {
                    let _ = events.send(McpEvent::Discovering { id });
                    match discovery(&service).await {
                        Ok((tools, omitted)) => {
                            let _ = events.send(McpEvent::Tools { id, tools, omitted, at: unix_now() });
                        }
                        Err(()) => {
                            let _ = events.send(McpEvent::DiscoverFailed { id, message: ERROR_DISCOVERY });
                        }
                    }
                }
            },
            () = tokio::time::sleep(Duration::from_millis(500)) => {
                if service.is_closed() {
                    let _ = events.send(McpEvent::Failed { id, message: ERROR_CLOSED });
                    return;
                }
            }
        }
    }
}

async fn connect(server: &McpServerConfig) -> Result<RunningService<RoleClient, ()>, &'static str> {
    match server.transport {
        McpTransport::Stdio => {
            let Some(command) = stdio_command(server) else {
                return Err(ERROR_SPAWN);
            };
            let (transport, stderr) = TokioChildProcess::builder(command)
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|_| ERROR_SPAWN)?;
            if let Some(stderr) = stderr {
                tokio::spawn(drain(stderr));
            }
            match tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport)).await {
                Ok(Ok(service)) => Ok(service),
                Ok(Err(_)) => Err(ERROR_HANDSHAKE),
                Err(_) => Err(ERROR_TIMEOUT),
            }
        }
        McpTransport::StreamableHttp => {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(HTTP_IDLE_TIMEOUT)
                .build()
                .map_err(|_| ERROR_UNREACHABLE)?;
            let mut config = StreamableHttpClientTransportConfig::with_uri(server.url.clone());
            config.channel_buffer_capacity = 32;
            config.max_sse_event_size = 8 * 1024 * 1024;
            let transport = StreamableHttpClientTransport::with_client(client, config);
            match tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport)).await {
                Ok(Ok(service)) => Ok(service),
                Ok(Err(_)) => Err(ERROR_UNREACHABLE),
                Err(_) => Err(ERROR_TIMEOUT),
            }
        }
    }
}

fn stdio_command(server: &McpServerConfig) -> Option<tokio::process::Command> {
    let command = server.command.trim();
    if command.is_empty() {
        return None;
    }
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(&server.args);
    if let Some(cwd) = &server.cwd {
        let cwd = cwd.trim();
        if !cwd.is_empty() {
            cmd.current_dir(cwd);
        }
    }
    let allowed: HashSet<&str> = server
        .env_allowlist
        .iter()
        .map(|name| name.as_str())
        .collect();
    for (key, value) in std::env::vars_os() {
        let key_text = key.to_string_lossy();
        if allowed.contains(key_text.as_ref()) {
            cmd.env(key, value);
        }
    }
    cmd.env("NEXUS_MCP_CLIENT", "1");
    Some(cmd)
}

async fn drain(mut stderr: tokio::process::ChildStderr) {
    use tokio::io::AsyncReadExt;
    let mut buffer = [0_u8; 4_096];
    let mut total = 0;
    while let Ok(read) = stderr.read(&mut buffer).await {
        if read == 0 {
            break;
        }
        total += read;
        if total > MAX_STDERR_DRAIN {
            break;
        }
    }
}

fn convert_capabilities(capabilities: &ServerCapabilities) -> Capabilities {
    Capabilities {
        tools: capabilities.tools.is_some(),
        resources: capabilities.resources.is_some(),
        prompts: capabilities.prompts.is_some(),
        logging: capabilities.logging.is_some(),
        completions: capabilities.completions.is_some(),
        experimental: capabilities.experimental.is_some(),
        tool_list_changed: capabilities
            .tools
            .as_ref()
            .and_then(|tools| tools.list_changed)
            .unwrap_or(false),
    }
}

async fn discovery(service: &RunningService<RoleClient, ()>) -> Result<(Vec<McpTool>, usize), ()> {
    let id = 0;
    let _ = id;
    let result = tokio::time::timeout(DISCOVERY_TIMEOUT, fetch_tools(service)).await;
    match result {
        Ok(Ok(found)) => Ok(found),
        _ => Err(()),
    }
}

async fn fetch_tools(
    service: &RunningService<RoleClient, ()>,
) -> Result<(Vec<McpTool>, usize), ()> {
    let mut tools = Vec::new();
    let mut omitted = 0;
    let mut seen = HashSet::new();
    let mut cursor = None;
    for _ in 0..MAX_DISCOVERY_PAGES {
        let params = PaginatedRequestParams::default().with_cursor(cursor);
        let page = service
            .peer()
            .list_tools(Some(params))
            .await
            .map_err(|_| ())?;
        for tool in page.tools {
            if tools.len() >= MAX_TOOLS_PER_SERVER {
                omitted += 1;
                continue;
            }
            match convert_tool(&tool) {
                Some(tool) if seen.insert(tool.name.clone()) => tools.push(tool),
                _ => omitted += 1,
            }
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    if cursor.is_some() {
        omitted += 1;
    }
    Ok((tools, omitted))
}

fn convert_tool(tool: &Tool) -> Option<McpTool> {
    let name = tool.name.trim();
    if name.is_empty() || name.len() > MAX_TOOL_NAME_BYTES {
        return None;
    }
    let schema = serde_json::to_value(&*tool.input_schema).ok()?;
    if serde_json::to_vec(&schema).ok()?.len() > MAX_TOOL_SCHEMA_BYTES {
        return None;
    }
    let description = tool
        .description
        .as_deref()
        .map(|text| truncate(text.trim(), MAX_TOOL_DESCRIPTION_BYTES))
        .unwrap_or_default();
    Some(McpTool {
        name: name.to_owned(),
        description,
        input_schema: schema,
    })
}

fn truncate(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "fixture_json.rs"]
mod fixture_json;

#[cfg(test)]
mod tests {
    use super::*;

    fn server(transport: McpTransport) -> McpServerConfig {
        McpServerConfig {
            id: 7,
            name: "Mock".to_owned(),
            transport,
            command: "/nonexistent/nexus-mcp-test-binary".to_owned(),
            args: Vec::new(),
            cwd: None,
            env_allowlist: Vec::new(),
            url: String::new(),
            bearer_auth: false,
            enabled_tools: Vec::new(),
            enabled: true,
        }
    }

    #[test]
    fn manager_rejects_connect_for_disabled_invalid_and_duplicate_servers() {
        let mut manager = McpManager::new();
        let mut settings = server(McpTransport::Stdio);
        settings.command.clear();
        assert!(manager.connect(&settings).is_err());
        settings.command = "/nonexistent/nexus-mcp-test-binary".to_owned();
        settings.enabled = false;
        assert!(manager.connect(&settings).is_err());
        settings.enabled = true;
        assert!(manager.connect(&settings).is_ok());
        assert!(manager.connect(&settings).is_err());
        assert_eq!(
            manager.status(settings.id).unwrap().state,
            ServerState::Connecting
        );
        manager.disconnect(settings.id);
    }

    #[test]
    fn fixture_json_finds_top_level_fields_in_any_order_with_whitespace() {
        use super::fixture_json::{object_field, string_value};

        let message = r#"{ "params" : { "nested" : { "id" : 99 }, "protocolVersion" : "2026-07-28" }, "method" : "initialize", "id" : 42 }"#;
        assert_eq!(object_field(message, "id"), Some("42"));
        assert_eq!(object_field(message, "method"), Some("\"initialize\""));
        assert_eq!(
            object_field(message, "method")
                .and_then(string_value)
                .as_deref(),
            Some("initialize")
        );
        let params = object_field(message, "params").unwrap();
        assert_eq!(
            object_field(params, "protocolVersion"),
            Some("\"2026-07-28\"")
        );
        assert_eq!(object_field(params, "id"), None);
        assert_eq!(
            object_field(r#"{"params":{"method":"fake","id":1}}"#, "method"),
            None
        );
    }

    #[test]
    fn fixture_json_preserves_ids_and_handles_escaped_strings() {
        use super::fixture_json::{object_field, string_value};

        for (message, expected) in [
            (r#"{"id":7}"#, "7"),
            (r#"{"id":"req-7"}"#, r#""req-7""#),
            (r#"{"id":-12.50e+2}"#, "-12.50e+2"),
            (r#"{"method":"ping","id":"a\"b"}"#, r#""a\"b""#),
        ] {
            assert_eq!(object_field(message, "id"), Some(expected));
        }
        assert_eq!(string_value(r#""a\"b""#).as_deref(), Some("a\"b"));
        assert_eq!(string_value("  \"ping\" \n").as_deref(), Some("ping"));
    }

    #[test]
    fn spawn_failure_becomes_a_terminal_error_without_raw_details() {
        let mut manager = McpManager::new();
        let settings = server(McpTransport::Stdio);
        manager.connect(&settings).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            manager.poll();
            if manager.status(settings.id).unwrap().state == ServerState::Error {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let status = manager.status(settings.id).unwrap();
        assert_eq!(status.state, ServerState::Error);
        assert_eq!(status.error.as_deref(), Some(ERROR_SPAWN));
    }

    fn mock_server_binary() -> &'static str {
        static BINARY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        BINARY.get_or_init(|| {
            let source = concat!(
                include_str!("fixture_json.rs"),
                r##"
use std::io::{BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let log = args
        .windows(2)
        .find(|pair| pair[0] == "--log")
        .map(|pair| pair[1].clone());
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let method = object_field(&line, "method")
            .and_then(string_value)
            .unwrap_or_default();
        if let Some(path) = &log {
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
            writeln!(file, "{method}").unwrap();
        }
        let Some(id) = object_field(&line, "id") else { continue };
        let result = match method.as_str() {
            "initialize" => {
                let version = object_field(&line, "params")
                    .and_then(|params| object_field(params, "protocolVersion"))
                    .unwrap_or("\"2026-07-28\"");
                format!(
                    "{{\"protocolVersion\":{version},\"capabilities\":{{\"tools\":{{\"listChanged\":false}}}},\"serverInfo\":{{\"name\":\"Mock MCP\",\"version\":\"1.0.0\"}},\"instructions\":\"Read-only fixture\"}}"
                )
            }
            "ping" => "{}".to_owned(),
            "tools/list" => r#"{"tools":[{"name":"echo","description":"Returns its input","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}},{"name":"noop","inputSchema":{"type":"object"}}]}"#.to_owned(),
            _ => {
                let response = format!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32601,\"message\":\"not supported by fixture\"}}}}"
                );
                writeln!(stdout, "{response}").unwrap();
                stdout.flush().unwrap();
                continue;
            }
        };
        let response = format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}");
        writeln!(stdout, "{response}").unwrap();
        stdout.flush().unwrap();
    }
}
"##
            );
            let dir = std::env::temp_dir().join(format!(
                "nexus-mcp-fixture-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let source_path = dir.join("mock.rs");
            let binary_path = dir.join("mock-server");
            std::fs::write(&source_path, source).unwrap();
            let status = std::process::Command::new("rustc")
                .arg("--edition=2021")
                .arg("-o")
                .arg(&binary_path)
                .arg(&source_path)
                .status()
                .expect("rustc must be available for the MCP fixture");
            assert!(status.success(), "fixture compilation failed");
            binary_path.to_string_lossy().into_owned()
        })
    }

    fn wait_for(manager: &mut McpManager, id: u64, state: ServerState, seconds: u64) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
        while std::time::Instant::now() < deadline {
            manager.poll();
            if manager
                .status(id)
                .is_some_and(|status| status.state == state)
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn stdio_lifecycle_discovers_metadata_and_never_calls_a_tool() {
        let binary = mock_server_binary();
        let log = std::env::temp_dir().join(format!(
            "nexus-mcp-fixture-{}-requests.log",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&log);
        let mut settings = server(McpTransport::Stdio);
        settings.command = binary.to_owned();
        settings.args = vec!["--log".to_owned(), log.to_string_lossy().into_owned()];
        let mut manager = McpManager::new();
        manager.connect(&settings).unwrap();
        assert!(
            wait_for(&mut manager, settings.id, ServerState::Ready, 20),
            "fixture did not become ready: {:?}",
            manager.status(settings.id)
        );
        let status = manager.status(settings.id).unwrap();
        assert_eq!(status.protocol_version.as_deref(), Some("2026-07-28"));
        assert_eq!(status.server_name.as_deref(), Some("Mock MCP"));
        assert_eq!(status.tools.len(), 2);
        assert_eq!(status.tools[0].name, "echo");
        assert!(status.tools[0].description.contains("input"));
        assert!(status.capabilities.tools);
        manager.disconnect(settings.id);
        assert!(
            wait_for(&mut manager, settings.id, ServerState::Disconnected, 5),
            "fixture did not disconnect"
        );
        assert!(manager.status(settings.id).unwrap().stale);
        let logged = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(logged.contains("tools/list"));
        for forbidden in ["tools/call", "resources/read", "prompts/get"] {
            assert!(!logged.contains(forbidden), "unexpected {forbidden}");
        }
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn streamable_http_refused_connection_is_reported_without_raw_details() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let mut settings = server(McpTransport::StreamableHttp);
        settings.command.clear();
        settings.url = format!("http://127.0.0.1:{port}/mcp");
        let mut manager = McpManager::new();
        manager.connect(&settings).unwrap();
        assert!(
            wait_for(&mut manager, settings.id, ServerState::Error, 20),
            "refused endpoint did not fail: {:?}",
            manager.status(settings.id)
        );
        let error = manager.status(settings.id).unwrap().error.clone().unwrap();
        assert!(error == ERROR_UNREACHABLE || error == ERROR_TIMEOUT);
        assert!(!error.contains("127.0.0.1"));
    }

    #[test]
    fn sync_drops_removed_servers_and_keeps_defaults() {
        let mut manager = McpManager::new();
        manager.sync(&[server(McpTransport::Stdio)]);
        assert!(manager.status(7).is_some());
        manager.sync(&[]);
        assert!(manager.status(7).is_none());
    }
}
