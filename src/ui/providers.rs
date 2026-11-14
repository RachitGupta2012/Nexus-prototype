//! Provider profiles: endpoints, model catalogs, and Keychain-backed keys.

use std::path::Path;

use eframe::egui::{self, Align, Layout, RichText, TextEdit};
use url::Url;

use crate::config::{AuthMode, Config, ModelConfig, ModelPricing, Protocol, ProviderProfile};
use crate::credentials;
use crate::provider::ProviderSettings;

const LIST_WIDTH: f32 = 230.0;

struct Preset {
    label: &'static str,
    name: &'static str,
    protocol: Protocol,
    base_url: &'static str,
    auth: AuthMode,
    model: &'static str,
}

const PRESETS: [Preset; 8] = [
    Preset {
        label: "OpenAI · Chat Completions",
        name: "OpenAI",
        protocol: Protocol::OpenAiChat,
        base_url: "https://api.openai.com/v1",
        auth: AuthMode::Bearer,
        model: "gpt-4.1",
    },
    Preset {
        label: "OpenAI · Responses",
        name: "OpenAI Responses",
        protocol: Protocol::OpenAiResponses,
        base_url: "https://api.openai.com/v1",
        auth: AuthMode::Bearer,
        model: "gpt-4.1",
    },
    Preset {
        label: "Anthropic · Messages",
        name: "Anthropic",
        protocol: Protocol::AnthropicMessages,
        base_url: "https://api.anthropic.com/v1",
        auth: AuthMode::XApiKey,
        model: "claude-sonnet-4-5",
    },
    Preset {
        label: "Google · Gemini",
        name: "Google Gemini",
        protocol: Protocol::GeminiGenerateContent,
        base_url: "https://generativelanguage.googleapis.com/v1beta",
        auth: AuthMode::XGoogApiKey,
        model: "gemini-2.5-flash",
    },
    Preset {
        label: "Ollama · native",
        name: "Ollama",
        protocol: Protocol::OllamaChat,
        base_url: "http://127.0.0.1:11434",
        auth: AuthMode::None,
        model: "llama3.2",
    },
    Preset {
        label: "Local gateway · FreeLLMAPI",
        name: "Local gateway",
        protocol: Protocol::OpenAiChat,
        base_url: "http://127.0.0.1:3001/v1",
        auth: AuthMode::None,
        model: "auto",
    },
    Preset {
        label: "LM Studio · local",
        name: "LM Studio",
        protocol: Protocol::OpenAiChat,
        base_url: "http://127.0.0.1:1234/v1",
        auth: AuthMode::None,
        model: "auto",
    },
    Preset {
        label: "LiteLLM · local",
        name: "LiteLLM",
        protocol: Protocol::OpenAiChat,
        base_url: "http://127.0.0.1:4000/v1",
        auth: AuthMode::None,
        model: "auto",
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum KeyState {
    Present,
    Missing,
    Unavailable(String),
}

pub struct ProvidersScreen {
    selected: Option<u64>,
    key_input: String,
    key_state: Option<(u64, String, KeyState)>,
    pending_remove_key: bool,
    delete_request: Option<u64>,
    default_request: Option<u64>,
    status: Option<String>,
}

impl ProvidersScreen {
    pub fn new(config: &Config) -> Self {
        Self {
            selected: config.default_provider().map(|provider| provider.id),
            key_input: String::new(),
            key_state: None,
            pending_remove_key: false,
            delete_request: None,
            default_request: None,
            status: None,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, config: &mut Config, config_path: &Path) {
        self.sync_selection(config);
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(LIST_WIDTH, height),
                Layout::top_down(Align::LEFT),
                |ui| self.list(ui, config),
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width().max(360.0), height),
                Layout::top_down(Align::LEFT),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("provider_editor")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.editor(ui, config, config_path));
                },
            );
        });

        self.apply_requests(ui, config);
    }

    fn sync_selection(&mut self, config: &Config) {
        let valid = self.selected.and_then(|id| config.provider(id)).is_some();
        if !valid {
            self.selected = config.default_provider().map(|provider| provider.id);
            self.key_state = None;
            self.pending_remove_key = false;
        }
    }

    fn list(&mut self, ui: &mut egui::Ui, config: &mut Config) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading("Providers");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button("+ Add", |ui| {
                    for preset in &PRESETS {
                        if ui.button(preset.label).clicked() {
                            let id = config.allocate_provider_id();
                            let mut profile = ProviderProfile::new(
                                id,
                                preset.name,
                                preset.protocol,
                                preset.base_url,
                                preset.auth,
                                preset.model,
                            );
                            profile.ensure_model(preset.model);
                            config.providers.push(profile);
                            self.selected = Some(id);
                            self.key_state = None;
                            self.pending_remove_key = false;
                            self.status = Some(format!(
                                "Added {}. Set a key, then save the configuration.",
                                preset.name
                            ));
                            ui.close();
                        }
                    }
                });
            });
        });
        ui.separator();

        let mut selected = self.selected;
        egui::ScrollArea::vertical()
            .id_salt("provider_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for provider in &config.providers {
                    let is_default = config.default_provider_id == Some(provider.id);
                    let label = if is_default {
                        format!("{}  ·  default", provider.name)
                    } else {
                        provider.name.clone()
                    };
                    if ui
                        .selectable_label(selected == Some(provider.id), label)
                        .on_hover_text(&provider.base_url)
                        .clicked()
                    {
                        selected = Some(provider.id);
                    }
                    ui.small(format!(
                        "{} · {}",
                        provider.protocol.short_label(),
                        provider.default_model
                    ));
                    ui.add_space(4.0);
                }
            });
        if selected != self.selected {
            self.selected = selected;
            self.key_input.clear();
            self.key_state = None;
            self.pending_remove_key = false;
            self.status = None;
        }

        ui.separator();
        if let Some(status) = &self.status {
            ui.small(status);
        }
        ui.add_space(6.0);
        ui.small("Secrets live in the macOS Keychain. Only endpoints and model prices are written to disk. Settings saves this same file.");
    }

    fn editor(&mut self, ui: &mut egui::Ui, config: &mut Config, config_path: &Path) {
        let Some(id) = self.selected else {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Add a provider to begin").strong());
                ui.small("Use “+ Add” to start from a preset.");
            });
            return;
        };
        if config.provider(id).is_none() {
            return;
        }

        if self.key_state.is_none()
            && let Some(profile) = config.provider(id)
            && profile.auth != AuthMode::None
        {
            self.check_key(profile);
        }

        ui.add_space(8.0);
        let is_default = config.default_provider_id == Some(id);
        let mut delete = false;
        let mut make_default = false;
        if let Some(provider) = config.provider_mut(id) {
            ui.horizontal(|ui| {
                ui.add(
                    TextEdit::singleline(&mut provider.name)
                        .desired_width(220.0)
                        .font(egui::TextStyle::Heading),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    delete = ui.button("Delete").clicked();
                    if !is_default {
                        make_default = ui.button("Set default").clicked();
                    }
                });
            });
            ui.add_space(4.0);
            ui.small(format!(
                "Profile {} · keys are scoped to this URL’s origin",
                provider.id
            ));
            ui.add_space(8.0);

            let tools_supported = provider.protocol.supports_tools();
            let previous_protocol = provider.protocol;
            let mut chosen_protocol = previous_protocol;
            egui::Grid::new("provider_fields")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Protocol");
                    egui::ComboBox::from_id_salt("provider_protocol")
                        .selected_text(provider.protocol.label())
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for protocol in Protocol::ALL {
                                ui.selectable_value(
                                    &mut chosen_protocol,
                                    protocol,
                                    protocol.label(),
                                );
                            }
                        });
                    ui.end_row();

                    ui.label("Base URL");
                    ui.add(
                        TextEdit::singleline(&mut provider.base_url)
                            .desired_width(f32::INFINITY)
                            .hint_text("https://api.example.com/v1"),
                    );
                    ui.end_row();

                    ui.label("Authentication");
                    egui::ComboBox::from_id_salt("provider_auth")
                        .selected_text(provider.auth.label())
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for auth in provider.protocol.allowed_auth() {
                                ui.selectable_value(&mut provider.auth, *auth, auth.label());
                            }
                        });
                    ui.end_row();

                    ui.label("Default model");
                    ui.add(
                        TextEdit::singleline(&mut provider.default_model)
                            .desired_width(f32::INFINITY)
                            .hint_text("model id"),
                    );
                    ui.end_row();
                });

            if chosen_protocol != previous_protocol {
                if provider.base_url.trim().is_empty()
                    || provider.base_url.trim() == previous_protocol.default_base_url()
                {
                    provider.base_url = chosen_protocol.default_base_url().to_owned();
                }
                if !chosen_protocol.allowed_auth().contains(&provider.auth) {
                    provider.auth = chosen_protocol.default_auth();
                }
                provider.protocol = chosen_protocol;
                self.key_state = None;
                self.pending_remove_key = false;
            }

            ui.add_space(6.0);
            let validation_model = if provider.default_model.trim().is_empty() {
                "auto"
            } else {
                provider.default_model.trim()
            };
            let settings = ProviderSettings {
                profile_id: provider.id,
                protocol: provider.protocol,
                base_url: provider.base_url.clone(),
                auth: provider.auth,
                model: validation_model.to_owned(),
            };
            match settings.validate() {
                Ok(_) => {
                    ui.small("Endpoint and authentication look valid.");
                }
                Err(error) => {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("Check this profile: {error}"),
                    );
                }
            }

            ui.add_space(10.0);
            self.key_section(ui, provider);

            ui.separator();
            ui.horizontal(|ui| {
                ui.heading("Models");
                if ui.button("+ Add model").clicked() {
                    provider.models.push(ModelConfig::new(format!(
                        "model-{}",
                        provider.models.len() + 1
                    )));
                }
            });
            ui.small("Prices are per million tokens. Estimates are shown only when both usage and prices exist.");
            ui.add_space(6.0);

            let mut remove_model = None;
            for (index, model) in provider.models.iter_mut().enumerate() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label("ID");
                        ui.add(TextEdit::singleline(&mut model.id).desired_width(180.0));
                        ui.label("Name");
                        ui.add(TextEdit::singleline(&mut model.display_name).desired_width(160.0));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("×").on_hover_text("Remove model").clicked() {
                                remove_model = Some(index);
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        let mut has_context = model.context_window.is_some();
                        if ui.checkbox(&mut has_context, "context").changed() {
                            model.context_window = has_context.then_some(8_192);
                        }
                        if let Some(context) = &mut model.context_window {
                            ui.add(
                                egui::DragValue::new(context)
                                    .speed(256.0)
                                    .range(0..=100_000_000),
                            );
                        }

                        let mut supports_tools = model.supports_tools.unwrap_or(false);
                        if ui
                            .add_enabled(
                                tools_supported,
                                egui::Checkbox::new(&mut supports_tools, "tools"),
                            )
                            .on_disabled_hover_text(
                                "This provider protocol has no tool-call support in this build.",
                            )
                            .changed()
                        {
                            model.supports_tools = Some(supports_tools);
                        }

                        let mut has_pricing = model.pricing.is_some();
                        if ui.checkbox(&mut has_pricing, "pricing").changed() {
                            model.pricing = has_pricing.then(default_pricing);
                        }
                        if let Some(pricing) = &mut model.pricing {
                            ui.add(
                                egui::DragValue::new(&mut pricing.input_per_mtok)
                                    .speed(0.01)
                                    .prefix("$")
                                    .suffix(" in/M"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut pricing.output_per_mtok)
                                    .speed(0.01)
                                    .prefix("$")
                                    .suffix(" out/M"),
                            );
                            let mut has_cached = pricing.cached_input_per_mtok.is_some();
                            if ui.checkbox(&mut has_cached, "cached").changed() {
                                pricing.cached_input_per_mtok = has_cached.then_some(0.0);
                            }
                            if let Some(cached) = &mut pricing.cached_input_per_mtok {
                                ui.add(
                                    egui::DragValue::new(cached)
                                        .speed(0.01)
                                        .prefix("$")
                                        .suffix(" cached/M"),
                                );
                            }
                            ui.add(TextEdit::singleline(&mut pricing.currency).desired_width(44.0));
                        }
                    });
                });
                ui.add_space(4.0);
            }
            if let Some(index) = remove_model {
                provider.models.remove(index);
            }

            let default_model = provider.default_model.clone();
            if !default_model.trim().is_empty()
                && !provider
                    .models
                    .iter()
                    .any(|model| model.id == default_model)
                && ui
                    .small_button(format!("Add “{default_model}” to the catalog"))
                    .clicked()
            {
                provider.ensure_model(&default_model);
            }

            ui.add_space(10.0);
            if ui.button("Save provider configuration").clicked() {
                self.status = Some(match config.save(config_path) {
                    Ok(()) => "Configuration saved.".to_owned(),
                    Err(error) => format!("Could not save the configuration: {error}"),
                });
            }
        }

        if delete {
            self.delete_request = Some(id);
        }
        if make_default {
            self.default_request = Some(id);
        }
    }

    fn key_section(&mut self, ui: &mut egui::Ui, provider: &mut ProviderProfile) {
        ui.heading("API key");
        if provider.auth == AuthMode::None {
            ui.small("This profile sends no credentials. Remote hosts must use HTTPS with a key.");
            self.key_state = None;
            return;
        }

        let Some(account) = account_for(provider) else {
            ui.small("Enter a valid base URL to manage the key.");
            return;
        };

        ui.horizontal(|ui| {
            match self.key_state.as_ref().map(|(_, _, state)| state.clone()) {
                Some(KeyState::Present) => {
                    ui.label(
                        RichText::new("Key saved in Keychain")
                            .color(egui::Color32::from_rgb(0x3d, 0x9a, 0x5f)),
                    );
                }
                Some(KeyState::Missing) => {
                    ui.label("No key saved yet.");
                }
                Some(KeyState::Unavailable(error)) => {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("Keychain unavailable: {error}"),
                    );
                }
                None => {
                    ui.small("Keychain status unknown.");
                }
            }
            if ui.small_button("Refresh").clicked() {
                self.key_state = None;
            }
        });

        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.key_input)
                    .password(true)
                    .desired_width(260.0)
                    .hint_text("Paste API key"),
            );
            if ui
                .add_enabled(
                    !self.key_input.trim().is_empty(),
                    egui::Button::new("Save key"),
                )
                .clicked()
            {
                match credentials::store(&account, &self.key_input) {
                    Ok(()) => {
                        self.key_input.clear();
                        self.load_key_state(provider.id, &account, KeyState::Present);
                        self.status = Some("API key saved in the Keychain.".to_owned());
                    }
                    Err(error) => {
                        self.status = Some(format!("Could not save the key: {error}"));
                    }
                }
            }
            if ui
                .add_enabled(self.pending_remove_key, egui::Button::new("Confirm remove"))
                .clicked()
            {
                match credentials::delete(&account) {
                    Ok(()) => {
                        self.pending_remove_key = false;
                        self.load_key_state(provider.id, &account, KeyState::Missing);
                        self.status = Some("API key removed from the Keychain.".to_owned());
                    }
                    Err(error) => {
                        self.status = Some(format!("Could not remove the key: {error}"));
                    }
                }
            }
            if !self.pending_remove_key && ui.button("Remove key").clicked() {
                self.pending_remove_key = true;
                self.status = Some("Click “Confirm remove” to delete the stored key.".to_owned());
            }
        });
    }

    fn check_key(&mut self, profile: &ProviderProfile) {
        let Some(account) = account_for(profile) else {
            return;
        };
        match credentials::load(&account) {
            Ok(Some(_)) => self.load_key_state(profile.id, &account, KeyState::Present),
            Ok(None) => self.load_key_state(profile.id, &account, KeyState::Missing),
            Err(error) => self.load_key_state(
                profile.id,
                &account,
                KeyState::Unavailable(error.to_string()),
            ),
        }
    }

    fn load_key_state(&mut self, id: u64, account: &str, state: KeyState) {
        let url = account
            .split_once('|')
            .map(|(_, url)| url)
            .unwrap_or_default();
        self.key_state = Some((id, url.to_owned(), state));
    }

    fn apply_requests(&mut self, ui: &mut egui::Ui, config: &mut Config) {
        if let Some(id) = self.delete_request.take() {
            if let Some(account) = config.provider(id).and_then(account_for) {
                let _ = credentials::delete(&account);
            }
            if config.remove_provider(id) {
                self.selected = None;
                self.key_state = None;
                self.pending_remove_key = false;
                self.status =
                    Some("Provider removed. Save the configuration to keep it.".to_owned());
            } else {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "At least one provider profile is required.",
                );
            }
        }
        if let Some(id) = self.default_request.take() {
            config.default_provider_id = Some(id);
            self.status = Some("Default provider updated. Save the configuration.".to_owned());
        }
    }
}

fn account_for(provider: &ProviderProfile) -> Option<String> {
    let url = Url::parse(provider.base_url.trim()).ok()?;
    url.host_str()?;
    Some(credentials::provider_account(
        provider.id,
        &url.origin().ascii_serialization(),
    ))
}

fn default_pricing() -> ModelPricing {
    ModelPricing {
        input_per_mtok: 0.0,
        output_per_mtok: 0.0,
        cached_input_per_mtok: None,
        currency: "USD".to_owned(),
        reviewed_at_unix_secs: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_renders_with_the_default_profile() {
        let mut config = Config::default();
        let mut screen = ProvidersScreen::new(&config);
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1_080.0, 720.0),
            )),
            ..Default::default()
        };
        let path = std::path::Path::new("/tmp/nexus-config.toml");
        let mut output = ctx.run_ui(input, |ui| {
            screen.show(ui, &mut config, path);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn presets_are_consistent_and_keep_their_model_in_the_catalog() {
        for preset in &PRESETS {
            assert!(
                preset.protocol.allowed_auth().contains(&preset.auth),
                "{} has an incompatible auth mode",
                preset.name
            );
            assert!(!preset.model.trim().is_empty());
            let profile = ProviderProfile::new(
                1,
                preset.name,
                preset.protocol,
                preset.base_url,
                preset.auth,
                preset.model,
            );
            let settings = ProviderSettings {
                profile_id: 1,
                protocol: profile.protocol,
                base_url: profile.base_url.clone(),
                auth: profile.auth,
                model: profile.default_model.clone(),
            };
            assert!(settings.validate().is_ok(), "{} is invalid", preset.name);
        }
    }

    #[test]
    fn key_accounts_use_the_origin_not_the_path() {
        let profile = ProviderProfile::new(
            4,
            "A",
            Protocol::OpenAiChat,
            "https://api.example.com/v1",
            AuthMode::Bearer,
            "m",
        );
        assert_eq!(
            account_for(&profile).as_deref(),
            Some("provider:4|https://api.example.com")
        );
        let mut broken = profile;
        broken.base_url = "not a url".into();
        assert!(account_for(&broken).is_none());
    }
}
