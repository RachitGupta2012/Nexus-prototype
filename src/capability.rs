//! Curated, typed capability claims. This catalog is separate from discovery.

use crate::agent::AgentId;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CapabilityId {
    CodeEditing,
    SoftwareDevelopment,
    Research,
    FileReading,
    FileEditing,
    FileCreation,
    StructuredOutput,
    ToolUsage,
    DataAnalysis,
    DocumentProcessing,
    Writing,
}

impl CapabilityId {
    pub const ALL: [Self; 11] = [
        Self::CodeEditing,
        Self::SoftwareDevelopment,
        Self::Research,
        Self::FileReading,
        Self::FileEditing,
        Self::FileCreation,
        Self::StructuredOutput,
        Self::ToolUsage,
        Self::DataAnalysis,
        Self::DocumentProcessing,
        Self::Writing,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::CodeEditing => "Code editing",
            Self::SoftwareDevelopment => "Software development",
            Self::Research => "Research",
            Self::FileReading => "File reading",
            Self::FileEditing => "File editing",
            Self::FileCreation => "File creation",
            Self::StructuredOutput => "Structured output",
            Self::ToolUsage => "Tool usage",
            Self::DataAnalysis => "Data analysis",
            Self::DocumentProcessing => "Document processing",
            Self::Writing => "Writing",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceState {
    Verified,
    Documented,
    UserReported,
    Inferred,
    Unsupported,
    Unknown,
}

impl EvidenceState {
    pub const ALL: [Self; 6] = [
        Self::Verified,
        Self::Documented,
        Self::UserReported,
        Self::Inferred,
        Self::Unsupported,
        Self::Unknown,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Verified => "Verified",
            Self::Documented => "Documented",
            Self::UserReported => "User-reported",
            Self::Inferred => "Inferred",
            Self::Unsupported => "Unsupported",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Freshness {
    Current,
    OlderVersion,
    Stale,
    Undated,
    InvalidDate,
    VersionUnverified,
    Conflicting,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityEvidence {
    pub state: EvidenceState,
    pub method: &'static str,
    pub source: Option<&'static str>,
    pub version: Option<&'static str>,
    pub reviewed_at_unix_secs: Option<u64>,
    pub summary: &'static str,
    pub limitation: Option<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityClaim {
    pub agent: AgentId,
    pub capability: CapabilityId,
    pub evidence: CapabilityEvidence,
}

const REVIEWED: u64 = 1_794_401_854;

const fn documented(
    agent: AgentId,
    capability: CapabilityId,
    summary: &'static str,
) -> CapabilityClaim {
    CapabilityClaim {
        agent,
        capability,
        evidence: CapabilityEvidence {
            state: EvidenceState::Documented,
            method: "Vendor documentation review",
            source: Some(agent.documentation_url()),
            version: None,
            reviewed_at_unix_secs: Some(REVIEWED),
            summary,
            limitation: Some(
                "Vendor documentation claim; not verified against the detected installation.",
            ),
        },
    }
}

/// Conservative initial claims: only software-development workflows are listed.
/// Other dimensions intentionally remain unknown until independently evidenced.
pub const CATALOG: &[CapabilityClaim] = &[
    documented(
        AgentId::CodexCli,
        CapabilityId::SoftwareDevelopment,
        "Codex CLI is documented as a coding agent.",
    ),
    documented(
        AgentId::ClaudeCode,
        CapabilityId::SoftwareDevelopment,
        "Claude Code is documented as a coding agent for software tasks.",
    ),
    documented(
        AgentId::OpenCode,
        CapabilityId::SoftwareDevelopment,
        "OpenCode is documented as an open-source coding agent.",
    ),
    documented(
        AgentId::Pi,
        CapabilityId::SoftwareDevelopment,
        "Pi is documented as a coding agent.",
    ),
    documented(
        AgentId::GeminiCli,
        CapabilityId::SoftwareDevelopment,
        "Gemini CLI is documented as a terminal coding agent.",
    ),
    documented(
        AgentId::GitHubCopilotCli,
        CapabilityId::SoftwareDevelopment,
        "GitHub Copilot CLI is documented as a coding assistant with agentic capabilities.",
    ),
    documented(
        AgentId::Aider,
        CapabilityId::SoftwareDevelopment,
        "Aider is documented as an AI pair-programming tool that edits source code.",
    ),
    documented(
        AgentId::Amp,
        CapabilityId::SoftwareDevelopment,
        "Amp is documented as a coding agent.",
    ),
    documented(
        AgentId::Goose,
        CapabilityId::SoftwareDevelopment,
        "Goose is documented as a general-purpose AI agent that can assist with software development.",
    ),
];

pub const MAX_EVIDENCE_AGE_SECS: u64 = 365 * 24 * 60 * 60;

pub fn freshness(
    evidence: &CapabilityEvidence,
    installed_version: Option<&str>,
    now: u64,
) -> Freshness {
    let Some(reviewed) = evidence.reviewed_at_unix_secs else {
        return Freshness::Undated;
    };
    if reviewed > now {
        return Freshness::InvalidDate;
    }
    if now.saturating_sub(reviewed) > MAX_EVIDENCE_AGE_SECS {
        return Freshness::Stale;
    }
    match evidence.version {
        Some(version) if installed_version == Some(version) => Freshness::Current,
        Some(_) if installed_version.is_some() => Freshness::OlderVersion,
        Some(_) => Freshness::VersionUnverified,
        None if installed_version.is_some() => Freshness::VersionUnverified,
        None => Freshness::VersionUnverified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_is_documented_and_version_unverified() {
        assert!(
            CATALOG
                .iter()
                .all(|claim| claim.evidence.state == EvidenceState::Documented)
        );
        let claim = &CATALOG[0];
        assert_eq!(
            freshness(&claim.evidence, None, REVIEWED),
            Freshness::VersionUnverified
        );
    }
    #[test]
    fn missing_and_old_dates_never_count_as_current() {
        let mut evidence = CATALOG[0].evidence.clone();
        evidence.reviewed_at_unix_secs = None;
        assert_eq!(freshness(&evidence, None, REVIEWED), Freshness::Undated);
        evidence.reviewed_at_unix_secs = Some(1);
        assert_eq!(
            freshness(&evidence, None, REVIEWED + MAX_EVIDENCE_AGE_SECS + 1),
            Freshness::Stale
        );
        evidence.reviewed_at_unix_secs = Some(REVIEWED + 1);
        assert_eq!(freshness(&evidence, None, REVIEWED), Freshness::InvalidDate);
    }
}
