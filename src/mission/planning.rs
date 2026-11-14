//! Bounded, read-only inputs for mission planning.
//!
//! Phase 5D.1 validates planning inputs and normalizes a caller-supplied
//! inventory. It does not evaluate compatibility, select agents or models,
//! estimate costs, create approvals, execute work, or persist a plan.

use std::collections::{BTreeMap, HashSet};

use serde_json::Value;

use super::{
    MAX_MODEL_BYTES, MAX_OBJECTIVE_BYTES, MAX_TASKS_PER_MISSION, MAX_TITLE_BYTES,
    MAX_WORKSPACE_BYTES, Mission, MissionId, MissionPrivacy, MissionStatus, MissionTask,
    MissionTaskId, TaskStatus, creates_dependency_cycle, deliverable_list, json_object,
    validate_dependency_list, validate_text,
};

pub const MAX_PLAN_AGENTS: usize = 64;
pub const MAX_PLAN_MODELS: usize = 2_000;
pub const MAX_AGENT_MODEL_LINKS: usize = 4_096;

pub const MAX_CAPABILITIES_PER_AGENT: usize = 32;
pub const MAX_EVIDENCE_PER_CAPABILITY: usize = 8;
pub const MAX_COMPATIBILITY_EVIDENCE_PER_LINK: usize = 8;
pub const MAX_LOCALITY_EVIDENCE_PER_CANDIDATE: usize = 2;

pub const MAX_GLOBAL_REQUIREMENTS: usize = 12;
pub const MAX_REQUIREMENTS_TOTAL: usize = 128;
pub const MAX_TASK_REQUIREMENT_GROUPS: usize = 128;
pub const MAX_PREFERENCES_TOTAL: usize = 24;
pub const MAX_PREFERRED_AGENTS: usize = 12;
pub const MAX_PREFERRED_MODELS: usize = 12;
pub const MAX_PREFERRED_CAPABILITIES: usize = 12;

pub const MAX_AGENT_KEY_BYTES: usize = 128;
pub const MAX_MODEL_KEY_BYTES: usize = 160;
pub const MAX_CAPABILITY_ID_BYTES: usize = 64;
pub const MAX_AGENT_LABEL_BYTES: usize = 200;
pub const MAX_PROVIDER_LABEL_BYTES: usize = 200;
pub const MAX_VERSION_BYTES: usize = 128;
pub const MAX_CURRENCY_BYTES: usize = 16;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct AgentKey(pub String);

#[derive(Clone, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct ModelKey(pub String);

#[derive(Clone, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct CapabilityId(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct VersionText(pub String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum EvidenceClaim {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum EvidenceQuality {
    Verified,
    VersionMatched,
    VersionUnverified,
    Stale,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum EvidenceSource {
    VendorDocumentation,
    LocalObservation,
    LiveProbe,
    UserReport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum DataLocality {
    Local,
    Remote,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityEvidence {
    pub claim: EvidenceClaim,
    pub quality: EvidenceQuality,
    pub source: EvidenceSource,
    pub observed_at: Option<u64>,
    pub evidence_version: Option<VersionText>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalityEvidence {
    pub claim: DataLocality,
    pub quality: EvidenceQuality,
    pub source: EvidenceSource,
    pub observed_at: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentCapability {
    pub capability: CapabilityId,
    pub evidence: Vec<CapabilityEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentCandidate {
    pub key: AgentKey,
    pub label: String,
    pub locality: Vec<LocalityEvidence>,
    pub capabilities: Vec<AgentCapability>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct ModelRef {
    pub provider_id: u64,
    pub model: ModelKey,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCandidate {
    pub provider_id: u64,
    pub provider_label: String,
    pub key: ModelKey,
    pub locality: Vec<LocalityEvidence>,
    pub context_window: Option<u32>,
    pub supports_tools: Option<bool>,
    pub input_micros_per_million: Option<u64>,
    pub output_micros_per_million: Option<u64>,
    pub currency: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompatibilityEvidence {
    pub claim: EvidenceClaim,
    pub quality: EvidenceQuality,
    pub source: EvidenceSource,
    pub observed_at: Option<u64>,
    pub evidence_version: Option<VersionText>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentModelLink {
    pub agent: AgentKey,
    pub model: ModelRef,
    pub evidence: Vec<CompatibilityEvidence>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum MinimumEvidence {
    AnyEligibleSupport,
    VersionMatched,
    Verified,
    LocalObservation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityRequirement {
    pub capability: CapabilityId,
    pub minimum_evidence: MinimumEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskRequirements {
    pub task_id: MissionTaskId,
    pub requirements: Vec<CapabilityRequirement>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningPreferences {
    pub preferred_agents: Vec<AgentKey>,
    pub preferred_models: Vec<ModelKey>,
    pub preferred_capabilities: Vec<CapabilityId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningBudget {
    pub currency: String,
    pub max_cost_micros: Option<u64>,
    pub max_total_tokens: Option<u64>,
    pub max_duration_secs: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningPolicy {
    pub budget: Option<PlanningBudget>,
    pub mandatory: Vec<CapabilityRequirement>,
    pub task_requirements: Vec<TaskRequirements>,
    pub preferences: PlanningPreferences,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningInventory {
    pub agents: Vec<AgentCandidate>,
    pub models: Vec<ModelCandidate>,
    pub links: Vec<AgentModelLink>,
}

pub struct PlanningRequest<'a> {
    pub mission: &'a Mission,
    pub tasks: &'a [MissionTask],
    pub policy: PlanningPolicy,
    pub inventory: PlanningInventory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedInventory {
    pub agents: Vec<AgentCandidate>,
    pub models: Vec<ModelCandidate>,
    pub links: Vec<AgentModelLink>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MissionPlan<'a> {
    pub id: MissionId,
    pub title: &'a str,
    pub objective: &'a str,
    pub constraints: &'a Value,
    pub workspace: &'a str,
    pub deliverables: &'a [String],
    pub privacy: MissionPrivacy,
    pub status: MissionStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanTask<'a> {
    pub id: MissionTaskId,
    pub ordinal: u32,
    pub title: &'a str,
    pub objective: &'a str,
    pub status: TaskStatus,
    pub depends_on: &'a [MissionTaskId],
    pub existing_assignment: Option<ExistingAssignment<'a>>,
    /// Task-specific requirements only. Global requirements remain in the
    /// snapshot policy and apply to every task.
    pub requirements: Vec<CapabilityRequirement>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExistingAssignment<'a> {
    pub provider_id: Option<u64>,
    pub model: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanningSnapshot<'a> {
    pub mission: MissionPlan<'a>,
    pub tasks: Vec<PlanTask<'a>>,
    pub policy: PlanningPolicy,
    pub inventory: NormalizedInventory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanningValidationError {
    InvalidMissionId(MissionId),
    InvalidMission(&'static str),
    MissionNotPlannable(MissionStatus),
    InvalidTaskId(MissionTaskId),
    InvalidTask {
        task_id: MissionTaskId,
        reason: &'static str,
    },
    TaskNotPlannable {
        task_id: MissionTaskId,
        status: TaskStatus,
    },
    TaskMissionMismatch {
        task_id: MissionTaskId,
        mission_id: MissionId,
    },
    TooManyTasks,
    DuplicateTaskId(MissionTaskId),
    InvalidOrdinal {
        task_id: MissionTaskId,
        ordinal: u32,
    },
    Dependency {
        task_id: MissionTaskId,
        reason: &'static str,
    },
    TooManyAgents,
    DuplicateAgent(AgentKey),
    TooManyModels,
    DuplicateModel {
        provider_id: u64,
        model: ModelKey,
    },
    TooManyLinks,
    DuplicateLink {
        agent: AgentKey,
        provider_id: u64,
        model: ModelKey,
    },
    LinkEndpointMissing {
        agent: AgentKey,
        provider_id: u64,
        model: ModelKey,
    },
    EmptyLinkEvidence {
        agent: AgentKey,
        provider_id: u64,
        model: ModelKey,
    },
    TooManyCapabilities(AgentKey),
    DuplicateCapability {
        agent: AgentKey,
        capability: CapabilityId,
    },
    TooManyCapabilityEvidence {
        agent: AgentKey,
        capability: CapabilityId,
    },
    TooManyCompatibilityEvidence {
        agent: AgentKey,
        provider_id: u64,
        model: ModelKey,
    },
    TooManyLocalityEvidence,
    TooManyGlobalRequirements,
    TooManyRequirements,
    TooManyTaskRequirementGroups,
    DuplicateTaskRequirements(MissionTaskId),
    UnknownRequirementsTask(MissionTaskId),
    ConflictingRequirement {
        task_id: Option<MissionTaskId>,
        capability: CapabilityId,
    },
    TooManyPreferences,
    TooManyPreferredAgents,
    TooManyPreferredModels,
    TooManyPreferredCapabilities,
    InvalidIdentifier(&'static str),
    InvalidLabel(&'static str),
    InvalidBudget(&'static str),
    InvalidPricing(&'static str),
}

/// Validates and normalizes planning inputs without evaluating or selecting
/// any candidate.
pub fn validate_planning_request<'a>(
    request: PlanningRequest<'a>,
) -> Result<PlanningSnapshot<'a>, PlanningValidationError> {
    let PlanningRequest {
        mission,
        tasks,
        mut policy,
        inventory,
    } = request;

    validate_mission(mission)?;
    if tasks.len() > MAX_TASKS_PER_MISSION as usize {
        return Err(PlanningValidationError::TooManyTasks);
    }

    let mut task_refs: Vec<&MissionTask> = tasks.iter().collect();
    let mut task_ids = HashSet::new();
    let mut ordinals = HashSet::new();
    for task in &task_refs {
        validate_task(task, mission.id)?;
        if !task_ids.insert(task.id) {
            return Err(PlanningValidationError::DuplicateTaskId(task.id));
        }
        if task.ordinal == 0 || !ordinals.insert(task.ordinal) {
            return Err(PlanningValidationError::InvalidOrdinal {
                task_id: task.id,
                ordinal: task.ordinal,
            });
        }
    }

    task_refs.sort_by_key(|task| task.ordinal);
    for (index, task) in task_refs.iter().enumerate() {
        if task.ordinal as usize != index + 1 {
            return Err(PlanningValidationError::InvalidOrdinal {
                task_id: task.id,
                ordinal: task.ordinal,
            });
        }
    }

    let mission_task_ids: Vec<_> = task_refs.iter().map(|task| task.id).collect();
    for task in &task_refs {
        validate_dependency_list(task.id, &task.depends_on, &mission_task_ids).map_err(
            |reason| PlanningValidationError::Dependency {
                task_id: task.id,
                reason,
            },
        )?;
    }
    let edges: Vec<_> = task_refs
        .iter()
        .map(|task| (task.id, task.depends_on.clone()))
        .collect();
    for task in &task_refs {
        if creates_dependency_cycle(task.id, &task.depends_on, &edges) {
            return Err(PlanningValidationError::Dependency {
                task_id: task.id,
                reason: "Task dependencies must not contain a cycle.",
            });
        }
    }

    let task_requirements = normalize_policy(&mut policy, &task_ids)?;
    let inventory = normalize_inventory(inventory)?;

    let plan_tasks = task_refs
        .into_iter()
        .map(|task| PlanTask {
            id: task.id,
            ordinal: task.ordinal,
            title: &task.title,
            objective: &task.objective,
            status: task.status,
            depends_on: &task.depends_on,
            existing_assignment: existing_assignment(task),
            requirements: task_requirements.get(&task.id).cloned().unwrap_or_default(),
        })
        .collect();

    Ok(PlanningSnapshot {
        mission: MissionPlan {
            id: mission.id,
            title: &mission.title,
            objective: &mission.objective,
            constraints: &mission.constraints,
            workspace: &mission.workspace,
            deliverables: &mission.deliverables,
            privacy: mission.privacy,
            status: mission.status,
        },
        tasks: plan_tasks,
        policy,
        inventory,
    })
}

fn validate_mission(mission: &Mission) -> Result<(), PlanningValidationError> {
    if mission.id <= 0 {
        return Err(PlanningValidationError::InvalidMissionId(mission.id));
    }
    if !matches!(
        mission.status,
        MissionStatus::Draft | MissionStatus::Planned
    ) {
        return Err(PlanningValidationError::MissionNotPlannable(mission.status));
    }
    validate_text(mission.title.as_str(), MAX_TITLE_BYTES, "invalid title")
        .and_then(|()| {
            validate_text(
                mission.objective.as_str(),
                MAX_OBJECTIVE_BYTES,
                "invalid objective",
            )
        })
        .and_then(|()| {
            validate_text(
                mission.workspace.as_str(),
                MAX_WORKSPACE_BYTES,
                "invalid workspace",
            )
        })
        .and_then(|()| json_object(&mission.constraints, "invalid constraints").map(|_| ()))
        .and_then(|()| deliverable_list(&mission.deliverables, "invalid deliverables").map(|_| ()))
        .map_err(PlanningValidationError::InvalidMission)
}

fn validate_task(task: &MissionTask, mission_id: MissionId) -> Result<(), PlanningValidationError> {
    if task.id <= 0 {
        return Err(PlanningValidationError::InvalidTaskId(task.id));
    }
    if task.status != TaskStatus::Pending {
        return Err(PlanningValidationError::TaskNotPlannable {
            task_id: task.id,
            status: task.status,
        });
    }
    if task.mission_id != mission_id {
        return Err(PlanningValidationError::TaskMissionMismatch {
            task_id: task.id,
            mission_id: task.mission_id,
        });
    }
    validate_text(task.title.as_str(), MAX_TITLE_BYTES, "invalid title")
        .and_then(|()| {
            validate_text(
                task.objective.as_str(),
                MAX_OBJECTIVE_BYTES,
                "invalid objective",
            )
        })
        .and_then(|()| {
            if let Some(model) = task.model.as_deref()
                && !validate_text_key(model, MAX_MODEL_BYTES)
            {
                return Err("invalid assigned model");
            }
            Ok(())
        })
        .map_err(|reason| PlanningValidationError::InvalidTask {
            task_id: task.id,
            reason,
        })
}

fn existing_assignment(task: &MissionTask) -> Option<ExistingAssignment<'_>> {
    if task.provider_id.is_none() && task.model.is_none() {
        return None;
    }
    Some(ExistingAssignment {
        provider_id: task.provider_id,
        model: task.model.as_deref(),
    })
}

fn normalize_policy(
    policy: &mut PlanningPolicy,
    task_ids: &HashSet<MissionTaskId>,
) -> Result<BTreeMap<MissionTaskId, Vec<CapabilityRequirement>>, PlanningValidationError> {
    if policy.mandatory.len() > MAX_GLOBAL_REQUIREMENTS {
        return Err(PlanningValidationError::TooManyGlobalRequirements);
    }
    if policy.task_requirements.len() > MAX_TASK_REQUIREMENT_GROUPS {
        return Err(PlanningValidationError::TooManyTaskRequirementGroups);
    }

    let submitted_total = policy.mandatory.len().saturating_add(
        policy
            .task_requirements
            .iter()
            .map(|group| group.requirements.len())
            .sum::<usize>(),
    );
    if submitted_total > MAX_REQUIREMENTS_TOTAL {
        return Err(PlanningValidationError::TooManyRequirements);
    }

    policy.mandatory = normalize_requirements(std::mem::take(&mut policy.mandatory), None)?;

    let mut groups = BTreeMap::new();
    for group in std::mem::take(&mut policy.task_requirements) {
        if !task_ids.contains(&group.task_id) {
            return Err(PlanningValidationError::UnknownRequirementsTask(
                group.task_id,
            ));
        }
        if groups.contains_key(&group.task_id) {
            return Err(PlanningValidationError::DuplicateTaskRequirements(
                group.task_id,
            ));
        }
        let requirements = normalize_requirements(group.requirements, Some(group.task_id))?;
        groups.insert(group.task_id, requirements);
    }
    policy.task_requirements = groups
        .iter()
        .map(|(task_id, requirements)| TaskRequirements {
            task_id: *task_id,
            requirements: requirements.clone(),
        })
        .collect();

    normalize_preferences(&mut policy.preferences)?;
    if let Some(budget) = &policy.budget {
        validate_currency(budget.currency.as_str())?;
    }
    Ok(groups)
}

fn normalize_requirements(
    mut requirements: Vec<CapabilityRequirement>,
    task_id: Option<MissionTaskId>,
) -> Result<Vec<CapabilityRequirement>, PlanningValidationError> {
    for requirement in &requirements {
        validate_capability_id(&requirement.capability)?;
    }
    requirements.sort_by(|left, right| left.capability.cmp(&right.capability));

    let mut normalized: Vec<CapabilityRequirement> = Vec::with_capacity(requirements.len());
    for requirement in requirements {
        if let Some(previous) = normalized.last()
            && previous.capability == requirement.capability
        {
            if previous.minimum_evidence != requirement.minimum_evidence {
                return Err(PlanningValidationError::ConflictingRequirement {
                    task_id,
                    capability: requirement.capability,
                });
            }
            continue;
        }
        normalized.push(requirement);
    }
    Ok(normalized)
}

fn normalize_preferences(
    preferences: &mut PlanningPreferences,
) -> Result<(), PlanningValidationError> {
    let total = preferences
        .preferred_agents
        .len()
        .saturating_add(preferences.preferred_models.len())
        .saturating_add(preferences.preferred_capabilities.len());
    if total > MAX_PREFERENCES_TOTAL {
        return Err(PlanningValidationError::TooManyPreferences);
    }
    if preferences.preferred_agents.len() > MAX_PREFERRED_AGENTS {
        return Err(PlanningValidationError::TooManyPreferredAgents);
    }
    if preferences.preferred_models.len() > MAX_PREFERRED_MODELS {
        return Err(PlanningValidationError::TooManyPreferredModels);
    }
    if preferences.preferred_capabilities.len() > MAX_PREFERRED_CAPABILITIES {
        return Err(PlanningValidationError::TooManyPreferredCapabilities);
    }

    for agent in &preferences.preferred_agents {
        validate_key(
            agent.0.as_str(),
            MAX_AGENT_KEY_BYTES,
            "invalid preferred agent",
        )?;
    }
    for model in &preferences.preferred_models {
        validate_key(
            model.0.as_str(),
            MAX_MODEL_KEY_BYTES,
            "invalid preferred model",
        )?;
    }
    for capability in &preferences.preferred_capabilities {
        validate_capability_id(capability)?;
    }

    preferences.preferred_agents.sort();
    preferences.preferred_models.sort();
    preferences.preferred_capabilities.sort();
    Ok(())
}

fn normalize_inventory(
    mut inventory: PlanningInventory,
) -> Result<NormalizedInventory, PlanningValidationError> {
    if inventory.agents.len() > MAX_PLAN_AGENTS {
        return Err(PlanningValidationError::TooManyAgents);
    }
    if inventory.models.len() > MAX_PLAN_MODELS {
        return Err(PlanningValidationError::TooManyModels);
    }
    if inventory.links.len() > MAX_AGENT_MODEL_LINKS {
        return Err(PlanningValidationError::TooManyLinks);
    }

    let mut agent_keys = HashSet::new();
    for agent in &mut inventory.agents {
        validate_key(
            agent.key.0.as_str(),
            MAX_AGENT_KEY_BYTES,
            "invalid agent key",
        )?;
        validate_label(
            agent.label.as_str(),
            MAX_AGENT_LABEL_BYTES,
            "invalid agent label",
        )?;
        if !agent_keys.insert(agent.key.clone()) {
            return Err(PlanningValidationError::DuplicateAgent(agent.key.clone()));
        }
        normalize_locality(&mut agent.locality)?;
        if agent.capabilities.len() > MAX_CAPABILITIES_PER_AGENT {
            return Err(PlanningValidationError::TooManyCapabilities(
                agent.key.clone(),
            ));
        }

        let mut capabilities = HashSet::new();
        for capability in &mut agent.capabilities {
            validate_capability_id(&capability.capability)?;
            if !capabilities.insert(capability.capability.clone()) {
                return Err(PlanningValidationError::DuplicateCapability {
                    agent: agent.key.clone(),
                    capability: capability.capability.clone(),
                });
            }
            if capability.evidence.len() > MAX_EVIDENCE_PER_CAPABILITY {
                return Err(PlanningValidationError::TooManyCapabilityEvidence {
                    agent: agent.key.clone(),
                    capability: capability.capability.clone(),
                });
            }
            for evidence in &capability.evidence {
                validate_evidence_version(evidence.evidence_version.as_ref())?;
            }
            capability.evidence.sort_by(evidence_order);
        }
        agent
            .capabilities
            .sort_by(|left, right| left.capability.cmp(&right.capability));
    }
    inventory
        .agents
        .sort_by(|left, right| left.key.cmp(&right.key));

    let mut model_refs = HashSet::new();
    for model in &mut inventory.models {
        validate_key(
            model.key.0.as_str(),
            MAX_MODEL_KEY_BYTES,
            "invalid model key",
        )?;
        validate_label(
            model.provider_label.as_str(),
            MAX_PROVIDER_LABEL_BYTES,
            "invalid provider label",
        )?;
        normalize_locality(&mut model.locality)?;
        validate_model_pricing(model)?;
        let reference = ModelRef {
            provider_id: model.provider_id,
            model: model.key.clone(),
        };
        if !model_refs.insert(reference.clone()) {
            return Err(PlanningValidationError::DuplicateModel {
                provider_id: reference.provider_id,
                model: reference.model,
            });
        }
    }
    inventory
        .models
        .sort_by(|left, right| (left.provider_id, &left.key).cmp(&(right.provider_id, &right.key)));

    let mut link_refs = HashSet::new();
    for link in &mut inventory.links {
        let reference = ModelRef {
            provider_id: link.model.provider_id,
            model: link.model.model.clone(),
        };
        if !model_refs.contains(&reference) || !agent_keys.contains(&link.agent) {
            return Err(PlanningValidationError::LinkEndpointMissing {
                agent: link.agent.clone(),
                provider_id: reference.provider_id,
                model: reference.model,
            });
        }
        let key = (link.agent.clone(), reference.clone());
        if !link_refs.insert(key) {
            return Err(PlanningValidationError::DuplicateLink {
                agent: link.agent.clone(),
                provider_id: reference.provider_id,
                model: reference.model,
            });
        }
        if link.evidence.is_empty() {
            return Err(PlanningValidationError::EmptyLinkEvidence {
                agent: link.agent.clone(),
                provider_id: reference.provider_id,
                model: reference.model,
            });
        }
        if link.evidence.len() > MAX_COMPATIBILITY_EVIDENCE_PER_LINK {
            return Err(PlanningValidationError::TooManyCompatibilityEvidence {
                agent: link.agent.clone(),
                provider_id: reference.provider_id,
                model: reference.model,
            });
        }
        for evidence in &link.evidence {
            validate_evidence_version(evidence.evidence_version.as_ref())?;
        }
        link.evidence.sort_by(compatibility_evidence_order);
    }
    inventory.links.sort_by(|left, right| {
        (&left.agent, left.model.provider_id, &left.model.model).cmp(&(
            &right.agent,
            right.model.provider_id,
            &right.model.model,
        ))
    });

    Ok(NormalizedInventory {
        agents: inventory.agents,
        models: inventory.models,
        links: inventory.links,
    })
}

fn validate_model_pricing(model: &ModelCandidate) -> Result<(), PlanningValidationError> {
    if let Some(currency) = model.currency.as_deref() {
        validate_currency(currency).map_err(|_| {
            PlanningValidationError::InvalidPricing(
                "Model pricing currency must be non-empty and within the byte limit.",
            )
        })?;
    }
    if (model.input_micros_per_million.is_some() || model.output_micros_per_million.is_some())
        && model.currency.is_none()
    {
        return Err(PlanningValidationError::InvalidPricing(
            "Model pricing requires a currency.",
        ));
    }
    Ok(())
}

fn normalize_locality(locality: &mut [LocalityEvidence]) -> Result<(), PlanningValidationError> {
    if locality.len() > MAX_LOCALITY_EVIDENCE_PER_CANDIDATE {
        return Err(PlanningValidationError::TooManyLocalityEvidence);
    }
    locality.sort_by(|left, right| {
        (left.claim, left.quality, left.source, left.observed_at).cmp(&(
            right.claim,
            right.quality,
            right.source,
            right.observed_at,
        ))
    });
    Ok(())
}

fn evidence_order(left: &CapabilityEvidence, right: &CapabilityEvidence) -> std::cmp::Ordering {
    (
        left.claim,
        left.quality,
        left.source,
        left.observed_at,
        left.evidence_version.as_ref(),
    )
        .cmp(&(
            right.claim,
            right.quality,
            right.source,
            right.observed_at,
            right.evidence_version.as_ref(),
        ))
}

fn compatibility_evidence_order(
    left: &CompatibilityEvidence,
    right: &CompatibilityEvidence,
) -> std::cmp::Ordering {
    (
        left.claim,
        left.quality,
        left.source,
        left.observed_at,
        left.evidence_version.as_ref(),
    )
        .cmp(&(
            right.claim,
            right.quality,
            right.source,
            right.observed_at,
            right.evidence_version.as_ref(),
        ))
}

fn validate_text_key(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_bytes && !value.contains('\0')
}

fn validate_key(
    value: &str,
    max_bytes: usize,
    message: &'static str,
) -> Result<(), PlanningValidationError> {
    if validate_text_key(value, max_bytes) {
        Ok(())
    } else {
        Err(PlanningValidationError::InvalidIdentifier(message))
    }
}

fn validate_label(
    value: &str,
    max_bytes: usize,
    message: &'static str,
) -> Result<(), PlanningValidationError> {
    if validate_text_key(value, max_bytes) {
        Ok(())
    } else {
        Err(PlanningValidationError::InvalidLabel(message))
    }
}

fn validate_capability_id(capability: &CapabilityId) -> Result<(), PlanningValidationError> {
    validate_key(
        capability.0.as_str(),
        MAX_CAPABILITY_ID_BYTES,
        "invalid capability identifier",
    )
}

fn validate_evidence_version(version: Option<&VersionText>) -> Result<(), PlanningValidationError> {
    if let Some(version) = version {
        validate_key(
            version.0.as_str(),
            MAX_VERSION_BYTES,
            "invalid evidence version",
        )?;
    }
    Ok(())
}

fn validate_currency(currency: &str) -> Result<(), PlanningValidationError> {
    if validate_text_key(currency, MAX_CURRENCY_BYTES) {
        Ok(())
    } else {
        Err(PlanningValidationError::InvalidBudget(
            "Budget currency must be non-empty and within the byte limit.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission::{MAX_DEPENDENCIES, MAX_JSON_BYTES};

    fn sample_mission() -> Mission {
        Mission {
            id: 1,
            title: "Mission".to_owned(),
            objective: "Produce a bounded plan".to_owned(),
            constraints: serde_json::json!({ "tone": "concise" }),
            workspace: "/tmp/nexus-planning".to_owned(),
            deliverables: vec!["A plan".to_owned()],
            privacy: MissionPrivacy::LocalOnly,
            status: MissionStatus::Draft,
            created_at: 1,
            updated_at: 1,
            started_at: None,
            completed_at: None,
        }
    }

    fn task(id: MissionTaskId, ordinal: u32) -> MissionTask {
        MissionTask {
            id,
            mission_id: 1,
            ordinal,
            title: format!("Task {id}"),
            objective: format!("Do task {id}"),
            status: TaskStatus::Pending,
            provider_id: None,
            model: None,
            depends_on: Vec::new(),
            created_at: 1,
            updated_at: 1,
            started_at: None,
            completed_at: None,
        }
    }

    fn sample_policy() -> PlanningPolicy {
        PlanningPolicy {
            budget: None,
            mandatory: Vec::new(),
            task_requirements: Vec::new(),
            preferences: PlanningPreferences {
                preferred_agents: Vec::new(),
                preferred_models: Vec::new(),
                preferred_capabilities: Vec::new(),
            },
        }
    }

    fn inventory() -> PlanningInventory {
        PlanningInventory {
            agents: Vec::new(),
            models: Vec::new(),
            links: Vec::new(),
        }
    }

    fn validate<'a>(
        mission: &'a Mission,
        tasks: &'a [MissionTask],
        policy: PlanningPolicy,
        inventory: PlanningInventory,
    ) -> Result<PlanningSnapshot<'a>, PlanningValidationError> {
        validate_planning_request(PlanningRequest {
            mission,
            tasks,
            policy,
            inventory,
        })
    }

    fn capability(name: &str, claim: EvidenceClaim, quality: EvidenceQuality) -> AgentCapability {
        AgentCapability {
            capability: CapabilityId(name.to_owned()),
            evidence: vec![CapabilityEvidence {
                claim,
                quality,
                source: EvidenceSource::VendorDocumentation,
                observed_at: Some(7),
                evidence_version: Some(VersionText("1.0".to_owned())),
            }],
        }
    }

    fn agent(key: &str) -> AgentCandidate {
        AgentCandidate {
            key: AgentKey(key.to_owned()),
            label: format!("Agent {key}"),
            locality: vec![LocalityEvidence {
                claim: DataLocality::Local,
                quality: EvidenceQuality::Verified,
                source: EvidenceSource::LocalObservation,
                observed_at: Some(1),
            }],
            capabilities: vec![capability(
                "coding",
                EvidenceClaim::Supported,
                EvidenceQuality::Verified,
            )],
        }
    }

    fn model(provider_id: u64, key: &str) -> ModelCandidate {
        ModelCandidate {
            provider_id,
            provider_label: format!("Provider {provider_id}"),
            key: ModelKey(key.to_owned()),
            locality: vec![LocalityEvidence {
                claim: DataLocality::Remote,
                quality: EvidenceQuality::VersionMatched,
                source: EvidenceSource::VendorDocumentation,
                observed_at: Some(1),
            }],
            context_window: Some(8_192),
            supports_tools: Some(true),
            input_micros_per_million: None,
            output_micros_per_million: None,
            currency: None,
        }
    }

    fn priced_model(
        input_micros_per_million: Option<u64>,
        output_micros_per_million: Option<u64>,
        currency: Option<&str>,
    ) -> ModelCandidate {
        let mut candidate = model(1, "m");
        candidate.input_micros_per_million = input_micros_per_million;
        candidate.output_micros_per_million = output_micros_per_million;
        candidate.currency = currency.map(str::to_owned);
        candidate
    }

    fn link(agent_key: &str, provider_id: u64, model_key: &str) -> AgentModelLink {
        AgentModelLink {
            agent: AgentKey(agent_key.to_owned()),
            model: ModelRef {
                provider_id,
                model: ModelKey(model_key.to_owned()),
            },
            evidence: vec![CompatibilityEvidence {
                claim: EvidenceClaim::Supported,
                quality: EvidenceQuality::VersionMatched,
                source: EvidenceSource::VendorDocumentation,
                observed_at: Some(1),
                evidence_version: Some(VersionText("1.0".to_owned())),
            }],
        }
    }

    #[test]
    fn valid_snapshot_preserves_inputs_and_orders_tasks() {
        let mission = sample_mission();
        let mut first = task(10, 1);
        first.provider_id = Some(7);
        first.model = Some("model-a".to_owned());
        let mut second = task(11, 2);
        second.depends_on = vec![10];
        let tasks = vec![second, first];

        let mut policy = sample_policy();
        policy.mandatory.push(CapabilityRequirement {
            capability: CapabilityId("review".to_owned()),
            minimum_evidence: MinimumEvidence::Verified,
        });
        policy.task_requirements.push(TaskRequirements {
            task_id: 10,
            requirements: vec![CapabilityRequirement {
                capability: CapabilityId("coding".to_owned()),
                minimum_evidence: MinimumEvidence::AnyEligibleSupport,
            }],
        });

        let snapshot = validate(&mission, &tasks, policy, inventory()).unwrap();
        assert_eq!(snapshot.mission.id, 1);
        assert_eq!(snapshot.mission.privacy, MissionPrivacy::LocalOnly);
        assert_eq!(snapshot.tasks[0].id, 10);
        assert_eq!(snapshot.tasks[1].id, 11);
        assert_eq!(
            snapshot.tasks[0]
                .existing_assignment
                .as_ref()
                .unwrap()
                .provider_id,
            Some(7)
        );
        assert_eq!(snapshot.policy.mandatory.len(), 1);
        assert_eq!(snapshot.tasks[0].requirements[0].capability.0, "coding");
        assert!(snapshot.tasks[1].requirements.is_empty());
    }

    #[test]
    fn non_plannable_statuses_are_rejected() {
        let mut mission = sample_mission();
        mission.status = MissionStatus::Running;
        assert_eq!(
            validate(&mission, &[], sample_policy(), inventory()),
            Err(PlanningValidationError::MissionNotPlannable(
                MissionStatus::Running
            ))
        );

        let mission = sample_mission();
        let mut task = task(10, 1);
        task.status = TaskStatus::Ready;
        assert_eq!(
            validate(&mission, &[task], sample_policy(), inventory()),
            Err(PlanningValidationError::TaskNotPlannable {
                task_id: 10,
                status: TaskStatus::Ready,
            })
        );
    }

    #[test]
    fn duplicate_ids_mismatched_missions_and_invalid_ordinals_are_rejected() {
        let mission = sample_mission();
        assert_eq!(
            validate(
                &mission,
                &[task(10, 1), task(10, 2)],
                sample_policy(),
                inventory()
            ),
            Err(PlanningValidationError::DuplicateTaskId(10))
        );

        let mut wrong = task(10, 1);
        wrong.mission_id = 2;
        assert_eq!(
            validate(&mission, &[wrong], sample_policy(), inventory()),
            Err(PlanningValidationError::TaskMissionMismatch {
                task_id: 10,
                mission_id: 2,
            })
        );

        assert_eq!(
            validate(
                &mission,
                &[task(10, 1), task(11, 3)],
                sample_policy(),
                inventory()
            ),
            Err(PlanningValidationError::InvalidOrdinal {
                task_id: 11,
                ordinal: 3,
            })
        );
    }

    #[test]
    fn dependency_errors_are_reported_without_partial_snapshots() {
        let mission = sample_mission();
        let mut missing = task(10, 1);
        missing.depends_on = vec![99];
        assert!(matches!(
            validate(&mission, &[missing], sample_policy(), inventory()),
            Err(PlanningValidationError::Dependency { task_id: 10, .. })
        ));

        let mut cyclic = task(10, 1);
        cyclic.depends_on = vec![20];
        let mut other = task(20, 2);
        other.depends_on = vec![10];
        assert!(matches!(
            validate(&mission, &[cyclic, other], sample_policy(), inventory()),
            Err(PlanningValidationError::Dependency { task_id: 10, .. })
        ));

        let mut too_many = task(10, 1);
        too_many.depends_on = (20..=20 + MAX_DEPENDENCIES as i64).collect();
        assert!(matches!(
            validate(&mission, &[too_many], sample_policy(), inventory()),
            Err(PlanningValidationError::Dependency { task_id: 10, .. })
        ));
    }

    #[test]
    fn dependency_edge_cases_and_bounds_are_enforced_at_entry_point() {
        let mission = sample_mission();

        let mut self_reference = task(10, 1);
        self_reference.depends_on = vec![10];
        assert!(matches!(
            validate(&mission, &[self_reference], sample_policy(), inventory()),
            Err(PlanningValidationError::Dependency { task_id: 10, .. })
        ));

        let mut duplicate = task(10, 1);
        duplicate.depends_on = vec![11, 11];
        assert!(matches!(
            validate(
                &mission,
                &[duplicate, task(11, 2)],
                sample_policy(),
                inventory()
            ),
            Err(PlanningValidationError::Dependency { task_id: 10, .. })
        ));

        let mut exact_tasks: Vec<_> = (1..=101).map(|id| task(id, id as u32)).collect();
        exact_tasks[0].depends_on = (2..=101).collect();
        assert!(validate(&mission, &exact_tasks, sample_policy(), inventory()).is_ok());

        let mut over_limit: Vec<_> = (1..=102).map(|id| task(id, id as u32)).collect();
        over_limit[0].depends_on = (2..=102).collect();
        assert!(matches!(
            validate(&mission, &over_limit, sample_policy(), inventory()),
            Err(PlanningValidationError::Dependency { task_id: 1, .. })
        ));
    }

    #[test]
    fn mission_and_task_ids_must_be_positive() {
        let mut zero_mission = sample_mission();
        zero_mission.id = 0;
        assert_eq!(
            validate(&zero_mission, &[], sample_policy(), inventory()),
            Err(PlanningValidationError::InvalidMissionId(0))
        );

        let mut negative_mission = sample_mission();
        negative_mission.id = -4;
        assert_eq!(
            validate(&negative_mission, &[], sample_policy(), inventory()),
            Err(PlanningValidationError::InvalidMissionId(-4))
        );

        let mission = sample_mission();
        assert_eq!(
            validate(&mission, &[task(0, 1)], sample_policy(), inventory()),
            Err(PlanningValidationError::InvalidTaskId(0))
        );
        assert_eq!(
            validate(&mission, &[task(-2, 1)], sample_policy(), inventory()),
            Err(PlanningValidationError::InvalidTaskId(-2))
        );
        assert!(validate(&mission, &[task(2, 1)], sample_policy(), inventory()).is_ok());
    }

    #[test]
    fn oversized_nested_constraints_are_rejected_without_changing_preservation() {
        let mut mission = sample_mission();
        mission.constraints = serde_json::json!({
            "nested": { "blob": "x".repeat(MAX_JSON_BYTES) }
        });
        assert!(matches!(
            validate(&mission, &[], sample_policy(), inventory()),
            Err(PlanningValidationError::InvalidMission(_))
        ));

        let mut valid = sample_mission();
        valid.constraints = serde_json::json!({
            "nested": { "tone": "concise", "limits": [1, 2, 3] }
        });
        let snapshot = validate(&valid, &[], sample_policy(), inventory()).unwrap();
        assert_eq!(snapshot.mission.constraints, &valid.constraints);
    }

    #[test]
    fn requirements_deduplicate_exact_entries_and_reject_conflicts() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];

        let mut policy = sample_policy();
        policy.mandatory.push(CapabilityRequirement {
            capability: CapabilityId("coding".to_owned()),
            minimum_evidence: MinimumEvidence::Verified,
        });
        policy.mandatory.push(CapabilityRequirement {
            capability: CapabilityId("coding".to_owned()),
            minimum_evidence: MinimumEvidence::Verified,
        });
        let snapshot = validate(&mission, &tasks, policy, inventory()).unwrap();
        assert_eq!(snapshot.policy.mandatory.len(), 1);

        let mut conflicting = sample_policy();
        conflicting.mandatory.push(CapabilityRequirement {
            capability: CapabilityId("coding".to_owned()),
            minimum_evidence: MinimumEvidence::Verified,
        });
        conflicting.mandatory.push(CapabilityRequirement {
            capability: CapabilityId("coding".to_owned()),
            minimum_evidence: MinimumEvidence::AnyEligibleSupport,
        });
        assert_eq!(
            validate(&mission, &tasks, conflicting, inventory()),
            Err(PlanningValidationError::ConflictingRequirement {
                task_id: None,
                capability: CapabilityId("coding".to_owned()),
            })
        );
    }

    #[test]
    fn task_requirement_groups_must_be_unique_and_reference_real_tasks() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];
        let requirement = CapabilityRequirement {
            capability: CapabilityId("coding".to_owned()),
            minimum_evidence: MinimumEvidence::Verified,
        };

        let mut duplicate = sample_policy();
        duplicate.task_requirements = vec![
            TaskRequirements {
                task_id: 10,
                requirements: vec![requirement.clone()],
            },
            TaskRequirements {
                task_id: 10,
                requirements: vec![requirement.clone()],
            },
        ];
        assert_eq!(
            validate(&mission, &tasks, duplicate, inventory()),
            Err(PlanningValidationError::DuplicateTaskRequirements(10))
        );

        let mut unknown = sample_policy();
        unknown.task_requirements.push(TaskRequirements {
            task_id: 99,
            requirements: vec![requirement],
        });
        assert_eq!(
            validate(&mission, &tasks, unknown, inventory()),
            Err(PlanningValidationError::UnknownRequirementsTask(99))
        );
    }

    #[test]
    fn requirement_bounds_apply_before_deduplication() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];
        let requirement = CapabilityRequirement {
            capability: CapabilityId("coding".to_owned()),
            minimum_evidence: MinimumEvidence::Verified,
        };
        let mut policy = sample_policy();
        policy.task_requirements.push(TaskRequirements {
            task_id: 10,
            requirements: vec![requirement.clone(); MAX_REQUIREMENTS_TOTAL],
        });
        let snapshot = validate(&mission, &tasks, policy, inventory()).unwrap();
        assert_eq!(snapshot.tasks[0].requirements.len(), 1);

        let mut policy = sample_policy();
        policy.task_requirements.push(TaskRequirements {
            task_id: 10,
            requirements: vec![requirement; MAX_REQUIREMENTS_TOTAL + 1],
        });
        assert_eq!(
            validate(&mission, &tasks, policy, inventory()),
            Err(PlanningValidationError::TooManyRequirements)
        );
    }

    #[test]
    fn exact_nested_limits_are_accepted_and_one_more_is_rejected() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];

        let mut exact_agents = inventory();
        exact_agents.agents = (0..MAX_PLAN_AGENTS)
            .map(|index| agent(&format!("agent-{index}")))
            .collect();
        assert!(validate(&mission, &tasks, sample_policy(), exact_agents).is_ok());

        let mut exact_models = inventory();
        exact_models.models = (0..MAX_PLAN_MODELS)
            .map(|index| model(1, &format!("model-{index}")))
            .collect();
        assert!(validate(&mission, &tasks, sample_policy(), exact_models).is_ok());

        let mut bounded_candidate = agent("bounded");
        bounded_candidate.locality = vec![bounded_candidate.locality[0].clone(); 2];
        bounded_candidate.capabilities = (0..MAX_CAPABILITIES_PER_AGENT)
            .map(|index| {
                capability(
                    &format!("cap-{index}"),
                    EvidenceClaim::Supported,
                    EvidenceQuality::Verified,
                )
            })
            .collect();
        bounded_candidate.capabilities[0].evidence = (0..MAX_EVIDENCE_PER_CAPABILITY)
            .map(|_| CapabilityEvidence {
                claim: EvidenceClaim::Supported,
                quality: EvidenceQuality::Verified,
                source: EvidenceSource::LocalObservation,
                observed_at: Some(1),
                evidence_version: Some(VersionText("1.0".to_owned())),
            })
            .collect();
        let mut nested = inventory();
        nested.agents = vec![bounded_candidate];
        let snapshot = validate(&mission, &tasks, sample_policy(), nested).unwrap();
        assert_eq!(snapshot.inventory.agents[0].locality.len(), 2);
        assert_eq!(
            snapshot.inventory.agents[0].capabilities.len(),
            MAX_CAPABILITIES_PER_AGENT
        );
        assert_eq!(
            snapshot.inventory.agents[0].capabilities[0].evidence.len(),
            MAX_EVIDENCE_PER_CAPABILITY
        );

        let mut exact_links = inventory();
        exact_links.agents = (0..64)
            .map(|index| agent(&format!("agent-{index}")))
            .collect();
        exact_links.models = (0..64)
            .map(|index| model(1, &format!("model-{index}")))
            .collect();
        for agent_index in 0..64 {
            for model_index in 0..64 {
                exact_links.links.push(link(
                    &format!("agent-{agent_index}"),
                    1,
                    &format!("model-{model_index}"),
                ));
            }
        }
        let snapshot = validate(&mission, &tasks, sample_policy(), exact_links).unwrap();
        assert_eq!(snapshot.inventory.links.len(), MAX_AGENT_MODEL_LINKS);
    }

    #[test]
    fn task_count_link_evidence_and_model_locality_boundaries_are_enforced() {
        let mission = sample_mission();
        let exact_tasks: Vec<_> = (1..=MAX_TASKS_PER_MISSION)
            .map(|ordinal| task(i64::from(ordinal), ordinal))
            .collect();
        let snapshot = validate(&mission, &exact_tasks, sample_policy(), inventory()).unwrap();
        assert_eq!(snapshot.tasks.len(), MAX_TASKS_PER_MISSION as usize);

        let over_tasks: Vec<_> = (1..=MAX_TASKS_PER_MISSION + 1)
            .map(|ordinal| task(i64::from(ordinal), ordinal))
            .collect();
        assert_eq!(
            validate(&mission, &over_tasks, sample_policy(), inventory()),
            Err(PlanningValidationError::TooManyTasks)
        );

        let mut exact_link = inventory();
        exact_link.agents = vec![agent("a")];
        exact_link.models = vec![model(1, "m")];
        let mut bounded_link = link("a", 1, "m");
        bounded_link.evidence = (0..MAX_COMPATIBILITY_EVIDENCE_PER_LINK)
            .map(|observed_at| CompatibilityEvidence {
                claim: EvidenceClaim::Supported,
                quality: EvidenceQuality::Verified,
                source: EvidenceSource::VendorDocumentation,
                observed_at: Some(observed_at as u64),
                evidence_version: Some(VersionText("1.0".to_owned())),
            })
            .collect();
        exact_link.links = vec![bounded_link];
        let snapshot = validate(&mission, &[], sample_policy(), exact_link).unwrap();
        assert_eq!(
            snapshot.inventory.links[0].evidence.len(),
            MAX_COMPATIBILITY_EVIDENCE_PER_LINK
        );

        let mut too_much_link_evidence = inventory();
        too_much_link_evidence.agents = vec![agent("a")];
        too_much_link_evidence.models = vec![model(1, "m")];
        let mut oversized_link = link("a", 1, "m");
        oversized_link.evidence = (0..=MAX_COMPATIBILITY_EVIDENCE_PER_LINK)
            .map(|observed_at| CompatibilityEvidence {
                claim: EvidenceClaim::Supported,
                quality: EvidenceQuality::Verified,
                source: EvidenceSource::VendorDocumentation,
                observed_at: Some(observed_at as u64),
                evidence_version: Some(VersionText("1.0".to_owned())),
            })
            .collect();
        too_much_link_evidence.links = vec![oversized_link];
        assert!(matches!(
            validate(&mission, &[], sample_policy(), too_much_link_evidence),
            Err(PlanningValidationError::TooManyCompatibilityEvidence { .. })
        ));

        let locality = LocalityEvidence {
            claim: DataLocality::Local,
            quality: EvidenceQuality::Verified,
            source: EvidenceSource::LocalObservation,
            observed_at: Some(1),
        };
        let mut exact_model_locality = inventory();
        let mut candidate = model(1, "m");
        candidate.locality = vec![locality.clone(); MAX_LOCALITY_EVIDENCE_PER_CANDIDATE];
        exact_model_locality.models = vec![candidate];
        let snapshot = validate(&mission, &[], sample_policy(), exact_model_locality).unwrap();
        assert_eq!(
            snapshot.inventory.models[0].locality.len(),
            MAX_LOCALITY_EVIDENCE_PER_CANDIDATE
        );

        let mut too_much_model_locality = inventory();
        let mut candidate = model(1, "m");
        candidate.locality = vec![locality; MAX_LOCALITY_EVIDENCE_PER_CANDIDATE + 1];
        too_much_model_locality.models = vec![candidate];
        assert_eq!(
            validate(&mission, &[], sample_policy(), too_much_model_locality),
            Err(PlanningValidationError::TooManyLocalityEvidence)
        );
    }

    #[test]
    fn exact_requirement_preference_and_group_limits_are_accepted() {
        let mission = sample_mission();
        let tasks: Vec<_> = (1..=MAX_TASK_REQUIREMENT_GROUPS)
            .map(|id| task(id as i64, id as u32))
            .collect();

        let mut global = sample_policy();
        global.mandatory = (0..MAX_GLOBAL_REQUIREMENTS)
            .map(|index| CapabilityRequirement {
                capability: CapabilityId(format!("cap-{index}")),
                minimum_evidence: MinimumEvidence::Verified,
            })
            .collect();
        let snapshot = validate(&mission, &tasks, global, inventory()).unwrap();
        assert_eq!(snapshot.policy.mandatory.len(), MAX_GLOBAL_REQUIREMENTS);

        let mut preferences = sample_policy();
        preferences.preferences.preferred_agents = (0..MAX_PREFERRED_AGENTS)
            .map(|index| AgentKey(format!("agent-{index}")))
            .collect();
        preferences.preferences.preferred_models = (0..MAX_PREFERRED_MODELS)
            .map(|index| ModelKey(format!("model-{index}")))
            .collect();
        assert!(validate(&mission, &tasks, preferences, inventory()).is_ok());

        let mut capabilities = sample_policy();
        capabilities.preferences.preferred_capabilities = (0..MAX_PREFERRED_CAPABILITIES)
            .map(|index| CapabilityId(format!("cap-{index}")))
            .collect();
        assert!(validate(&mission, &tasks, capabilities, inventory()).is_ok());

        let mut too_many_models = sample_policy();
        too_many_models.preferences.preferred_models = (0..=MAX_PREFERRED_MODELS)
            .map(|index| ModelKey(format!("model-{index}")))
            .collect();
        assert_eq!(
            validate(&mission, &tasks, too_many_models, inventory()),
            Err(PlanningValidationError::TooManyPreferredModels)
        );

        let mut too_many_capabilities = sample_policy();
        too_many_capabilities.preferences.preferred_capabilities = (0..=MAX_PREFERRED_CAPABILITIES)
            .map(|index| CapabilityId(format!("cap-{index}")))
            .collect();
        assert_eq!(
            validate(&mission, &tasks, too_many_capabilities, inventory()),
            Err(PlanningValidationError::TooManyPreferredCapabilities)
        );

        let mut groups = sample_policy();
        groups.task_requirements = (1..=MAX_TASK_REQUIREMENT_GROUPS)
            .map(|id| TaskRequirements {
                task_id: id as i64,
                requirements: Vec::new(),
            })
            .collect();
        let snapshot = validate(&mission, &tasks, groups, inventory()).unwrap();
        assert_eq!(
            snapshot.policy.task_requirements.len(),
            MAX_TASK_REQUIREMENT_GROUPS
        );
    }

    #[test]
    fn planning_module_has_no_clock_io_network_or_storage_access() {
        let source = include_str!("planning.rs");
        let clock = concat!("std::", "time");
        let system_time = concat!("System", "Time");
        let instant_now = concat!("Instant", "::now");
        let chrono = concat!("chrono", "::");
        let filesystem = concat!("std::", "fs");
        let file = concat!("File", "::");
        let open_options = concat!("Open", "Options");
        let network = concat!("std::", "net");
        let tcp = concat!("Tcp", "Stream");
        let udp = concat!("Udp", "Socket");
        let http_client = concat!("http", "::");
        let process = concat!("std::", "process");
        let command = concat!("Command", "::new");
        let environment = concat!("std::", "env");
        let worker = concat!("std::", "thread");
        let tokio = concat!("tokio", "::");
        let spawn = concat!("spawn", "(");
        let http = concat!("req", "west::");
        let database = concat!("rusq", "lite");
        let connection = concat!("Connection", "::");
        let storage = concat!("Storage", "::");
        let credentials = concat!("key", "ring");
        let sec_framework = concat!("security_", "framework");
        let credential = concat!("Credential", "::");
        for forbidden in [
            clock,
            system_time,
            instant_now,
            chrono,
            filesystem,
            file,
            open_options,
            network,
            tcp,
            udp,
            http_client,
            process,
            command,
            environment,
            worker,
            tokio,
            spawn,
            http,
            database,
            connection,
            storage,
            credentials,
            sec_framework,
            credential,
        ] {
            assert!(
                !source.contains(forbidden),
                "found forbidden access: {forbidden}"
            );
        }
    }

    #[test]
    fn global_requirement_and_preference_limits_are_enforced() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];
        let mut policy = sample_policy();
        policy.mandatory = (0..=MAX_GLOBAL_REQUIREMENTS)
            .map(|index| CapabilityRequirement {
                capability: CapabilityId(format!("cap-{index}")),
                minimum_evidence: MinimumEvidence::Verified,
            })
            .collect();
        assert_eq!(
            validate(&mission, &tasks, policy, inventory()),
            Err(PlanningValidationError::TooManyGlobalRequirements)
        );

        let mut policy = sample_policy();
        policy.preferences.preferred_agents = (0..=MAX_PREFERRED_AGENTS)
            .map(|index| AgentKey(format!("agent-{index}")))
            .collect();
        assert_eq!(
            validate(&mission, &tasks, policy, inventory()),
            Err(PlanningValidationError::TooManyPreferredAgents)
        );

        let mut policy = sample_policy();
        policy.preferences.preferred_agents = (0..MAX_PREFERRED_AGENTS)
            .map(|index| AgentKey(format!("agent-{index}")))
            .collect();
        policy.preferences.preferred_models = (0..MAX_PREFERRED_MODELS)
            .map(|index| ModelKey(format!("model-{index}")))
            .collect();
        policy.preferences.preferred_capabilities = (0..=MAX_PREFERRED_CAPABILITIES)
            .map(|index| CapabilityId(format!("cap-{index}")))
            .collect();
        assert_eq!(
            validate(&mission, &tasks, policy, inventory()),
            Err(PlanningValidationError::TooManyPreferences)
        );
    }

    #[test]
    fn task_requirement_group_limit_is_enforced() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];
        let mut policy = sample_policy();
        policy.task_requirements = (0..=MAX_TASK_REQUIREMENT_GROUPS)
            .map(|_| TaskRequirements {
                task_id: 10,
                requirements: Vec::new(),
            })
            .collect();
        assert_eq!(
            validate(&mission, &tasks, policy, inventory()),
            Err(PlanningValidationError::TooManyTaskRequirementGroups)
        );
    }

    #[test]
    fn candidate_collection_bounds_are_enforced() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];

        let mut too_many_agents = inventory();
        too_many_agents.agents = (0..=MAX_PLAN_AGENTS)
            .map(|index| agent(&format!("agent-{index}")))
            .collect();
        assert_eq!(
            validate(&mission, &tasks, sample_policy(), too_many_agents),
            Err(PlanningValidationError::TooManyAgents)
        );

        let mut too_many_models = inventory();
        too_many_models.models = (0..=MAX_PLAN_MODELS)
            .map(|index| model(1, &format!("model-{index}")))
            .collect();
        assert_eq!(
            validate(&mission, &tasks, sample_policy(), too_many_models),
            Err(PlanningValidationError::TooManyModels)
        );

        let mut too_many_links = inventory();
        too_many_links.links = (0..=MAX_AGENT_MODEL_LINKS)
            .map(|index| link("missing", 1, &format!("model-{index}")))
            .collect();
        assert_eq!(
            validate(&mission, &tasks, sample_policy(), too_many_links),
            Err(PlanningValidationError::TooManyLinks)
        );
    }

    #[test]
    fn duplicate_candidates_links_and_bad_endpoints_are_rejected() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];

        let mut duplicate_agent = inventory();
        duplicate_agent.agents = vec![agent("a"), agent("a")];
        assert!(matches!(
            validate(&mission, &tasks, sample_policy(), duplicate_agent),
            Err(PlanningValidationError::DuplicateAgent(_))
        ));

        let mut duplicate_model = inventory();
        duplicate_model.models = vec![model(1, "m"), model(1, "m")];
        assert!(matches!(
            validate(&mission, &tasks, sample_policy(), duplicate_model),
            Err(PlanningValidationError::DuplicateModel { .. })
        ));

        let mut missing_endpoint = inventory();
        missing_endpoint.links = vec![link("a", 1, "m")];
        assert!(matches!(
            validate(&mission, &tasks, sample_policy(), missing_endpoint),
            Err(PlanningValidationError::LinkEndpointMissing { .. })
        ));

        let mut duplicate_link = inventory();
        duplicate_link.agents = vec![agent("a")];
        duplicate_link.models = vec![model(1, "m")];
        duplicate_link.links = vec![link("a", 1, "m"), link("a", 1, "m")];
        assert!(matches!(
            validate(&mission, &tasks, sample_policy(), duplicate_link),
            Err(PlanningValidationError::DuplicateLink { .. })
        ));
    }

    #[test]
    fn link_evidence_must_exist_be_bounded_and_preserve_conflicts() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];

        let mut empty = inventory();
        empty.agents = vec![agent("a")];
        empty.models = vec![model(1, "m")];
        empty.links = vec![AgentModelLink {
            agent: AgentKey("a".to_owned()),
            model: ModelRef {
                provider_id: 1,
                model: ModelKey("m".to_owned()),
            },
            evidence: Vec::new(),
        }];
        assert!(matches!(
            validate(&mission, &tasks, sample_policy(), empty),
            Err(PlanningValidationError::EmptyLinkEvidence { .. })
        ));

        let mut conflicting = inventory();
        conflicting.agents = vec![agent("a")];
        conflicting.models = vec![model(1, "m")];
        let mut link = link("a", 1, "m");
        link.evidence.push(CompatibilityEvidence {
            claim: EvidenceClaim::Unsupported,
            quality: EvidenceQuality::VersionMatched,
            source: EvidenceSource::VendorDocumentation,
            observed_at: Some(2),
            evidence_version: Some(VersionText("1.0".to_owned())),
        });
        link.evidence.push(CompatibilityEvidence {
            claim: EvidenceClaim::Unknown,
            quality: EvidenceQuality::Stale,
            source: EvidenceSource::UserReport,
            observed_at: None,
            evidence_version: None,
        });
        conflicting.links = vec![link];
        let snapshot = validate(&mission, &tasks, sample_policy(), conflicting).unwrap();
        let evidence = &snapshot.inventory.links[0].evidence;
        assert_eq!(evidence.len(), 3);
        assert!(
            evidence
                .iter()
                .any(|item| item.claim == EvidenceClaim::Supported)
        );
        assert!(
            evidence
                .iter()
                .any(|item| item.claim == EvidenceClaim::Unsupported)
        );
        assert!(
            evidence
                .iter()
                .any(|item| item.claim == EvidenceClaim::Unknown)
        );
        assert!(
            evidence
                .iter()
                .any(|item| item.quality == EvidenceQuality::Stale)
        );
    }

    #[test]
    fn evidence_provenance_is_preserved_for_capability_compatibility_and_locality() {
        let mission = sample_mission();
        let capability_evidence = vec![
            CapabilityEvidence {
                claim: EvidenceClaim::Supported,
                quality: EvidenceQuality::Verified,
                source: EvidenceSource::VendorDocumentation,
                observed_at: Some(3),
                evidence_version: Some(VersionText("9.0".to_owned())),
            },
            CapabilityEvidence {
                claim: EvidenceClaim::Unsupported,
                quality: EvidenceQuality::Stale,
                source: EvidenceSource::UserReport,
                observed_at: None,
                evidence_version: None,
            },
            CapabilityEvidence {
                claim: EvidenceClaim::Unknown,
                quality: EvidenceQuality::VersionMatched,
                source: EvidenceSource::LiveProbe,
                observed_at: Some(1),
                evidence_version: Some(VersionText("2.0".to_owned())),
            },
        ];
        let compatibility_evidence = vec![
            CompatibilityEvidence {
                claim: EvidenceClaim::Supported,
                quality: EvidenceQuality::Verified,
                source: EvidenceSource::VendorDocumentation,
                observed_at: Some(4),
                evidence_version: Some(VersionText("9.0".to_owned())),
            },
            CompatibilityEvidence {
                claim: EvidenceClaim::Unsupported,
                quality: EvidenceQuality::Stale,
                source: EvidenceSource::UserReport,
                observed_at: None,
                evidence_version: None,
            },
            CompatibilityEvidence {
                claim: EvidenceClaim::Unknown,
                quality: EvidenceQuality::VersionMatched,
                source: EvidenceSource::LiveProbe,
                observed_at: Some(2),
                evidence_version: Some(VersionText("2.0".to_owned())),
            },
        ];
        let locality_evidence = vec![
            LocalityEvidence {
                claim: DataLocality::Local,
                quality: EvidenceQuality::Verified,
                source: EvidenceSource::LocalObservation,
                observed_at: Some(5),
            },
            LocalityEvidence {
                claim: DataLocality::Remote,
                quality: EvidenceQuality::Stale,
                source: EvidenceSource::UserReport,
                observed_at: None,
            },
        ];

        let mut candidate = agent("a");
        candidate.locality = locality_evidence.clone();
        candidate.capabilities[0].evidence = capability_evidence.clone();
        let mut model_candidate = model(1, "m");
        model_candidate.locality = locality_evidence.clone();
        let mut compatibility = link("a", 1, "m");
        compatibility.evidence = compatibility_evidence.clone();

        let mut populated = inventory();
        populated.agents = vec![candidate];
        populated.models = vec![model_candidate];
        populated.links = vec![compatibility];

        let snapshot = validate(&mission, &[], sample_policy(), populated).unwrap();
        assert_eq!(
            snapshot.inventory.agents[0].capabilities[0].evidence,
            capability_evidence
        );
        assert_eq!(snapshot.inventory.links[0].evidence, compatibility_evidence);
        assert_eq!(snapshot.inventory.agents[0].locality, locality_evidence);
        assert_eq!(snapshot.inventory.models[0].locality, locality_evidence);
    }

    #[test]
    fn capability_entries_are_unique_and_hold_multiple_evidence_records() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];

        let mut duplicate = inventory();
        let mut candidate = agent("a");
        candidate.capabilities.push(capability(
            "coding",
            EvidenceClaim::Supported,
            EvidenceQuality::Verified,
        ));
        duplicate.agents = vec![candidate];
        assert!(matches!(
            validate(&mission, &tasks, sample_policy(), duplicate),
            Err(PlanningValidationError::DuplicateCapability { .. })
        ));

        let mut multiple = inventory();
        let mut candidate = agent("a");
        candidate.capabilities[0].evidence.push(CapabilityEvidence {
            claim: EvidenceClaim::Unsupported,
            quality: EvidenceQuality::VersionMatched,
            source: EvidenceSource::UserReport,
            observed_at: None,
            evidence_version: Some(VersionText("1.0".to_owned())),
        });
        multiple.agents = vec![candidate];
        let snapshot = validate(&mission, &tasks, sample_policy(), multiple).unwrap();
        assert_eq!(
            snapshot.inventory.agents[0].capabilities[0].evidence.len(),
            2
        );
    }

    #[test]
    fn capability_and_locality_evidence_bounds_are_enforced() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];

        let mut too_many_capabilities = inventory();
        let mut candidate = agent("a");
        candidate.capabilities = (0..=MAX_CAPABILITIES_PER_AGENT)
            .map(|index| {
                capability(
                    &format!("cap-{index}"),
                    EvidenceClaim::Supported,
                    EvidenceQuality::Verified,
                )
            })
            .collect();
        too_many_capabilities.agents = vec![candidate];
        assert!(matches!(
            validate(&mission, &tasks, sample_policy(), too_many_capabilities),
            Err(PlanningValidationError::TooManyCapabilities(_))
        ));

        let mut too_many_evidence = inventory();
        let mut candidate = agent("a");
        candidate.capabilities[0].evidence = (0..=MAX_EVIDENCE_PER_CAPABILITY)
            .map(|_| CapabilityEvidence {
                claim: EvidenceClaim::Supported,
                quality: EvidenceQuality::Verified,
                source: EvidenceSource::LocalObservation,
                observed_at: Some(1),
                evidence_version: Some(VersionText("1.0".to_owned())),
            })
            .collect();
        too_many_evidence.agents = vec![candidate];
        assert!(matches!(
            validate(&mission, &tasks, sample_policy(), too_many_evidence),
            Err(PlanningValidationError::TooManyCapabilityEvidence { .. })
        ));

        let mut too_many_locality = inventory();
        let mut candidate = agent("a");
        candidate.locality = vec![
            LocalityEvidence {
                claim: DataLocality::Local,
                quality: EvidenceQuality::Verified,
                source: EvidenceSource::LocalObservation,
                observed_at: Some(1),
            };
            MAX_LOCALITY_EVIDENCE_PER_CANDIDATE + 1
        ];
        too_many_locality.agents = vec![candidate];
        assert_eq!(
            validate(&mission, &tasks, sample_policy(), too_many_locality),
            Err(PlanningValidationError::TooManyLocalityEvidence)
        );
    }

    #[test]
    fn identifiers_labels_versions_and_pricing_are_validated() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];

        let mut bad_agent = inventory();
        let mut candidate = agent("a");
        candidate.key = AgentKey("a".repeat(MAX_AGENT_KEY_BYTES + 1));
        bad_agent.agents = vec![candidate];
        assert!(matches!(
            validate(&mission, &tasks, sample_policy(), bad_agent),
            Err(PlanningValidationError::InvalidIdentifier(_))
        ));

        let mut bad_label = inventory();
        let mut candidate = agent("a");
        candidate.label = "a".repeat(MAX_AGENT_LABEL_BYTES + 1);
        bad_label.agents = vec![candidate];
        assert!(matches!(
            validate(&mission, &tasks, sample_policy(), bad_label),
            Err(PlanningValidationError::InvalidLabel(_))
        ));

        let mut bad_version = inventory();
        let mut candidate = agent("a");
        candidate.capabilities[0].evidence[0].evidence_version =
            Some(VersionText("1".repeat(MAX_VERSION_BYTES + 1)));
        bad_version.agents = vec![candidate];
        assert!(matches!(
            validate(&mission, &tasks, sample_policy(), bad_version),
            Err(PlanningValidationError::InvalidIdentifier(_))
        ));

        let mut missing_currency = inventory();
        let mut priced = model(1, "m");
        priced.input_micros_per_million = Some(1);
        missing_currency.models = vec![priced];
        assert_eq!(
            validate(&mission, &tasks, sample_policy(), missing_currency),
            Err(PlanningValidationError::InvalidPricing(
                "Model pricing requires a currency."
            ))
        );

        let mut zero_prices = inventory();
        let mut priced = model(1, "m");
        priced.input_micros_per_million = Some(0);
        priced.output_micros_per_million = Some(0);
        priced.currency = Some("USD".to_owned());
        zero_prices.models = vec![priced];
        assert!(validate(&mission, &tasks, sample_policy(), zero_prices).is_ok());
    }

    #[test]
    fn pricing_combinations_are_validated_without_cost_math() {
        let mission = sample_mission();

        let mut absent = inventory();
        absent.models = vec![priced_model(None, None, None)];
        assert!(validate(&mission, &[], sample_policy(), absent).is_ok());

        let mut input_only = inventory();
        input_only.models = vec![priced_model(Some(1), None, Some("USD"))];
        assert!(validate(&mission, &[], sample_policy(), input_only).is_ok());

        let mut output_only = inventory();
        output_only.models = vec![priced_model(None, Some(1), Some("USD"))];
        assert!(validate(&mission, &[], sample_policy(), output_only).is_ok());

        let mut both = inventory();
        both.models = vec![priced_model(Some(1), Some(2), Some("USD"))];
        assert!(validate(&mission, &[], sample_policy(), both).is_ok());

        let mut missing_input_currency = inventory();
        missing_input_currency.models = vec![priced_model(Some(1), None, None)];
        assert_eq!(
            validate(&mission, &[], sample_policy(), missing_input_currency),
            Err(PlanningValidationError::InvalidPricing(
                "Model pricing requires a currency."
            ))
        );

        let mut missing_output_currency = inventory();
        missing_output_currency.models = vec![priced_model(None, Some(1), None)];
        assert_eq!(
            validate(&mission, &[], sample_policy(), missing_output_currency),
            Err(PlanningValidationError::InvalidPricing(
                "Model pricing requires a currency."
            ))
        );

        let mut empty_currency = inventory();
        empty_currency.models = vec![priced_model(None, None, Some(""))];
        assert!(matches!(
            validate(&mission, &[], sample_policy(), empty_currency),
            Err(PlanningValidationError::InvalidPricing(_))
        ));

        let mut long_currency = inventory();
        long_currency.models = vec![priced_model(
            Some(1),
            None,
            Some(&"U".repeat(MAX_CURRENCY_BYTES + 1)),
        )];
        assert!(matches!(
            validate(&mission, &[], sample_policy(), long_currency),
            Err(PlanningValidationError::InvalidPricing(_))
        ));
    }

    #[test]
    fn zero_budget_is_distinct_from_an_unspecified_budget() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];

        let mut unspecified = sample_policy();
        unspecified.budget = Some(PlanningBudget {
            currency: "USD".to_owned(),
            max_cost_micros: None,
            max_total_tokens: None,
            max_duration_secs: None,
        });
        assert!(validate(&mission, &tasks, unspecified, inventory()).is_ok());

        let mut zero = sample_policy();
        zero.budget = Some(PlanningBudget {
            currency: "USD".to_owned(),
            max_cost_micros: Some(0),
            max_total_tokens: Some(0),
            max_duration_secs: Some(0),
        });
        assert!(validate(&mission, &tasks, zero, inventory()).is_ok());

        let mut blank = sample_policy();
        blank.budget = Some(PlanningBudget {
            currency: String::new(),
            max_cost_micros: Some(1),
            max_total_tokens: None,
            max_duration_secs: None,
        });
        assert!(matches!(
            validate(&mission, &tasks, blank, inventory()),
            Err(PlanningValidationError::InvalidBudget(_))
        ));
    }

    #[test]
    fn empty_inventory_is_valid_and_no_link_means_unknown() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];
        let snapshot = validate(&mission, &tasks, sample_policy(), inventory()).unwrap();
        assert!(snapshot.inventory.agents.is_empty());
        assert!(snapshot.inventory.models.is_empty());
        assert!(snapshot.inventory.links.is_empty());
    }

    #[test]
    fn normalization_is_deterministic_and_preserves_all_records() {
        let mission = sample_mission();
        let tasks = vec![task(10, 1)];
        let mut first = inventory();
        let mut agent_b = agent("b");
        agent_b.capabilities.push(capability(
            "review",
            EvidenceClaim::Supported,
            EvidenceQuality::VersionMatched,
        ));
        first.agents = vec![agent_b, agent("a")];
        first.models = vec![model(2, "z"), model(1, "a")];
        first.links = vec![link("b", 2, "z"), link("a", 1, "a")];

        let mut first_policy = sample_policy();
        first_policy.mandatory = vec![
            CapabilityRequirement {
                capability: CapabilityId("review".to_owned()),
                minimum_evidence: MinimumEvidence::Verified,
            },
            CapabilityRequirement {
                capability: CapabilityId("coding".to_owned()),
                minimum_evidence: MinimumEvidence::VersionMatched,
            },
        ];
        let mut second_policy = first_policy.clone();
        second_policy.mandatory.reverse();
        let mut second = inventory();
        second.agents = first.agents.iter().cloned().rev().collect();
        second.models = first.models.iter().cloned().rev().collect();
        second.links = first.links.iter().cloned().rev().collect();

        let left = validate(&mission, &tasks, first_policy, first).unwrap();
        let right = validate(&mission, &tasks, second_policy, second).unwrap();
        assert_eq!(left.inventory, right.inventory);
        assert_eq!(left.policy, right.policy);
        assert_eq!(left.inventory.agents[0].key.0, "a");
        assert_eq!(left.inventory.models[0].key.0, "a");
        assert_eq!(left.inventory.links[0].agent.0, "a");
        assert_eq!(left.inventory.agents[0].capabilities.len(), 1);
        assert_eq!(left.inventory.agents[1].capabilities.len(), 2);
    }
}
