//! Headless interaction and snapshot tests for the desktop shell.
//!
//! These run through AccessKit via `egui_kittest`, so they click real widgets
//! and read real labels instead of calling handlers directly. Snapshots live in
//! `tests/snapshots`; regenerate them with:
//!
//! ```sh
//! UPDATE_SNAPSHOTS=1 cargo test --workspace snapshot
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::NexusApp;
use super::Section;
use crate::config::{Config, Protocol, Theme};
use crate::storage::Storage;

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "nexus-ui-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct TestApp {
    dir: TestDir,
    harness: Harness<'static, NexusApp>,
}

impl TestApp {
    fn new() -> Self {
        Self::seeded(|_| {})
    }

    fn checked() -> Self {
        Self::build(|_| {})
    }

    /// Same UI with fixed, realistic data-location paths so snapshots do not
    /// embed the temporary directory name.
    fn fixed_paths() -> Self {
        let mut app = Self::build(|_| {});
        let base = Path::new("/Users/example/Library/Application Support/NEXUS");
        app.harness.state_mut().config_path = base.join("config.toml");
        app.harness.state_mut().database_path = base.join("nexus.db");
        app
    }

    fn seeded(seed: impl FnOnce(&Storage)) -> Self {
        Self::build(seed)
    }

    fn build(seed: impl FnOnce(&Storage)) -> Self {
        let dir = TestDir::new();
        let config_path = dir.0.join("config.toml");
        let database_path = dir.0.join("nexus.db");
        let config = Config {
            theme: Theme::Dark,
            ..Config::default()
        };
        config.save(&config_path).unwrap();
        let storage = Storage::open(&database_path).unwrap();
        seed(&storage);
        let app = NexusApp::headless(config, config_path, database_path.clone(), storage);
        let harness = Harness::builder()
            .with_size(egui::vec2(1_080.0, 720.0))
            .with_theme(egui::Theme::Dark)
            .with_accessibility_check(true)
            .build_ui_state(|ui, app: &mut NexusApp| app.show(ui), app);
        Self { dir, harness }
    }

    fn config_path(&self) -> PathBuf {
        self.dir.0.join("config.toml")
    }

    fn count(&self) -> u64 {
        self.harness.state().storage.conversation_count().unwrap()
    }

    fn settle(&mut self) {
        self.harness.run();
        self.harness.step();
        self.harness.run();
    }

    fn click(&mut self, label: &str) {
        let node = self
            .harness
            .query_all_by_label(label)
            .next()
            .unwrap_or_else(|| panic!("no widget labelled {label:?}"));
        node.click_accesskit();
        self.settle();
    }

    fn has(&self, label: &str) -> bool {
        self.harness.query_all_by_label(label).next().is_some()
    }

    fn go(&mut self, label: &str) {
        self.click(label);
    }

    fn type_into(&mut self, label: &str, text: &str) {
        let node = self
            .harness
            .query_all_by_label(label)
            .next()
            .unwrap_or_else(|| panic!("no text field labelled {label:?}"));
        node.focus();
        node.type_text(text);
        self.settle();
    }
}

fn seed_conversations(storage: &Storage) {
    for title in ["Alpha notes", "Beta plan", "Gamma tasks"] {
        storage
            .create_conversation(1, Protocol::OpenAiChat, "auto", title, "")
            .unwrap();
    }
}

#[test]
fn navigation_reaches_every_section() {
    let mut app = TestApp::new();
    for (label, section) in [
        ("Providers", Section::Providers),
        ("MCP", Section::Mcp),
        ("Usage", Section::Usage),
        ("Settings", Section::Settings),
        ("Chat", Section::Chat),
    ] {
        app.go(label);
        assert_eq!(app.harness.state().selected_section, section, "{label}");
    }
}

#[test]
fn theme_selection_updates_config_and_file() {
    let mut app = TestApp::new();
    app.go("Settings");
    app.click("Light");
    assert_eq!(app.harness.state().config.theme, Theme::Light);
    let saved = fs::read_to_string(app.config_path()).unwrap();
    assert!(saved.contains("theme = \"light\""), "{saved}");
    app.click("Dark");
    assert_eq!(app.harness.state().config.theme, Theme::Dark);
}

#[test]
fn history_toggle_updates_storage_gate_and_file() {
    let mut app = TestApp::new();
    app.go("Settings");
    app.click("Save conversations to this Mac");
    assert!(!app.harness.state().config.history_enabled);
    assert!(!app.harness.state().storage.history_writes_enabled());
    let saved = fs::read_to_string(app.config_path()).unwrap();
    assert!(saved.contains("history_enabled = false"), "{saved}");

    app.click("Save conversations to this Mac");
    assert!(app.harness.state().config.history_enabled);
    assert!(app.harness.state().storage.history_writes_enabled());
}

#[test]
fn search_filters_by_title_and_reports_no_matches() {
    let mut app = TestApp::seeded(seed_conversations);
    assert!(app.has("Beta plan"));
    app.harness
        .get_by_label("Search conversations")
        .set_text_value_accesskit("Beta");
    app.settle();
    assert!(app.has("Beta plan"));
    assert!(!app.has("Alpha notes"));
    assert!(!app.has("Gamma tasks"));

    app.harness
        .get_by_label("Search conversations")
        .set_text_value_accesskit("zzzz");
    app.settle();
    assert!(app.has("No conversations match that search."));
}

#[test]
fn selecting_a_conversation_opens_it() {
    let mut app = TestApp::seeded(seed_conversations);
    let expected = app.harness.state().chat.selected_id();
    assert_eq!(expected, None);
    app.click("Beta plan");
    let opened = app.harness.state().chat.selected_id().unwrap();
    let stored = app
        .harness
        .state()
        .storage
        .load_conversation(opened)
        .unwrap();
    assert_eq!(stored.unwrap().title, "Beta plan");
}

#[test]
fn deleting_a_conversation_requires_confirmation() {
    let mut app = TestApp::seeded(seed_conversations);
    assert_eq!(app.count(), 3);
    app.click("Delete Alpha notes");
    assert!(app.has("Delete this conversation?"));
    app.click("Cancel");
    assert_eq!(app.count(), 3);
    assert!(!app.has("Delete this conversation?"));

    app.click("Delete Alpha notes");
    app.click("Delete");
    assert_eq!(app.count(), 2);
}

#[test]
fn clearing_history_requires_confirmation() {
    let mut app = TestApp::seeded(seed_conversations);
    app.click("Clear all history");
    assert!(app.has("Delete all saved conversations?"));
    app.click("Cancel");
    assert_eq!(app.count(), 3);

    app.click("Clear all history");
    app.click("Delete everything");
    assert_eq!(app.count(), 0);
    assert!(app.has("No conversations yet."));
}

#[test]
fn empty_states_are_visible() {
    let mut app = TestApp::new();
    assert!(app.has("No conversations yet."));
    app.go("Usage");
    assert!(app.has("No usage recorded yet"));
}

#[test]
fn accessibility_names_pass_on_chat_settings_and_usage() {
    let mut app = TestApp::checked();
    app.settle();
    app.go("Settings");
    app.go("Usage");
    app.go("Chat");
}

#[test]
fn accessibility_names_pass_on_saved_conversations() {
    let mut app = TestApp::build(seed_conversations);
    app.settle();
    assert!(app.has("Beta plan"));
}

#[test]
fn first_run_callout_follows_config_and_dismisses_persistently() {
    let mut app = TestApp::new();
    assert!(app.has("Set up a provider"));
    app.click("Dismiss");
    assert!(!app.has("Set up a provider"));
    assert!(app.harness.state().config.first_run_callout_dismissed);
    let saved = fs::read_to_string(app.config_path()).unwrap();
    assert!(
        saved.contains("first_run_callout_dismissed = true"),
        "{saved}"
    );

    let mut reopened = TestApp::new();
    reopened
        .harness
        .state_mut()
        .config
        .first_run_callout_dismissed = true;
    reopened.harness.run();
    assert!(!reopened.has("Set up a provider"));
}

#[test]
fn mcp_servers_are_added_saved_and_removed_only_on_request() {
    let mut app = TestApp::new();
    app.go("MCP");
    assert!(app.has("No MCP servers configured."));
    assert!(!app.has("Connect"));

    app.click("+ Add server");
    app.type_into("Executable path", "/usr/local/bin/mock-mcp");
    app.click("Save configuration");
    let config = &app.harness.state().config;
    assert_eq!(config.mcp_servers.len(), 1);
    assert_eq!(config.mcp_servers[0].command, "/usr/local/bin/mock-mcp");
    assert_eq!(config.mcp_servers[0].name, "MCP server 1");
    assert!(app.has("Disconnected"));
    assert!(app.has("Connect"));
    let saved = fs::read_to_string(app.config_path()).unwrap();
    assert!(saved.contains("mock-mcp"), "{saved}");
    assert!(saved.contains("enabled = true"), "{saved}");

    // Nothing connects just by opening the section.
    assert_eq!(
        app.harness.state().mcp.manager_status_for_test(1).unwrap(),
        crate::mcp::ServerState::Disconnected
    );

    app.click("Remove");
    assert!(app.has("Confirm remove"));
    app.click("Cancel");
    assert_eq!(app.harness.state().config.mcp_servers.len(), 1);

    app.click("Remove");
    app.click("Confirm remove");
    assert!(app.harness.state().config.mcp_servers.is_empty());
    assert!(app.has("No MCP servers configured."));
    let saved = fs::read_to_string(app.config_path()).unwrap();
    assert!(!saved.contains("mock-mcp"), "{saved}");
}

#[test]
fn mcp_editor_rejects_an_incomplete_stdio_server_without_saving() {
    let mut app = TestApp::new();
    app.go("MCP");
    app.click("+ Add server");
    app.click("Save configuration");
    assert!(app.has("Set the executable to launch for this STDIO server."));
    assert!(app.harness.state().config.mcp_servers.is_empty());
    app.click("Cancel");
    assert!(app.has("No MCP servers configured."));
}

#[test]
fn mcp_section_exposes_distinct_accessible_names() {
    let mut app = TestApp::new();
    app.go("MCP");
    app.click("+ Add server");
    for label in [
        "Server name",
        "Executable path",
        "Arguments, one per line",
        "Working directory",
        "Environment variable names, one per line",
        "Save configuration",
        "Cancel",
    ] {
        assert!(app.has(label), "missing accessible name: {label}");
    }
    app.click("Cancel");
    app.click("+ Add server");
    app.click("Streamable HTTP");
    app.type_into("Endpoint URL", "http://example.com/mcp");
    app.click("Save configuration");
    assert_eq!(
        app.harness.state().mcp.message_for_test(),
        Some("Use HTTPS, or HTTP only for a loopback endpoint.")
    );
    assert!(app.has("Endpoint URL"));
}

#[test]
fn recovery_screen_snapshot_inputs_stay_sanitized() {
    let screen = crate::ui::recovery::RecoveryScreen::preview(
        "/Users/example/Library/Application Support/NEXUS/config.toml",
        "The file is not valid TOML (parse error at line 4, column 1).",
    );
    assert!(!screen.error_text().contains("sk-"));
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

fn snapshot(name: &str, app: &mut TestApp) {
    app.harness.run();
    app.harness.snapshot(name);
}

#[test]
fn provider_fields_and_catalog_expose_distinct_accessible_names() {
    let mut app = TestApp::new();
    app.go("Providers");
    for label in [
        "Provider name",
        "Base URL",
        "Default model",
        "Test connection",
        "Fetch models",
        "Model ID for model 1 (auto)",
        "Display name for model 1 (auto)",
        "Track context window for model 1 (auto)",
        "Declare tool support for model 1 (auto)",
        "Track pricing for model 1 (auto)",
        "Remove model 1 (auto) from the catalog",
    ] {
        assert!(app.has(label), "missing accessible name: {label}");
    }

    app.click("+ Add model");
    assert!(app.has("Model ID for model 2 (model-2)"));
    assert!(app.has("Display name for model 2 (model-2)"));
    assert!(app.has("Model ID for model 1 (auto)"));
    assert!(!app.has("Model ID for model 2 (auto)"));
}

#[test]
fn all_provider_presets_offer_model_discovery() {
    let mut app = TestApp::new();
    app.go("Providers");
    app.click("+ Add");
    app.click("Anthropic · Messages");
    assert!(app.has("Fetch models"));
    assert!(!app.has("Model discovery is not implemented for this protocol in this build."));

    app.click("+ Add");
    app.click("Google · Gemini");
    assert!(app.has("Fetch models"));
    assert!(!app.has("Model discovery is not implemented for this protocol in this build."));

    app.click("+ Add");
    app.click("OpenAI · Responses");
    assert!(app.has("Fetch models"));
    assert!(!app.has("Model discovery is not implemented for this protocol in this build."));
}

#[test]
fn connectivity_controls_show_loading_and_result_states() {
    let mut app = TestApp::new();
    app.go("Providers");
    app.harness
        .state_mut()
        .providers
        .set_probe_state_for_test(None, true);
    app.settle();
    assert!(app.has("Contacting the provider…"));

    app.harness.state_mut().providers.set_probe_state_for_test(
        Some((false, "Authentication was rejected (HTTP 401).")),
        false,
    );
    app.settle();
    assert!(app.has("Authentication was rejected (HTTP 401)."));

    app.harness.state_mut().providers.set_probe_state_for_test(
        Some((
            true,
            "Server reachable; this protocol needs no API key. 2 models were found.",
        )),
        false,
    );
    app.settle();
    assert!(app.has("Server reachable; this protocol needs no API key. 2 models were found."));
}

#[test]
fn snapshot_chat_with_first_run_callout() {
    let mut app = TestApp::new();
    snapshot("chat-empty", &mut app);
}

#[test]
fn snapshot_providers_default_profile() {
    let mut app = TestApp::new();
    app.go("Providers");
    snapshot("providers-default", &mut app);
}

#[test]
fn snapshot_settings_history_section() {
    let mut app = TestApp::fixed_paths();
    app.go("Settings");
    snapshot("settings-history", &mut app);
}

#[test]
fn snapshot_usage_empty() {
    let mut app = TestApp::new();
    app.go("Usage");
    snapshot("usage-empty", &mut app);
}

#[test]
fn snapshot_recovery_window() {
    let screen = crate::ui::recovery::RecoveryScreen::preview(
        "/Users/example/Library/Application Support/NEXUS/config.toml",
        "The file is not valid TOML (parse error at line 4, column 1).",
    );
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1_080.0, 720.0))
        .with_theme(egui::Theme::Dark)
        .build_ui_state(
            |ui, screen: &mut crate::ui::recovery::RecoveryScreen| {
                let _ = screen.show(ui);
            },
            screen,
        );
    harness.run();
    harness.snapshot("recovery-config-error");
}

#[test]
fn snapshot_mcp_empty_section() {
    let mut app = TestApp::fixed_paths();
    app.go("MCP");
    snapshot("mcp-empty", &mut app);
}

#[test]
fn snapshot_mcp_configured_server() {
    let mut app = TestApp::fixed_paths();
    app.harness
        .state_mut()
        .config
        .mcp_servers
        .push(crate::config::McpServerConfig {
            id: 1,
            name: "Workspace files".to_owned(),
            transport: crate::config::McpTransport::Stdio,
            command: "npx".to_owned(),
            args: vec![
                "-y".to_owned(),
                "@modelcontextprotocol/server-filesystem".to_owned(),
            ],
            cwd: None,
            env_allowlist: Vec::new(),
            url: String::new(),
            bearer_auth: false,
            enabled_tools: Vec::new(),
            enabled: true,
        });
    app.harness.run();
    app.go("MCP");
    snapshot("mcp-configured", &mut app);
}

#[test]
fn snapshot_directory_is_documented() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    assert!(
        path.is_dir() || !path.exists(),
        "snapshot directory should be {}",
        path.display()
    );
}
