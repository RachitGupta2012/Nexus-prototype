use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const VENDOR_DOCS_REVIEWED_AT_UNIX_SECS: u64 = 1_794_401_854;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentId {
    CodexCli,
    ClaudeCode,
    OpenCode,
    Pi,
    GeminiCli,
    GitHubCopilotCli,
    Aider,
    Amp,
    Goose,
}

impl AgentId {
    pub const ALL: [Self; 9] = [
        Self::CodexCli,
        Self::ClaudeCode,
        Self::OpenCode,
        Self::Pi,
        Self::GeminiCli,
        Self::GitHubCopilotCli,
        Self::Aider,
        Self::Amp,
        Self::Goose,
    ];

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::CodexCli => "Codex CLI",
            Self::ClaudeCode => "Claude Code",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi coding agent",
            Self::GeminiCli => "Gemini CLI",
            Self::GitHubCopilotCli => "GitHub Copilot CLI",
            Self::Aider => "Aider",
            Self::Amp => "Amp CLI",
            Self::Goose => "Goose CLI",
        }
    }

    pub const fn executable_name(self) -> &'static str {
        match self {
            Self::CodexCli => "codex",
            Self::ClaudeCode => "claude",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
            Self::GeminiCli => "gemini",
            Self::GitHubCopilotCli => "copilot",
            Self::Aider => "aider",
            Self::Amp => "amp",
            Self::Goose => "goose",
        }
    }

    pub const fn documentation_url(self) -> &'static str {
        match self {
            Self::CodexCli => {
                "https://developers.openai.com/cookbook/examples/codex/using_goals_in_codex"
            }
            Self::ClaudeCode => "https://docs.anthropic.com/en/docs/claude-code/cli-usage",
            Self::OpenCode => "https://opencode.ai/docs/cli/commands",
            Self::Pi => "https://pi.dev/docs/latest",
            Self::GeminiCli => "https://github.com/google-gemini/gemini-cli",
            Self::GitHubCopilotCli => {
                "https://docs.github.com/en/copilot/get-started/cli-quickstart"
            }
            Self::Aider => "https://aider.chat/docs/usage.html",
            Self::Amp => "https://ampcode.com/docs/cli",
            Self::Goose => "https://goose-docs.ai/docs/getting-started/installation/",
        }
    }

    pub const fn version_command(self) -> Option<&'static str> {
        match self {
            Self::CodexCli => Some("--version"),
            Self::ClaudeCode => Some("doctor"),
            Self::OpenCode => Some("--version"),
            Self::Pi => None,
            Self::GeminiCli | Self::GitHubCopilotCli | Self::Aider | Self::Amp | Self::Goose => {
                None
            }
        }
    }

    pub const fn documented_interfaces(self) -> &'static [InteractionInterface] {
        match self {
            Self::CodexCli => &[InteractionInterface::InteractiveCli],
            Self::ClaudeCode => &[
                InteractionInterface::InteractiveCli,
                InteractionInterface::PrintCli,
                InteractionInterface::JsonOutput,
            ],
            Self::OpenCode => &[
                InteractionInterface::InteractiveCli,
                InteractionInterface::PrintCli,
                InteractionInterface::AcpServer,
            ],
            Self::Pi => &[
                InteractionInterface::InteractiveCli,
                InteractionInterface::PrintCli,
                InteractionInterface::JsonOutput,
                InteractionInterface::Rpc,
            ],
            Self::GeminiCli | Self::GitHubCopilotCli | Self::Aider | Self::Amp | Self::Goose => {
                &[InteractionInterface::InteractiveCli]
            }
        }
    }

    pub const fn basename_is_ambiguous(self) -> bool {
        matches!(self, Self::Pi | Self::Amp)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    Harness,
    Provider,
    Gateway,
    Model,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionInterface {
    InteractiveCli,
    PrintCli,
    JsonOutput,
    Rpc,
    AcpServer,
}

impl InteractionInterface {
    pub const fn label(self) -> &'static str {
        match self {
            Self::InteractiveCli => "Interactive CLI",
            Self::PrintCli => "Print mode CLI",
            Self::JsonOutput => "JSON output",
            Self::Rpc => "RPC interface",
            Self::AcpServer => "ACP server interface",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Evidence {
    pub description: String,
    pub source: Option<String>,
    pub observed_at_unix_secs: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", content = "details", rename_all = "snake_case")]
pub enum Knowledge<T> {
    Documented {
        value: T,
        evidence: Evidence,
    },
    Observed {
        value: T,
        evidence: Evidence,
    },
    Inferred {
        value: T,
        reason: String,
    },
    UserReported {
        value: T,
        evidence: Evidence,
    },
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionStatus {
    NotSearched,
    Found,
    NotFoundInScope,
    Inaccessible,
    Skipped,
    Ambiguous,
}

impl DetectionStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::NotSearched => "Not searched",
            Self::Found => "Detected executable",
            Self::NotFoundInScope => "Not found in search scope",
            Self::Inaccessible => "Location inaccessible",
            Self::Skipped => "Location skipped",
            Self::Ambiguous => "Ambiguous executable or multiple matches",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationStatus {
    DiscoveryOnly,
    AdapterNotImplemented,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutableLocation {
    pub discovered_path: PathBuf,
    pub resolved_path: PathBuf,
    pub search_root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRecord {
    pub id: AgentId,
    pub name: String,
    pub agent_type: AgentType,
    pub executable_name: String,
    pub locations: Vec<ExecutableLocation>,
    pub detection_status: DetectionStatus,
    pub installed_version: Knowledge<String>,
    pub interfaces: Vec<Knowledge<InteractionInterface>>,
    pub capabilities: Vec<Knowledge<String>>,
    pub integration_status: IntegrationStatus,
    pub ignored: bool,
    pub last_discovered_at_unix_secs: Option<u64>,
}

impl AgentRecord {
    pub fn catalog_entry(id: AgentId) -> Self {
        let evidence = Evidence {
            description:
                "Vendor documentation reviewed 2026-11-11; not verified against the installed copy"
                    .to_owned(),
            source: Some(id.documentation_url().to_owned()),
            observed_at_unix_secs: Some(VENDOR_DOCS_REVIEWED_AT_UNIX_SECS),
        };
        let interfaces = id
            .documented_interfaces()
            .iter()
            .copied()
            .map(|value| Knowledge::Documented {
                value,
                evidence: evidence.clone(),
            })
            .collect();

        Self {
            id,
            name: id.display_name().to_owned(),
            agent_type: AgentType::Harness,
            executable_name: id.executable_name().to_owned(),
            locations: Vec::new(),
            detection_status: DetectionStatus::NotSearched,
            installed_version: Knowledge::Unknown,
            interfaces,
            capabilities: vec![Knowledge::Unknown],
            integration_status: IntegrationStatus::DiscoveryOnly,
            ignored: false,
            last_discovered_at_unix_secs: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentId, AgentRecord, AgentType, DetectionStatus, IntegrationStatus, Knowledge};

    #[test]
    fn catalog_claims_are_documented_not_observed() {
        let record = AgentRecord::catalog_entry(AgentId::OpenCode);

        assert_eq!(record.detection_status, DetectionStatus::NotSearched);
        assert!(matches!(record.installed_version, Knowledge::Unknown));
        assert!(record.interfaces.iter().all(|claim| matches!(
            claim,
            Knowledge::Documented { evidence, .. }
                if evidence.description.contains("not verified")
        )));
        assert_eq!(record.integration_status, IntegrationStatus::DiscoveryOnly);
    }

    #[test]
    fn expanded_catalog_contains_only_named_harness_candidates() {
        assert_eq!(AgentId::ALL.len(), 9);
        assert_eq!(AgentId::GeminiCli.executable_name(), "gemini");
        assert_eq!(AgentId::GitHubCopilotCli.executable_name(), "copilot");
        assert_eq!(AgentId::Aider.executable_name(), "aider");
        assert_eq!(AgentId::Amp.executable_name(), "amp");
        assert_eq!(AgentId::Goose.executable_name(), "goose");
        for id in AgentId::ALL {
            let entry = AgentRecord::catalog_entry(id);
            assert_eq!(entry.agent_type, AgentType::Harness);
            assert_eq!(entry.integration_status, IntegrationStatus::DiscoveryOnly);
            assert!(matches!(entry.installed_version, Knowledge::Unknown));
        }
    }
}
