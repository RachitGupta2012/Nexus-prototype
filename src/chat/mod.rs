//! Conversation domain types shared by the provider layer, storage, and UI.

pub mod controller;

use serde::{Deserialize, Serialize};

use crate::config::{ModelPricing, Protocol};

pub const MAX_USER_MESSAGE_BYTES: usize = 32 * 1024;
pub const MAX_ASSISTANT_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TOOL_ARGUMENT_BYTES: usize = 256 * 1024;
pub const MAX_TOOL_CALLS_PER_TURN: usize = 16;
pub const MAX_SYSTEM_PROMPT_BYTES: usize = 16 * 1024;

pub type ConversationId = i64;
pub type MessageId = i64;
pub type ToolCallId = String;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "tool" => Some(Self::Tool),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    #[default]
    Complete,
    Cancelled,
    Failed,
}

impl MessageStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "complete" => Some(Self::Complete),
            "cancelled" => Some(Self::Cancelled),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    /// JSON arguments exactly as returned by the provider.
    pub arguments: String,
}

impl ToolCall {
    pub fn arguments_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_str(&self.arguments)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
}

impl TokenUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    pub fn is_empty(&self) -> bool {
        self.input_tokens == 0 && self.output_tokens == 0 && self.cached_input_tokens == 0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostEstimate {
    pub amount: f64,
    pub currency: String,
    pub input_price_per_mtok: f64,
    pub output_price_per_mtok: f64,
    pub cached_input_price_per_mtok: Option<f64>,
}

impl CostEstimate {
    pub fn from_usage(pricing: &ModelPricing, usage: &TokenUsage) -> Option<Self> {
        if !pricing.input_per_mtok.is_finite()
            || !pricing.output_per_mtok.is_finite()
            || pricing.input_per_mtok < 0.0
            || pricing.output_per_mtok < 0.0
            || pricing.currency.trim().is_empty()
            || usage.is_empty()
        {
            return None;
        }
        let cached = usage.cached_input_tokens.min(usage.input_tokens);
        let uncached = usage.input_tokens.saturating_sub(cached);
        let cached_price = pricing
            .cached_input_per_mtok
            .filter(|price| price.is_finite() && *price >= 0.0);
        let cached_cost =
            cached as f64 * cached_price.unwrap_or(pricing.input_per_mtok) / 1_000_000.0;
        let input_cost = uncached as f64 * pricing.input_per_mtok / 1_000_000.0;
        let output_cost = usage.output_tokens as f64 * pricing.output_per_mtok / 1_000_000.0;
        Some(Self {
            amount: input_cost + cached_cost + output_cost,
            currency: pricing.currency.trim().to_owned(),
            input_price_per_mtok: pricing.input_per_mtok,
            output_price_per_mtok: pricing.output_per_mtok,
            cached_input_price_per_mtok: cached_price,
        })
    }

    pub fn label(&self) -> String {
        format!("~${:.4} {}", self.amount, self.currency)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: MessageId,
    pub role: MessageRole,
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub tool_call_id: Option<ToolCallId>,
    #[serde(default)]
    pub tool_name: Option<String>,
    pub status: MessageStatus,
    pub created_at: u64,
    #[serde(default)]
    pub usage: Option<TokenUsage>,
    #[serde(default)]
    pub cost: Option<CostEstimate>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: ConversationId,
    pub title: String,
    pub provider_id: u64,
    pub protocol: Protocol,
    pub model: String,
    pub system_prompt: String,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationSummary {
    pub id: ConversationId,
    pub title: String,
    pub provider_id: u64,
    pub model: String,
    pub updated_at: u64,
    pub message_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_uses_cached_and_uncached_input_prices() {
        let pricing = ModelPricing {
            input_per_mtok: 2.0,
            output_per_mtok: 8.0,
            cached_input_per_mtok: Some(0.5),
            currency: "USD".into(),
            reviewed_at_unix_secs: None,
        };
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cached_input_tokens: 400_000,
        };
        let cost = CostEstimate::from_usage(&pricing, &usage).unwrap();
        assert!(
            (cost.amount
                - (600_000.0 * 2.0 / 1_000_000.0
                    + 400_000.0 * 0.5 / 1_000_000.0
                    + 500_000.0 * 8.0 / 1_000_000.0))
                .abs()
                < f64::EPSILON
        );
        assert!(cost.label().starts_with("~$"));
    }

    #[test]
    fn cost_is_unknown_without_usage_or_valid_prices() {
        let pricing = ModelPricing {
            input_per_mtok: 1.0,
            output_per_mtok: 1.0,
            cached_input_per_mtok: None,
            currency: "USD".into(),
            reviewed_at_unix_secs: None,
        };
        assert!(CostEstimate::from_usage(&pricing, &TokenUsage::default()).is_none());
        let invalid = ModelPricing {
            input_per_mtok: f64::NAN,
            ..pricing
        };
        assert!(
            CostEstimate::from_usage(
                &invalid,
                &TokenUsage {
                    input_tokens: 1,
                    ..TokenUsage::default()
                }
            )
            .is_none()
        );
    }
}
