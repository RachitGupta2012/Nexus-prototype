//! Durable mission domain types.
//!
//! Phase 5 starts with the persistence model only. These types describe a
//! mission, its ordered tasks, execution attempts, checkpoints, and approval
//! requests. Nothing in this module decomposes, schedules, executes, or
//! approves work; the mission engine will consume this store later.

use serde_json::Value;

pub type MissionId = i64;
pub type MissionTaskId = i64;
pub type AttemptId = i64;
pub type CheckpointId = i64;
pub type ApprovalId = i64;

/// Bounded list sizes exposed by the store.
pub const MAX_MISSIONS: u32 = 200;
pub const MAX_TASKS: u32 = 1_000;
pub const MAX_ATTEMPTS: u32 = 200;
pub const MAX_CHECKPOINTS: u32 = 200;
pub const MAX_APPROVALS: u32 = 200;

/// Storage caps independent of list size.
pub const MAX_TASKS_PER_MISSION: u32 = 1_000;
pub const MAX_ATTEMPTS_PER_TASK: u32 = 200;
pub const MAX_CHECKPOINTS_PER_MISSION: u32 = 500;
pub const MAX_APPROVALS_PER_MISSION: u32 = 500;

pub const MAX_TITLE_BYTES: usize = 200;
pub const MAX_OBJECTIVE_BYTES: usize = 16_384;
pub const MAX_WORKSPACE_BYTES: usize = 4_096;
pub const MAX_MODEL_BYTES: usize = 160;
pub const MAX_JSON_BYTES: usize = 65_536;
pub const MAX_LABEL_BYTES: usize = 200;
pub const MAX_SUMMARY_BYTES: usize = 2_000;
pub const MAX_NOTE_BYTES: usize = 4_096;
pub const MAX_ERROR_BYTES: usize = 4_096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MissionStatus {
    Draft,
    Planned,
    Running,
    Paused,
    AwaitingApproval,
    Completed,
    Failed,
    Cancelled,
}

impl MissionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Planned => "planned",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "draft" => Self::Draft,
            "planned" => Self::Planned,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "awaiting_approval" => Self::AwaitingApproval,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub const fn can_transition_to(self, next: Self) -> bool {
        match self {
            Self::Draft => matches!(next, Self::Planned | Self::Cancelled),
            Self::Planned => matches!(next, Self::Running | Self::Cancelled),
            Self::Running => matches!(
                next,
                Self::Paused
                    | Self::AwaitingApproval
                    | Self::Completed
                    | Self::Failed
                    | Self::Cancelled
            ),
            Self::Paused => matches!(next, Self::Running | Self::Failed | Self::Cancelled),
            Self::AwaitingApproval => matches!(
                next,
                Self::Running | Self::Paused | Self::Failed | Self::Cancelled
            ),
            Self::Completed | Self::Failed | Self::Cancelled => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskStatus {
    Pending,
    Ready,
    Running,
    AwaitingApproval,
    Completed,
    Failed,
    Cancelled,
    Skipped,
}

impl TaskStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "pending" => Self::Pending,
            "ready" => Self::Ready,
            "running" => Self::Running,
            "awaiting_approval" => Self::AwaitingApproval,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "skipped" => Self::Skipped,
            _ => return None,
        })
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Skipped
        )
    }

    pub const fn can_transition_to(self, next: Self) -> bool {
        match self {
            Self::Pending => matches!(next, Self::Ready | Self::Cancelled | Self::Skipped),
            Self::Ready => matches!(
                next,
                Self::Running | Self::Cancelled | Self::Skipped | Self::Failed
            ),
            Self::Running => matches!(
                next,
                Self::AwaitingApproval | Self::Completed | Self::Failed | Self::Cancelled
            ),
            Self::AwaitingApproval => {
                matches!(next, Self::Running | Self::Failed | Self::Cancelled)
            }
            Self::Completed | Self::Failed | Self::Cancelled | Self::Skipped => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl AttemptStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckpointKind {
    Plan,
    Progress,
    TaskCompleted,
    Approval,
    Manual,
}

impl CheckpointKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Progress => "progress",
            Self::TaskCompleted => "task_completed",
            Self::Approval => "approval",
            Self::Manual => "manual",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "plan" => Self::Plan,
            "progress" => Self::Progress,
            "task_completed" => Self::TaskCompleted,
            "approval" => Self::Approval,
            "manual" => Self::Manual,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalKind {
    ToolExecution,
    Spending,
    NetworkAccess,
    FileWrite,
    Other,
}

impl ApprovalKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ToolExecution => "tool_execution",
            Self::Spending => "spending",
            Self::NetworkAccess => "network_access",
            Self::FileWrite => "file_write",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "tool_execution" => Self::ToolExecution,
            "spending" => Self::Spending,
            "network_access" => Self::NetworkAccess,
            "file_write" => Self::FileWrite,
            "other" => Self::Other,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
    Cancelled,
}

impl ApprovalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "pending" => Self::Pending,
            "approved" => Self::Approved,
            "denied" => Self::Denied,
            "expired" => Self::Expired,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    pub const fn is_decision(self) -> bool {
        !matches!(self, Self::Pending)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mission {
    pub id: MissionId,
    pub title: String,
    pub objective: String,
    pub constraints: Value,
    pub workspace: String,
    pub status: MissionStatus,
    pub created_at: u64,
    pub updated_at: u64,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MissionSummary {
    pub id: MissionId,
    pub title: String,
    pub status: MissionStatus,
    pub task_count: u32,
    pub pending_approval_count: u32,
    pub created_at: u64,
    pub updated_at: u64,
}

pub struct NewMission<'a> {
    pub title: &'a str,
    pub objective: &'a str,
    pub constraints: &'a Value,
    pub workspace: &'a str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MissionTask {
    pub id: MissionTaskId,
    pub mission_id: MissionId,
    pub ordinal: u32,
    pub title: String,
    pub objective: String,
    pub status: TaskStatus,
    pub provider_id: Option<u64>,
    pub model: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
}

pub struct NewMissionTask<'a> {
    pub title: &'a str,
    pub objective: &'a str,
    pub provider_id: Option<u64>,
    pub model: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskAttempt {
    pub id: AttemptId,
    pub task_id: MissionTaskId,
    pub attempt_number: u32,
    pub status: AttemptStatus,
    pub error: Option<String>,
    pub started_at: u64,
    pub completed_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Checkpoint {
    pub id: CheckpointId,
    pub mission_id: MissionId,
    pub task_id: Option<MissionTaskId>,
    pub kind: CheckpointKind,
    pub label: String,
    pub payload: Value,
    pub created_at: u64,
}

pub struct NewCheckpoint<'a> {
    pub task_id: Option<MissionTaskId>,
    pub kind: CheckpointKind,
    pub label: &'a str,
    pub payload: &'a Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Approval {
    pub id: ApprovalId,
    pub mission_id: MissionId,
    pub task_id: Option<MissionTaskId>,
    pub kind: ApprovalKind,
    pub summary: String,
    pub payload: Value,
    pub status: ApprovalStatus,
    pub requested_at: u64,
    pub decided_at: Option<u64>,
    pub decision_note: Option<String>,
}

pub struct NewApproval<'a> {
    pub task_id: Option<MissionTaskId>,
    pub kind: ApprovalKind,
    pub summary: &'a str,
    pub payload: &'a Value,
}

pub fn validate_text(
    value: &str,
    max_bytes: usize,
    message: &'static str,
) -> Result<(), &'static str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(message);
    }
    if value.len() > max_bytes || value.contains('\0') {
        return Err(message);
    }
    Ok(())
}

pub fn validate_optional_text(
    value: Option<&str>,
    max_bytes: usize,
    message: &'static str,
) -> Result<(), &'static str> {
    if let Some(value) = value
        && (value.len() > max_bytes || value.contains('\0'))
    {
        return Err(message);
    }
    Ok(())
}

/// Serializes a JSON object after checking its encoded size.
pub fn json_object(value: &Value, message: &'static str) -> Result<String, &'static str> {
    if !value.is_object() {
        return Err(message);
    }
    let encoded = serde_json::to_string(value).map_err(|_| message)?;
    if encoded.len() > MAX_JSON_BYTES {
        return Err(message);
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mission_transitions_are_explicit_and_terminal_states_stay_closed() {
        assert!(MissionStatus::Draft.can_transition_to(MissionStatus::Planned));
        assert!(MissionStatus::Planned.can_transition_to(MissionStatus::Running));
        assert!(MissionStatus::Running.can_transition_to(MissionStatus::AwaitingApproval));
        assert!(MissionStatus::AwaitingApproval.can_transition_to(MissionStatus::Running));
        assert!(!MissionStatus::Completed.can_transition_to(MissionStatus::Running));
        assert!(!MissionStatus::Draft.can_transition_to(MissionStatus::Running));
        assert!(!MissionStatus::Running.can_transition_to(MissionStatus::Running));
    }

    #[test]
    fn task_and_attempt_transitions_reject_restarts() {
        assert!(TaskStatus::Pending.can_transition_to(TaskStatus::Ready));
        assert!(TaskStatus::Ready.can_transition_to(TaskStatus::Running));
        assert!(TaskStatus::Running.can_transition_to(TaskStatus::Completed));
        assert!(!TaskStatus::Completed.can_transition_to(TaskStatus::Running));
        assert!(AttemptStatus::Succeeded.is_terminal());
        assert!(!AttemptStatus::Running.is_terminal());
    }

    #[test]
    fn validation_rejects_empty_oversized_and_non_object_values() {
        assert!(validate_text("  ", 10, "bad").is_err());
        assert!(validate_text("ok", 10, "bad").is_ok());
        assert!(validate_text("too long", 3, "bad").is_err());
        assert!(json_object(&serde_json::json!({}), "bad").is_ok());
        assert!(json_object(&serde_json::json!([]), "bad").is_err());
    }
}
