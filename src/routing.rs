//! Pure deterministic recommendation evaluation; this module has no host or I/O access.

use crate::agent::{AgentId, AgentRecord, DetectionStatus, Knowledge};
use crate::capability::{self, CapabilityClaim, CapabilityId, EvidenceState, Freshness};
use crate::task::{Task, TaskCategory};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Outcome {
    Supported,
    Tentative,
    InsufficientEvidence,
    Rejected,
    Excluded,
}

impl Outcome {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Supported => "Supported",
            Self::Tentative => "Tentative",
            Self::InsufficientEvidence => "Insufficient evidence",
            Self::Rejected => "Rejected",
            Self::Excluded => "Excluded",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequirementResult {
    pub capability: CapabilityId,
    pub mandatory: bool,
    pub state: EvidenceState,
    pub freshness: Option<Freshness>,
    pub evidence_summary: Option<&'static str>,
    pub evidence_method: Option<&'static str>,
    pub evidence_source: Option<&'static str>,
    pub evidence_version: Option<&'static str>,
    pub reviewed_at_unix_secs: Option<u64>,
    pub limitation: Option<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateResult {
    pub agent: AgentId,
    pub display_name: String,
    pub discovery_status: DetectionStatus,
    pub outcome: Outcome,
    pub requirements: Vec<RequirementResult>,
    pub category_evidence: Option<RequirementResult>,
    pub matched_preferences: Vec<CapabilityId>,
    pub unknown_preferences: Vec<CapabilityId>,
    pub explanation: Vec<String>,
    pub limitations: Vec<String>,
    pub tied_for_primary: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecommendationSet {
    pub candidates: Vec<CandidateResult>,
    pub primary: Vec<AgentId>,
    pub has_sufficient_recommendation: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum EvidenceRank {
    None,
    Documented,
    Verified,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Rank {
    outcome: Outcome,
    evidence: EvidenceRank,
    category: EvidenceRank,
    preferences: usize,
}

impl Rank {
    fn cmp(self, other: Self) -> std::cmp::Ordering {
        (self.outcome, self.evidence, self.category, self.preferences).cmp(&(
            other.outcome,
            other.evidence,
            other.category,
            other.preferences,
        ))
    }
}

pub fn recommend(
    task: &Task,
    records: &[AgentRecord],
    catalog: &[CapabilityClaim],
    now_unix_secs: u64,
) -> RecommendationSet {
    let mut candidates: Vec<_> = records
        .iter()
        .map(|record| evaluate(task, record, catalog, now_unix_secs))
        .collect();
    candidates.sort_by(|a, b| {
        rank(a)
            .cmp(rank(b))
            .reverse()
            .then_with(|| a.agent.cmp(&b.agent))
    });
    let best = candidates
        .iter()
        .filter(|item| item.outcome != Outcome::Rejected && item.outcome != Outcome::Excluded)
        .map(rank)
        .max_by(|left, right| (*left).cmp(*right));
    let primary: Vec<_> = best
        .filter(|best_rank| matches!(best_rank.outcome, Outcome::Supported | Outcome::Tentative))
        .map(|best_rank| {
            candidates
                .iter()
                .filter(|item| rank(item).cmp(best_rank).is_eq())
                .map(|item| item.agent)
                .collect()
        })
        .unwrap_or_default();
    if let Some(best_rank) = best
        .filter(|best_rank| matches!(best_rank.outcome, Outcome::Supported | Outcome::Tentative))
    {
        let primary_name = candidates
            .iter()
            .find(|item| rank(item).cmp(best_rank).is_eq())
            .map(|item| item.display_name.clone())
            .unwrap_or_else(|| "the primary group".into());
        for candidate in &mut candidates {
            if rank(candidate).cmp(best_rank).is_lt()
                && candidate.outcome != Outcome::Rejected
                && candidate.outcome != Outcome::Excluded
            {
                let candidate_rank = rank(candidate);
                let dimension = if candidate_rank.outcome != best_rank.outcome {
                    "outcome"
                } else if candidate_rank.evidence != best_rank.evidence {
                    "mandatory evidence quality"
                } else if candidate_rank.category != best_rank.category {
                    "category evidence"
                } else {
                    "matched optional preferences"
                };
                candidate.explanation.push(format!(
                    "Ranked below {primary_name} because its {dimension} comparison is lower."
                ));
            }
        }
    }
    for item in &mut candidates {
        item.tied_for_primary = primary.contains(&item.agent) && primary.len() > 1;
    }
    let has_sufficient_recommendation = candidates
        .iter()
        .any(|item| matches!(item.outcome, Outcome::Supported | Outcome::Tentative));
    RecommendationSet {
        candidates,
        primary,
        has_sufficient_recommendation,
    }
}

fn rank(result: &CandidateResult) -> Rank {
    let quality = |state, freshness| match (state, freshness) {
        (EvidenceState::Verified, Some(Freshness::Current)) => EvidenceRank::Verified,
        (EvidenceState::Documented, Some(Freshness::Current | Freshness::VersionUnverified)) => {
            EvidenceRank::Documented
        }
        _ => EvidenceRank::None,
    };
    let mandatory: Vec<_> = result.requirements.iter().filter(|r| r.mandatory).collect();
    let evidence = if mandatory.is_empty() {
        EvidenceRank::None
    } else {
        mandatory
            .iter()
            .map(|r| quality(r.state, r.freshness))
            .min()
            .unwrap_or(EvidenceRank::None)
    };
    let category = result
        .category_evidence
        .as_ref()
        .map_or(EvidenceRank::None, |r| quality(r.state, r.freshness));
    Rank {
        outcome: result.outcome,
        evidence,
        category,
        preferences: result.matched_preferences.len(),
    }
}

fn evaluate(
    task: &Task,
    record: &AgentRecord,
    catalog: &[CapabilityClaim],
    now: u64,
) -> CandidateResult {
    let mut result = CandidateResult {
        agent: record.id,
        display_name: record.name.clone(),
        discovery_status: record.detection_status,
        outcome: Outcome::InsufficientEvidence,
        requirements: vec![],
        category_evidence: None,
        matched_preferences: vec![],
        unknown_preferences: vec![],
        explanation: vec![],
        limitations: vec![],
        tied_for_primary: false,
    };
    if record.ignored {
        result.outcome = Outcome::Excluded;
        result
            .explanation
            .push("Candidate is ignored in the registry.".into());
        return result;
    }
    if record.detection_status == DetectionStatus::Ambiguous {
        result.outcome = Outcome::Excluded;
        result
            .explanation
            .push("Ambiguous executable identity is excluded by default.".into());
        return result;
    }
    if record.detection_status != DetectionStatus::Found || record.locations.is_empty() {
        result.outcome = Outcome::Excluded;
        result
            .explanation
            .push("No usable found executable record is available.".into());
        return result;
    }
    let version = match &record.installed_version {
        Knowledge::Observed { value, .. } => Some(value.as_str()),
        _ => None,
    };
    let mut rejected = false;
    let mut unresolved = false;
    let mut all_verified = true;
    for capability in &task.mandatory {
        let evaluated = find_claim(record.id, *capability, version, catalog, now);
        if evaluated.state == EvidenceState::Unsupported && is_fresh(evaluated.freshness) {
            rejected = true;
        }
        if evaluated.state != EvidenceState::Verified
            || evaluated.freshness != Some(Freshness::Current)
        {
            all_verified = false;
        }
        if !is_usable(&evaluated) {
            unresolved = true;
        }
        result.requirements.push(evaluated);
    }
    let category_cap = category_capability(task.category);
    let mut category = find_claim(record.id, category_cap, version, catalog, now);
    category.mandatory = false;
    if category.state == EvidenceState::Unsupported && is_fresh(category.freshness) {
        rejected = true;
    }
    let category_known = is_usable(&category);
    if category_known && category.state != EvidenceState::Verified {
        all_verified = false;
    }
    if !category_known {
        unresolved = true;
    }
    if let Some(limitation) = category.limitation {
        result.limitations.push(limitation.to_owned());
    }
    result.category_evidence = Some(category);
    for capability in &task.preferences {
        let mut claim = find_claim(record.id, *capability, version, catalog, now);
        claim.mandatory = false;
        if is_usable(&claim) {
            result.matched_preferences.push(*capability);
        } else {
            result.unknown_preferences.push(*capability);
        }
        if let Some(limitation) = claim.limitation {
            result.limitations.push(limitation.to_owned());
        }
    }
    if rejected {
        result.outcome = Outcome::Rejected;
        result.explanation.push("A mandatory requirement has current evidence that it is unsupported; optional strengths cannot offset it.".into());
    } else if unresolved {
        result.outcome = Outcome::InsufficientEvidence;
        result.explanation.push("One or more mandatory requirements or category compatibility lack current, usable evidence.".into());
    } else if all_verified {
        result.outcome = Outcome::Supported;
        result.explanation.push("All mandatory requirements and category compatibility have current, version-relevant verification.".into());
    } else {
        result.outcome = Outcome::Tentative;
        result.explanation.push("Mandatory requirements and category compatibility are documented, but not verified for this installed copy.".into());
    }
    if version.is_none() {
        result.limitations.push("Installed version is not independently observed; documentation is not version-matched.".into());
    }
    for requirement in &result.requirements {
        if requirement.state == EvidenceState::Unknown
            || requirement.freshness.is_some_and(|f| {
                matches!(
                    f,
                    Freshness::Stale
                        | Freshness::Undated
                        | Freshness::InvalidDate
                        | Freshness::OlderVersion
                )
            })
        {
            result.explanation.push(format!(
                "{}: {} evidence ({:?}).",
                requirement.capability.label(),
                requirement.state_label(),
                requirement.freshness
            ));
        }
        if let Some(limitation) = requirement.limitation {
            result.limitations.push(limitation.to_owned());
        }
    }
    if !result.matched_preferences.is_empty() {
        result.explanation.push(format!(
            "Optional preferences with usable evidence: {}.",
            result
                .matched_preferences
                .iter()
                .map(|c| c.label())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !result.unknown_preferences.is_empty() {
        result.explanation.push(format!(
            "Optional preferences remain unknown: {}.",
            result
                .unknown_preferences
                .iter()
                .map(|c| c.label())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    result
}

impl RequirementResult {
    fn state_label(&self) -> &'static str {
        match self.state {
            EvidenceState::Verified => "verified",
            EvidenceState::Documented => "documented",
            EvidenceState::UserReported => "user-reported",
            EvidenceState::Inferred => "inferred",
            EvidenceState::Unsupported => "unsupported",
            EvidenceState::Unknown => "unknown",
        }
    }
}

fn is_usable(result: &RequirementResult) -> bool {
    matches!(
        result.state,
        EvidenceState::Verified | EvidenceState::Documented
    ) && matches!(
        result.freshness,
        Some(Freshness::Current | Freshness::VersionUnverified)
    )
}

fn is_fresh(freshness: Option<Freshness>) -> bool {
    matches!(
        freshness,
        Some(Freshness::Current | Freshness::VersionUnverified)
    )
}

fn find_claim(
    agent: AgentId,
    capability: CapabilityId,
    version: Option<&str>,
    catalog: &[CapabilityClaim],
    now: u64,
) -> RequirementResult {
    let found: Vec<&CapabilityClaim> = catalog
        .iter()
        .filter(|claim| claim.agent == agent && claim.capability == capability)
        .collect();
    if found.is_empty() {
        return RequirementResult {
            capability,
            mandatory: true,
            state: EvidenceState::Unknown,
            freshness: None,
            evidence_summary: None,
            evidence_method: None,
            evidence_source: None,
            evidence_version: None,
            reviewed_at_unix_secs: None,
            limitation: None,
        };
    }
    let has_current_claim = found.iter().any(|claim| {
        matches!(
            claim.evidence.state,
            EvidenceState::Verified
                | EvidenceState::Documented
                | EvidenceState::UserReported
                | EvidenceState::Inferred
        ) && is_fresh(Some(capability::freshness(&claim.evidence, version, now)))
    });
    let has_usable_contradiction = found.iter().any(|claim| {
        claim.evidence.state == EvidenceState::Unsupported
            && is_fresh(Some(capability::freshness(&claim.evidence, version, now)))
    });
    if has_current_claim && has_usable_contradiction {
        return RequirementResult {
            capability,
            mandatory: true,
            state: EvidenceState::Unknown,
            freshness: Some(Freshness::Conflicting),
            evidence_summary: Some("Conflicting current evidence exists."),
            evidence_method: Some("Conflicting catalog evidence"),
            evidence_source: None,
            evidence_version: None,
            reviewed_at_unix_secs: None,
            limitation: Some("Conflicting claims must be reviewed before routing."),
        };
    }
    let claim = found
        .iter()
        .filter(|claim| is_fresh(Some(capability::freshness(&claim.evidence, version, now))))
        .max_by_key(|claim| match claim.evidence.state {
            EvidenceState::Verified => 5,
            EvidenceState::Documented => 4,
            EvidenceState::Unsupported => 3,
            EvidenceState::UserReported => 2,
            EvidenceState::Inferred => 1,
            EvidenceState::Unknown => 0,
        })
        .copied()
        .unwrap_or(found[0]);
    RequirementResult {
        capability,
        mandatory: true,
        state: claim.evidence.state,
        freshness: Some(capability::freshness(&claim.evidence, version, now)),
        evidence_summary: Some(claim.evidence.summary),
        evidence_method: Some(claim.evidence.method),
        evidence_source: claim.evidence.source,
        evidence_version: claim.evidence.version,
        reviewed_at_unix_secs: claim.evidence.reviewed_at_unix_secs,
        limitation: claim.evidence.limitation,
    }
}

fn category_capability(category: TaskCategory) -> CapabilityId {
    match category {
        TaskCategory::SoftwareEngineering => CapabilityId::SoftwareDevelopment,
        TaskCategory::Research => CapabilityId::Research,
        TaskCategory::DataAnalysis => CapabilityId::DataAnalysis,
        TaskCategory::DocumentProcessing => CapabilityId::DocumentProcessing,
        TaskCategory::Writing => CapabilityId::Writing,
        TaskCategory::GeneralReasoning | TaskCategory::Other => CapabilityId::StructuredOutput,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentRecord, Knowledge};
    use crate::capability::CapabilityEvidence;
    use crate::task::TaskId;

    fn record(id: AgentId) -> AgentRecord {
        let mut value = AgentRecord::catalog_entry(id);
        value.detection_status = DetectionStatus::Found;
        value.locations.push(crate::agent::ExecutableLocation {
            discovered_path: "/tmp/a".into(),
            resolved_path: "/tmp/a".into(),
            search_root: "/tmp".into(),
        });
        value
    }
    fn task() -> Task {
        Task::new(
            TaskId(1),
            "Implement a feature".into(),
            TaskCategory::SoftwareEngineering,
            vec![CapabilityId::CodeEditing],
            vec![],
        )
        .unwrap()
    }

    fn category_only_task() -> Task {
        Task::new(
            TaskId(2),
            "Implement a feature".into(),
            TaskCategory::SoftwareEngineering,
            vec![],
            vec![],
        )
        .unwrap()
    }

    #[test]
    fn unknown_mandatory_requirement_is_insufficient_not_passed() {
        let result = recommend(
            &task(),
            &[record(AgentId::CodexCli)],
            capability::CATALOG,
            1_800_000_000,
        );
        assert_eq!(result.candidates[0].outcome, Outcome::InsufficientEvidence);
    }

    #[test]
    fn documented_category_with_unknown_mandatory_is_insufficient() {
        let result = recommend(
            &task(),
            &[record(AgentId::CodexCli)],
            capability::CATALOG,
            1_800_000_000,
        );
        assert_eq!(
            result.candidates[0]
                .category_evidence
                .as_ref()
                .unwrap()
                .state,
            EvidenceState::Documented
        );
        assert!(result.primary.is_empty());
    }

    #[test]
    fn ambiguous_and_ignored_candidates_are_excluded() {
        let mut a = record(AgentId::CodexCli);
        a.detection_status = DetectionStatus::Ambiguous;
        let mut b = record(AgentId::ClaudeCode);
        b.ignored = true;
        let set = recommend(&task(), &[a, b], capability::CATALOG, 1_800_000_000);
        assert!(
            set.candidates
                .iter()
                .all(|item| item.outcome == Outcome::Excluded)
        );
        assert!(set.primary.is_empty());
    }

    #[test]
    fn routing_is_deterministic_and_does_not_mutate_registry_records() {
        let records = vec![record(AgentId::CodexCli), record(AgentId::ClaudeCode)];
        let before = records.clone();
        let first = recommend(
            &category_only_task(),
            &records,
            capability::CATALOG,
            1_800_000_000,
        );
        let second = recommend(
            &category_only_task(),
            &records,
            capability::CATALOG,
            1_800_000_000,
        );
        assert_eq!(first, second);
        assert_eq!(records[0].name, before[0].name);
        assert_eq!(first.primary, vec![AgentId::CodexCli, AgentId::ClaudeCode]);
    }

    #[test]
    fn evidence_states_do_not_treat_user_reports_or_inference_as_usable() {
        let evidence = CapabilityEvidence {
            state: EvidenceState::UserReported,
            method: "User report",
            source: None,
            version: None,
            reviewed_at_unix_secs: Some(1),
            summary: "claim",
            limitation: None,
        };
        assert_eq!(
            capability::freshness(&evidence, None, 1),
            Freshness::VersionUnverified
        );
        assert!(!is_usable(&RequirementResult {
            capability: CapabilityId::Research,
            mandatory: true,
            state: EvidenceState::Inferred,
            freshness: Some(Freshness::Current),
            evidence_summary: None,
            evidence_method: None,
            evidence_source: None,
            evidence_version: None,
            reviewed_at_unix_secs: None,
            limitation: None
        }));
    }

    fn synthetic_claim(
        agent: AgentId,
        capability: CapabilityId,
        state: EvidenceState,
        version: Option<&'static str>,
        date: Option<u64>,
    ) -> CapabilityClaim {
        CapabilityClaim {
            agent,
            capability,
            evidence: CapabilityEvidence {
                state,
                method: "Test evidence fixture",
                source: Some("test evidence"),
                version,
                reviewed_at_unix_secs: date,
                summary: "test claim",
                limitation: None,
            },
        }
    }

    #[test]
    fn verified_version_matched_candidate_is_supported() {
        let mut candidate = record(AgentId::CodexCli);
        candidate.installed_version = Knowledge::Observed {
            value: "1.2.3".into(),
            evidence: crate::agent::Evidence {
                description: "test observation".into(),
                source: None,
                observed_at_unix_secs: Some(1_800_000_000),
            },
        };
        let catalog = [
            synthetic_claim(
                AgentId::CodexCli,
                CapabilityId::CodeEditing,
                EvidenceState::Verified,
                Some("1.2.3"),
                Some(1_800_000_000),
            ),
            synthetic_claim(
                AgentId::CodexCli,
                CapabilityId::SoftwareDevelopment,
                EvidenceState::Verified,
                Some("1.2.3"),
                Some(1_800_000_000),
            ),
        ];
        let result = recommend(&task(), &[candidate], &catalog, 1_800_000_000);
        assert_eq!(result.candidates[0].outcome, Outcome::Supported);
        assert_eq!(result.primary, vec![AgentId::CodexCli]);
    }

    #[test]
    fn unsupported_mandatory_requirement_rejects_candidate() {
        let catalog = [
            synthetic_claim(
                AgentId::CodexCli,
                CapabilityId::CodeEditing,
                EvidenceState::Unsupported,
                None,
                Some(1_800_000_000),
            ),
            synthetic_claim(
                AgentId::CodexCli,
                CapabilityId::SoftwareDevelopment,
                EvidenceState::Documented,
                None,
                Some(1_800_000_000),
            ),
        ];
        let result = recommend(
            &task(),
            &[record(AgentId::CodexCli)],
            &catalog,
            1_800_000_000,
        );
        assert_eq!(result.candidates[0].outcome, Outcome::Rejected);
        assert!(result.primary.is_empty());
    }

    #[test]
    fn conflicting_current_evidence_remains_insufficient() {
        let catalog = [
            synthetic_claim(
                AgentId::CodexCli,
                CapabilityId::CodeEditing,
                EvidenceState::Documented,
                None,
                Some(1_800_000_000),
            ),
            synthetic_claim(
                AgentId::CodexCli,
                CapabilityId::CodeEditing,
                EvidenceState::Unsupported,
                None,
                Some(1_800_000_000),
            ),
            synthetic_claim(
                AgentId::CodexCli,
                CapabilityId::SoftwareDevelopment,
                EvidenceState::Documented,
                None,
                Some(1_800_000_000),
            ),
        ];
        let result = recommend(
            &task(),
            &[record(AgentId::CodexCli)],
            &catalog,
            1_800_000_000,
        );
        assert_eq!(result.candidates[0].outcome, Outcome::InsufficientEvidence);
        assert_eq!(
            result.candidates[0].requirements[0].freshness,
            Some(Freshness::Conflicting)
        );
    }

    #[test]
    fn optional_preference_refines_a_tentative_tie_without_becoming_mandatory() {
        let task = Task::new(
            TaskId(3),
            "Implement a feature".into(),
            TaskCategory::SoftwareEngineering,
            vec![],
            vec![CapabilityId::FileReading],
        )
        .unwrap();
        let catalog = [
            synthetic_claim(
                AgentId::CodexCli,
                CapabilityId::SoftwareDevelopment,
                EvidenceState::Documented,
                None,
                Some(1_800_000_000),
            ),
            synthetic_claim(
                AgentId::CodexCli,
                CapabilityId::FileReading,
                EvidenceState::Documented,
                None,
                Some(1_800_000_000),
            ),
            synthetic_claim(
                AgentId::ClaudeCode,
                CapabilityId::SoftwareDevelopment,
                EvidenceState::Documented,
                None,
                Some(1_800_000_000),
            ),
        ];
        let result = recommend(
            &task,
            &[record(AgentId::CodexCli), record(AgentId::ClaudeCode)],
            &catalog,
            1_800_000_000,
        );
        assert_eq!(
            result
                .candidates
                .iter()
                .find(|item| item.agent == AgentId::CodexCli)
                .unwrap()
                .outcome,
            Outcome::Tentative
        );
        assert_eq!(result.primary, vec![AgentId::CodexCli]);
    }
}
