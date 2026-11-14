//! Usage dashboard: provider-reported tokens and clearly-labelled estimates.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use time::{OffsetDateTime, UtcOffset, macros::format_description};

use crate::config::Config;
use crate::storage::{Storage, UsageDay, UsageRow, UsageTotals};

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const DAYS_SHOWN: u32 = 14;

#[derive(Default)]
pub struct UsageOutcome {
    pub open_settings: bool,
}

struct UsageData {
    totals: UsageTotals,
    rows: Vec<UsageRow>,
    days: Vec<UsageDay>,
}

pub struct UsageScreen {
    data: Option<UsageData>,
    loaded_at: Option<Instant>,
    error: Option<String>,
}

impl UsageScreen {
    pub fn new() -> Self {
        Self {
            data: None,
            loaded_at: None,
            error: None,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, config: &Config, storage: &Storage) -> UsageOutcome {
        let mut outcome = UsageOutcome::default();
        let stale = self
            .loaded_at
            .is_none_or(|loaded| loaded.elapsed() >= REFRESH_INTERVAL);
        if stale {
            self.load(storage);
            self.loaded_at = Some(Instant::now());
        }

        ui.heading("Usage");
        ui.horizontal(|ui| {
            ui.small("Provider-reported token counts. Costs are estimates from your model prices.");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Settings").clicked() {
                    outcome.open_settings = true;
                }
                if ui.small_button("Refresh").clicked() {
                    self.load(storage);
                    self.loaded_at = Some(Instant::now());
                }
            });
        });

        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().warn_fg_color, error);
        }
        let Some(data) = &self.data else {
            return outcome;
        };
        if data.rows.is_empty() && data.totals.input_tokens == 0 && data.totals.output_tokens == 0 {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No usage recorded yet").strong());
                ui.small(
                    "Send a message with a saved conversation to start collecting provider usage.",
                );
                if !config.history_enabled {
                    ui.small("History is off, so usage is not recorded.");
                }
            });
            return outcome;
        }

        ui.add_space(12.0);
        ui.separator();
        ui.heading("Totals");
        egui::Grid::new("usage_totals")
            .num_columns(2)
            .spacing([24.0, 6.0])
            .show(ui, |ui| {
                ui.label("Input tokens");
                ui.strong(data.totals.input_tokens.to_string());
                ui.end_row();
                ui.label("Output tokens");
                ui.strong(data.totals.output_tokens.to_string());
                ui.end_row();
                ui.label("Cached input tokens");
                ui.strong(data.totals.cached_input_tokens.to_string());
                ui.end_row();
                ui.label("Total tokens");
                ui.strong(
                    data.totals
                        .input_tokens
                        .saturating_add(data.totals.output_tokens)
                        .to_string(),
                );
                ui.end_row();
                ui.label("Estimated cost");
                if data.totals.costs.is_empty() {
                    ui.weak("Unavailable — no model prices configured");
                } else {
                    ui.strong(
                        data.totals
                            .costs
                            .iter()
                            .map(|(currency, amount)| format!("~{amount:.4} {currency}"))
                            .collect::<Vec<_>>()
                            .join(" + "),
                    );
                }
                ui.end_row();
            });

        ui.add_space(4.0);
        ui.small(
            "Estimated means token counts × the prices you configured. It is not provider billing.",
        );

        ui.add_space(12.0);
        ui.separator();
        ui.heading("By provider");
        let providers = provider_totals(&data.rows);
        if providers.is_empty() {
            ui.weak("No provider-attributed usage yet.");
        } else {
            egui::Grid::new("usage_providers")
                .striped(true)
                .num_columns(4)
                .spacing([18.0, 4.0])
                .show(ui, |ui| {
                    ui.strong("Provider");
                    ui.strong("Input");
                    ui.strong("Output");
                    ui.strong("Estimated");
                    ui.end_row();
                    for (provider_id, usage) in providers {
                        ui.label(provider_name(config, provider_id));
                        ui.label(usage.input_tokens.to_string());
                        ui.label(usage.output_tokens.to_string());
                        ui.label(match usage.cost {
                            Some((currency, amount)) => format!("~{amount:.4} {currency}"),
                            None => "—".to_owned(),
                        });
                        ui.end_row();
                    }
                });
        }

        ui.add_space(12.0);
        ui.separator();
        ui.heading("By model");
        egui::Grid::new("usage_models")
            .striped(true)
            .num_columns(5)
            .spacing([18.0, 4.0])
            .show(ui, |ui| {
                ui.strong("Provider");
                ui.strong("Model");
                ui.strong("Input");
                ui.strong("Output");
                ui.strong("Estimated");
                ui.end_row();
                for row in &data.rows {
                    ui.label(provider_name(config, row.provider_id));
                    ui.label(&row.model);
                    ui.label(row.input_tokens.to_string());
                    ui.label(row.output_tokens.to_string());
                    ui.label(match (&row.currency, row.estimated_cost) {
                        (Some(currency), cost) if cost > 0.0 => {
                            format!("~{cost:.4} {currency}")
                        }
                        _ => "—".to_owned(),
                    });
                    ui.end_row();
                }
            });

        ui.add_space(12.0);
        ui.separator();
        ui.heading(format!("Last {DAYS_SHOWN} days"));
        if data.days.is_empty() {
            ui.weak("No daily usage in the retention window.");
        } else {
            egui::Grid::new("usage_days")
                .striped(true)
                .num_columns(4)
                .spacing([18.0, 4.0])
                .show(ui, |ui| {
                    ui.strong("Day (local)");
                    ui.strong("Input");
                    ui.strong("Output");
                    ui.strong("Estimated");
                    ui.end_row();
                    for day in &data.days {
                        ui.label(local_date(day.day_start_utc));
                        ui.label(day.input_tokens.to_string());
                        ui.label(day.output_tokens.to_string());
                        ui.label(if day.costs.is_empty() {
                            "—".to_owned()
                        } else {
                            day.costs
                                .iter()
                                .map(|(currency, amount)| format!("~{amount:.4} {currency}"))
                                .collect::<Vec<_>>()
                                .join(" + ")
                        });
                        ui.end_row();
                    }
                });
        }

        outcome
    }

    fn load(&mut self, storage: &Storage) {
        match (
            storage.usage_totals(),
            storage.usage_rows(),
            storage.usage_by_day(DAYS_SHOWN),
        ) {
            (Ok(totals), Ok(rows), Ok(days)) => {
                self.data = Some(UsageData { totals, rows, days });
                self.error = None;
            }
            (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
                self.error = Some(format!("Could not read usage: {error}"));
            }
        }
    }
}

fn provider_name(config: &Config, provider_id: u64) -> String {
    config
        .provider(provider_id)
        .map(|provider| provider.name.clone())
        .unwrap_or_else(|| format!("Removed provider #{provider_id}"))
}

#[derive(Default)]
struct ProviderUsage {
    input_tokens: u64,
    output_tokens: u64,
    cost: Option<(String, f64)>,
}

/// Aggregate per-provider rows without double counting sub-rows.
fn provider_totals(rows: &[UsageRow]) -> BTreeMap<u64, ProviderUsage> {
    let mut totals: BTreeMap<u64, ProviderUsage> = BTreeMap::new();
    for row in rows {
        let entry = totals.entry(row.provider_id).or_default();
        entry.input_tokens = entry.input_tokens.saturating_add(row.input_tokens);
        entry.output_tokens = entry.output_tokens.saturating_add(row.output_tokens);
        if let Some(currency) = &row.currency {
            let slot = entry.cost.get_or_insert((currency.clone(), 0.0));
            if slot.0 == *currency {
                slot.1 += row.estimated_cost;
            }
        }
    }
    totals
}

/// UTC day bucket rendered in the machine's local time zone.
pub fn local_date(day_start_utc: u64) -> String {
    let Ok(timestamp) = i64::try_from(day_start_utc) else {
        return "unknown".to_owned();
    };
    let Ok(datetime) = OffsetDateTime::from_unix_timestamp(timestamp) else {
        return "unknown".to_owned();
    };
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    datetime
        .to_offset(offset)
        .format(format_description!("[year]-[month]-[day]"))
        .unwrap_or_else(|_| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Protocol;

    fn row(provider_id: u64, model: &str, input: u64, output: u64) -> UsageRow {
        UsageRow {
            provider_id,
            protocol: Protocol::OpenAiChat,
            model: model.to_owned(),
            message_count: 1,
            input_tokens: input,
            output_tokens: output,
            cached_input_tokens: 0,
            estimated_cost: 0.001,
            currency: Some("USD".to_owned()),
        }
    }

    #[test]
    fn usage_renders_its_empty_state() {
        let storage = Storage::open_in_memory().unwrap();
        let config = Config::default();
        let mut screen = UsageScreen::new();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1_080.0, 720.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            screen.show(ui, &config, &storage);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn provider_rows_are_summed_once() {
        let rows = vec![row(1, "a", 10, 2), row(1, "b", 5, 1), row(2, "c", 3, 3)];
        let totals = provider_totals(&rows);
        assert_eq!(totals[&1].input_tokens, 15);
        assert_eq!(totals[&1].output_tokens, 3);
        assert_eq!(totals[&2].input_tokens, 3);
        let (currency, cost) = totals[&1].cost.as_ref().unwrap();
        assert_eq!(currency, "USD");
        assert!((cost - 0.002).abs() < 1e-9);
    }

    #[test]
    fn local_date_converts_utc_day_boundaries() {
        // 2023-11-14T00:00:00Z
        let day = 1_699_920_000;
        let utc = OffsetDateTime::from_unix_timestamp(day as i64).unwrap();
        let tokyo = utc.to_offset(UtcOffset::from_hms(9, 0, 0).unwrap());
        let los_angeles = utc.to_offset(UtcOffset::from_hms(-8, 0, 0).unwrap());
        assert_eq!(tokyo.date().to_string(), "2023-11-14");
        assert_eq!(los_angeles.date().to_string(), "2023-11-13");
        assert_eq!(local_date(day).len(), 10);
        assert_eq!(local_date(u64::MAX), "unknown");
    }
}
