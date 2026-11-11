//! Bounded, session-only task inputs for recommendation.

use std::collections::HashSet;

use thiserror::Error;

use crate::capability::CapabilityId;

pub const MAX_DESCRIPTION_BYTES: usize = 16 * 1024;
pub const MAX_REQUIREMENTS: usize = 12;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TaskId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TaskCategory {
    SoftwareEngineering,
    Research,
    DataAnalysis,
    DocumentProcessing,
    Writing,
    GeneralReasoning,
    Other,
}

impl TaskCategory {
    pub const ALL: [Self; 7] = [
        Self::SoftwareEngineering,
        Self::Research,
        Self::DataAnalysis,
        Self::DocumentProcessing,
        Self::Writing,
        Self::GeneralReasoning,
        Self::Other,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::SoftwareEngineering => "Software engineering",
            Self::Research => "Research",
            Self::DataAnalysis => "Data analysis",
            Self::DocumentProcessing => "Document processing",
            Self::Writing => "Writing",
            Self::GeneralReasoning => "General reasoning",
            Self::Other => "Other",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Task {
    pub id: TaskId,
    pub description: String,
    pub category: TaskCategory,
    pub mandatory: Vec<CapabilityId>,
    pub preferences: Vec<CapabilityId>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum TaskValidationError {
    #[error("task description cannot be empty")]
    EmptyDescription,
    #[error("task description exceeds the {MAX_DESCRIPTION_BYTES}-byte limit")]
    DescriptionTooLong,
    #[error("task has more than {MAX_REQUIREMENTS} total requirements and preferences")]
    TooManyRequirements,
    #[error("{0:?} is listed more than once")]
    DuplicateRequirement(CapabilityId),
}

impl Task {
    pub fn new(
        id: TaskId,
        description: String,
        category: TaskCategory,
        mandatory: Vec<CapabilityId>,
        preferences: Vec<CapabilityId>,
    ) -> Result<Self, TaskValidationError> {
        if description.trim().is_empty() {
            return Err(TaskValidationError::EmptyDescription);
        }
        if description.len() > MAX_DESCRIPTION_BYTES {
            return Err(TaskValidationError::DescriptionTooLong);
        }
        if mandatory.len() + preferences.len() > MAX_REQUIREMENTS {
            return Err(TaskValidationError::TooManyRequirements);
        }
        let mut seen = HashSet::new();
        for requirement in mandatory.iter().chain(preferences.iter()) {
            if !seen.insert(*requirement) {
                return Err(TaskValidationError::DuplicateRequirement(*requirement));
            }
        }
        Ok(Self {
            id,
            description,
            category,
            mandatory,
            preferences,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityId;

    fn task(description: &str) -> Result<Task, TaskValidationError> {
        Task::new(
            TaskId(1),
            description.to_owned(),
            TaskCategory::Other,
            vec![],
            vec![],
        )
    }

    #[test]
    fn accepts_valid_task_and_separates_required_and_preferred() {
        let value = Task::new(
            TaskId(7),
            "Review code".into(),
            TaskCategory::SoftwareEngineering,
            vec![CapabilityId::CodeEditing],
            vec![CapabilityId::StructuredOutput],
        )
        .unwrap();
        assert_eq!(value.mandatory.len(), 1);
        assert_eq!(value.preferences.len(), 1);
    }

    #[test]
    fn rejects_empty_and_oversized_descriptions() {
        assert_eq!(
            task("  ").unwrap_err(),
            TaskValidationError::EmptyDescription
        );
        assert_eq!(
            task(&"x".repeat(MAX_DESCRIPTION_BYTES + 1)).unwrap_err(),
            TaskValidationError::DescriptionTooLong
        );
    }

    #[test]
    fn rejects_duplicate_and_excessive_requirements() {
        assert!(matches!(
            Task::new(
                TaskId(1),
                "x".into(),
                TaskCategory::Other,
                vec![CapabilityId::CodeEditing],
                vec![CapabilityId::CodeEditing]
            ),
            Err(TaskValidationError::DuplicateRequirement(_))
        ));
        assert_eq!(
            Task::new(
                TaskId(1),
                "x".into(),
                TaskCategory::Other,
                vec![CapabilityId::CodeEditing; MAX_REQUIREMENTS + 1],
                vec![]
            )
            .unwrap_err(),
            TaskValidationError::TooManyRequirements
        );
    }
}
