use eframe::egui;

use crate::mission::{
    Approval, ApprovalKind, AttemptStatus, Checkpoint, MAX_APPROVALS, MAX_ATTEMPTS,
    MAX_CHECKPOINTS, MAX_MISSIONS, MAX_OBJECTIVE_BYTES, MAX_STATUS_EVENTS, MAX_TASKS,
    MAX_TITLE_BYTES, MAX_WORKSPACE_BYTES, Mission, MissionId, MissionPrivacy, MissionStatus,
    MissionSummary, MissionTask, MissionTaskId, NewMission, NewMissionTask, TaskAttempt,
    TaskStatus, TaskStatusEvent, UpdateMission, UpdateMissionTask, creates_dependency_cycle,
    deliverable_list, json_object, validate_dependency_list, validate_text,
};
use crate::storage::{Storage, StorageError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Missions,
    Approvals,
}

#[derive(Clone, Copy, Debug)]
enum Action {
    RefreshMissions,
    RefreshApprovals,
    SelectMission(MissionId),
    ClearSelection,
    SelectTask(MissionTaskId),
    RefreshTask,
    NewMission,
    EditMission,
    CancelMissionForm,
    SaveMission,
    RequestDeleteMission,
    ConfirmDeleteMission,
    CancelDeleteMission,
    NewTask,
    EditTask,
    CancelTaskForm,
    SaveTask,
    MoveTaskUp,
    MoveTaskDown,
    RequestDeleteTask,
    ConfirmDeleteTask,
    CancelDeleteTask,
    EditDependencies,
    SaveDependencies,
    CancelDependencies,
}

#[derive(Clone)]
struct DependencyOption {
    id: MissionTaskId,
    ordinal: u32,
    title: String,
    depends_on: Vec<MissionTaskId>,
}

struct DependencyForm {
    task_id: MissionTaskId,
    task_label: String,
    options: Vec<DependencyOption>,
    selected: Vec<MissionTaskId>,
}

impl DependencyForm {
    fn allowed(&self) -> Vec<MissionTaskId> {
        self.options.iter().map(|option| option.id).collect()
    }

    fn edges(&self) -> Vec<(MissionTaskId, Vec<MissionTaskId>)> {
        self.options
            .iter()
            .map(|option| (option.id, option.depends_on.clone()))
            .collect()
    }

    fn selected_in_order(&self) -> Vec<MissionTaskId> {
        self.options
            .iter()
            .filter(|option| self.selected.contains(&option.id))
            .map(|option| option.id)
            .collect()
    }

    fn validation_error(&self) -> Option<&'static str> {
        validate_dependency_list(self.task_id, &self.selected, &self.allowed())
            .err()
            .or_else(|| {
                creates_dependency_cycle(self.task_id, &self.selected, &self.edges())
                    .then_some("This dependency would create a cycle.")
            })
    }
}

#[derive(Clone, Default)]
struct MissionDraft {
    title: String,
    objective: String,
    constraints: String,
    workspace: String,
    deliverables: String,
    privacy: MissionPrivacy,
}

impl MissionDraft {
    fn new() -> Self {
        Self {
            constraints: "{}".to_owned(),
            ..Self::default()
        }
    }

    fn from_mission(mission: &Mission) -> Self {
        let constraints =
            serde_json::to_string_pretty(&mission.constraints).unwrap_or_else(|_| "{}".to_owned());
        Self {
            title: mission.title.clone(),
            objective: mission.objective.clone(),
            constraints,
            workspace: mission.workspace.clone(),
            deliverables: mission.deliverables.join("\n"),
            privacy: mission.privacy,
        }
    }

    fn deliverables(&self) -> Vec<String> {
        self.deliverables
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn parsed_constraints(&self) -> Result<serde_json::Value, &'static str> {
        let value: serde_json::Value = serde_json::from_str(self.constraints.trim())
            .map_err(|_| "Constraints must be a JSON object.")?;
        json_object(&value, "Constraints must be a JSON object up to 65 KiB.")?;
        Ok(value)
    }

    fn errors(&self) -> MissionErrors {
        MissionErrors {
            title: validate_text(
                &self.title,
                MAX_TITLE_BYTES,
                "Enter a title of at most 200 bytes.",
            )
            .err(),
            objective: validate_text(
                &self.objective,
                MAX_OBJECTIVE_BYTES,
                "Enter an objective of at most 16 KiB.",
            )
            .err(),
            workspace: if self.workspace.len() > MAX_WORKSPACE_BYTES
                || self.workspace.contains('\0')
            {
                Some("Use a workspace path of at most 4 KiB.")
            } else {
                None
            },
            constraints: self.parsed_constraints().err(),
            deliverables: deliverable_list(
                &self.deliverables(),
                "Use at most 100 non-empty deliverables of 1,000 bytes each.",
            )
            .err(),
        }
    }
}

#[derive(Default)]
struct MissionErrors {
    title: Option<&'static str>,
    objective: Option<&'static str>,
    constraints: Option<&'static str>,
    workspace: Option<&'static str>,
    deliverables: Option<&'static str>,
}

impl MissionErrors {
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.objective.is_none()
            && self.constraints.is_none()
            && self.workspace.is_none()
            && self.deliverables.is_none()
    }
}

struct MissionForm {
    id: Option<MissionId>,
    draft: MissionDraft,
}

#[derive(Clone, Default)]
struct TaskDraft {
    title: String,
    objective: String,
}

impl TaskDraft {
    fn from_task(task: &MissionTask) -> Self {
        Self {
            title: task.title.clone(),
            objective: task.objective.clone(),
        }
    }

    fn errors(&self) -> TaskErrors {
        TaskErrors {
            title: validate_text(
                &self.title,
                MAX_TITLE_BYTES,
                "Enter a title of at most 200 bytes.",
            )
            .err(),
            objective: validate_text(
                &self.objective,
                MAX_OBJECTIVE_BYTES,
                "Enter an objective of at most 16 KiB.",
            )
            .err(),
        }
    }
}

#[derive(Default)]
struct TaskErrors {
    title: Option<&'static str>,
    objective: Option<&'static str>,
}

impl TaskErrors {
    fn is_empty(&self) -> bool {
        self.title.is_none() && self.objective.is_none()
    }
}

struct TaskForm {
    mission_id: MissionId,
    mission_title: String,
    id: Option<MissionTaskId>,
    draft: TaskDraft,
}

struct MissionDetail {
    mission: Mission,
    tasks: Vec<MissionTask>,
    tasks_overflow: bool,
}

struct TaskDetail {
    task: MissionTask,
    attempts: Vec<TaskAttempt>,
    history: Vec<TaskStatusEvent>,
    checkpoints: Vec<Checkpoint>,
    attempts_overflow: bool,
    history_overflow: bool,
}

pub struct MissionsScreen {
    view: View,
    loaded: bool,
    missions: Vec<MissionSummary>,
    missions_overflow: bool,
    selected: Option<MissionId>,
    detail: Option<MissionDetail>,
    missing_selection: bool,
    selected_task: Option<MissionTaskId>,
    task_detail: Option<TaskDetail>,
    pending: Vec<Approval>,
    pending_loaded: bool,
    pending_overflow: bool,
    error: Option<String>,
    forced_error: Option<&'static str>,
    mission_form: Option<MissionForm>,
    task_form: Option<TaskForm>,
    dependency_form: Option<DependencyForm>,
    confirming_mission_delete: bool,
    confirming_task_delete: bool,
}

impl Default for MissionsScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl MissionsScreen {
    pub fn new() -> Self {
        Self {
            view: View::Missions,
            loaded: false,
            missions: Vec::new(),
            missions_overflow: false,
            selected: None,
            detail: None,
            missing_selection: false,
            selected_task: None,
            task_detail: None,
            pending: Vec::new(),
            pending_loaded: false,
            pending_overflow: false,
            error: None,
            forced_error: None,
            mission_form: None,
            task_form: None,
            dependency_form: None,
            confirming_mission_delete: false,
            confirming_task_delete: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn force_error_for_test(&mut self, message: &'static str) {
        self.forced_error = Some(message);
        self.loaded = true;
        self.pending_loaded = true;
        self.error = Some(message.to_owned());
    }

    pub fn show(&mut self, ui: &mut egui::Ui, storage: &Storage) {
        self.ensure_loaded(storage);

        ui.heading("Missions");
        ui.label(
            "Define missions and tasks locally. Execution history is read-only, and this build \
             cannot run work or grant approvals.",
        );
        ui.add_space(6.0);

        let mut action = None;
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.view == View::Missions, "Missions")
                .clicked()
                && self.view != View::Missions
            {
                self.view = View::Missions;
                self.pending_loaded = false;
            }
            if ui
                .selectable_label(self.view == View::Approvals, "Approval queue")
                .clicked()
                && self.view != View::Approvals
            {
                self.view = View::Approvals;
            }
            ui.separator();
            let refresh = if self.view == View::Missions {
                "Refresh missions"
            } else {
                "Refresh approval queue"
            };
            if ui.button(refresh).clicked() {
                action = Some(match self.view {
                    View::Missions => Action::RefreshMissions,
                    View::Approvals => Action::RefreshApprovals,
                });
            }
            if ui.button("New mission").clicked() {
                action = Some(Action::NewMission);
            }
        });
        ui.separator();

        if let Some(error) = &self.error {
            ui.add_space(8.0);
            ui.group(|ui| {
                ui.colored_label(ui.visuals().error_fg_color, error);
                ui.label("The last successful data is kept below when available.");
            });
        }

        match self.view {
            View::Missions => self.show_missions(ui, &mut action),
            View::Approvals => self.show_approvals(ui),
        }

        if let Some(action) = action {
            self.handle(action, storage);
        }
    }

    fn ensure_loaded(&mut self, storage: &Storage) {
        if !self.loaded {
            self.refresh_missions(storage);
        }
        if self.view == View::Approvals && !self.pending_loaded {
            self.refresh_pending(storage);
        }
    }

    fn handle(&mut self, action: Action, storage: &Storage) {
        match action {
            Action::RefreshMissions => self.refresh_missions(storage),
            Action::RefreshApprovals => self.refresh_pending(storage),
            Action::SelectMission(id) => {
                self.selected = Some(id);
                self.missing_selection = false;
                self.selected_task = None;
                self.task_detail = None;
                self.task_form = None;
                self.dependency_form = None;
                self.detail = None;
                self.refresh_selected(storage);
            }
            Action::ClearSelection => {
                self.selected = None;
                self.missing_selection = false;
                self.selected_task = None;
                self.task_detail = None;
                self.detail = None;
            }
            Action::SelectTask(id) => {
                self.selected_task = Some(id);
                self.task_detail = None;
                self.task_form = None;
                self.dependency_form = None;
                if let Some(mission_id) = self.selected {
                    self.load_task(storage, mission_id, id);
                }
            }
            Action::RefreshTask => {
                if let (Some(mission_id), Some(task_id)) = (self.selected, self.selected_task) {
                    self.load_task(storage, mission_id, task_id);
                }
            }
            Action::NewMission => {
                self.view = View::Missions;
                self.task_form = None;
                self.dependency_form = None;
                self.confirming_mission_delete = false;
                self.confirming_task_delete = false;
                self.mission_form = Some(MissionForm {
                    id: None,
                    draft: MissionDraft::new(),
                });
            }
            Action::EditMission => {
                if let Some(detail) = &self.detail {
                    self.task_form = None;
                    self.dependency_form = None;
                    self.confirming_mission_delete = false;
                    self.mission_form = Some(MissionForm {
                        id: Some(detail.mission.id),
                        draft: MissionDraft::from_mission(&detail.mission),
                    });
                }
            }
            Action::CancelMissionForm => self.mission_form = None,
            Action::SaveMission => self.save_mission(storage),
            Action::RequestDeleteMission => self.confirming_mission_delete = true,
            Action::CancelDeleteMission => self.confirming_mission_delete = false,
            Action::ConfirmDeleteMission => self.delete_mission(storage),
            Action::NewTask => {
                if let Some(detail) = &self.detail {
                    self.mission_form = None;
                    self.dependency_form = None;
                    self.confirming_task_delete = false;
                    self.task_form = Some(TaskForm {
                        mission_id: detail.mission.id,
                        mission_title: detail.mission.title.clone(),
                        id: None,
                        draft: TaskDraft::default(),
                    });
                }
            }
            Action::EditTask => {
                if let (Some(detail), Some(task)) = (&self.detail, &self.task_detail) {
                    self.dependency_form = None;
                    self.confirming_task_delete = false;
                    self.task_form = Some(TaskForm {
                        mission_id: detail.mission.id,
                        mission_title: detail.mission.title.clone(),
                        id: Some(task.task.id),
                        draft: TaskDraft::from_task(&task.task),
                    });
                }
            }
            Action::CancelTaskForm => self.task_form = None,
            Action::SaveTask => self.save_task(storage),
            Action::MoveTaskUp => self.move_selected_task(-1, storage),
            Action::MoveTaskDown => self.move_selected_task(1, storage),
            Action::RequestDeleteTask => self.confirming_task_delete = true,
            Action::CancelDeleteTask => self.confirming_task_delete = false,
            Action::ConfirmDeleteTask => self.delete_task(storage),
            Action::EditDependencies => {
                if let (Some(detail), Some(task)) = (&self.detail, &self.task_detail) {
                    let options = detail
                        .tasks
                        .iter()
                        .map(|task| DependencyOption {
                            id: task.id,
                            ordinal: task.ordinal,
                            title: task.title.clone(),
                            depends_on: task.depends_on.clone(),
                        })
                        .collect();
                    self.task_form = None;
                    self.dependency_form = Some(DependencyForm {
                        task_id: task.task.id,
                        task_label: format!("#{} {}", task.task.ordinal, task.task.title),
                        options,
                        selected: task.task.depends_on.clone(),
                    });
                }
            }
            Action::CancelDependencies => self.dependency_form = None,
            Action::SaveDependencies => self.save_dependencies(storage),
        }
    }

    fn save_dependencies(&mut self, storage: &Storage) {
        let Some(form) = self.dependency_form.take() else {
            return;
        };
        if let Some(error) = form.validation_error() {
            self.dependency_form = Some(form);
            self.error = Some(error.to_owned());
            return;
        }
        let selected = form.selected_in_order();
        match storage.update_mission_task_dependencies(form.task_id, &selected) {
            Ok(()) => {
                self.selected_task = Some(form.task_id);
                self.task_detail = None;
                self.dependency_form = None;
                self.refresh_selected(storage);
            }
            Err(error) => {
                self.dependency_form = Some(form);
                self.error = Some(save_error(
                    error,
                    "The dependencies could not be saved. Check for cycles and definition state.",
                ));
            }
        }
    }

    fn save_mission(&mut self, storage: &Storage) {
        let Some(form) = self.mission_form.take() else {
            return;
        };
        let errors = form.draft.errors();
        if !errors.is_empty() {
            self.mission_form = Some(form);
            self.error = Some("Fix the highlighted mission fields before saving.".to_owned());
            return;
        }
        let Ok(constraints) = form.draft.parsed_constraints() else {
            self.mission_form = Some(form);
            self.error = Some("Fix the highlighted mission fields before saving.".to_owned());
            return;
        };
        let deliverables = form.draft.deliverables();
        let result = match form.id {
            Some(id) => storage
                .update_mission_definition(
                    id,
                    UpdateMission {
                        title: &form.draft.title,
                        objective: &form.draft.objective,
                        constraints: &constraints,
                        workspace: &form.draft.workspace,
                        deliverables: &deliverables,
                        privacy: form.draft.privacy,
                    },
                )
                .map(|()| id),
            None => storage.create_mission(NewMission {
                title: &form.draft.title,
                objective: &form.draft.objective,
                constraints: &constraints,
                workspace: &form.draft.workspace,
                deliverables: &deliverables,
                privacy: form.draft.privacy,
            }),
        };
        match result {
            Ok(id) => {
                self.selected = Some(id);
                self.selected_task = None;
                self.detail = None;
                self.task_detail = None;
                self.missing_selection = false;
                self.mission_form = None;
                self.confirming_mission_delete = false;
                self.refresh_missions(storage);
            }
            Err(error) => {
                self.mission_form = Some(form);
                self.error = Some(save_error(
                    error,
                    "The mission could not be saved. Check the fields and definition state.",
                ));
            }
        }
    }

    fn save_task(&mut self, storage: &Storage) {
        let Some(form) = self.task_form.take() else {
            return;
        };
        if !form.draft.errors().is_empty() {
            self.task_form = Some(form);
            self.error = Some("Fix the highlighted task fields before saving.".to_owned());
            return;
        }
        let result = match form.id {
            Some(id) => storage
                .update_mission_task_definition(
                    id,
                    UpdateMissionTask {
                        title: &form.draft.title,
                        objective: &form.draft.objective,
                    },
                )
                .map(|()| id),
            None => storage.add_mission_task(
                form.mission_id,
                NewMissionTask {
                    title: &form.draft.title,
                    objective: &form.draft.objective,
                    provider_id: None,
                    model: None,
                    depends_on: &[],
                },
            ),
        };
        match result {
            Ok(id) => {
                self.selected_task = Some(id);
                self.task_detail = None;
                self.task_form = None;
                self.confirming_task_delete = false;
                self.refresh_selected(storage);
            }
            Err(error) => {
                self.task_form = Some(form);
                self.error = Some(save_error(
                    error,
                    "The task could not be saved. Check the fields and definition state.",
                ));
            }
        }
    }

    fn move_selected_task(&mut self, delta: i32, storage: &Storage) {
        let (Some(detail), Some(selected)) = (&self.detail, self.selected_task) else {
            return;
        };
        let mut ids: Vec<_> = detail.tasks.iter().map(|task| task.id).collect();
        let Some(index) = ids.iter().position(|id| *id == selected) else {
            return;
        };
        let target = index as i32 + delta;
        if target < 0 || target as usize >= ids.len() {
            return;
        }
        ids.swap(index, target as usize);
        match storage.reorder_mission_tasks(detail.mission.id, &ids) {
            Ok(()) => self.refresh_selected(storage),
            Err(error) => {
                self.error = Some(save_error(
                    error,
                    "The task order could not be changed while the mission is locked.",
                ));
            }
        }
    }

    fn delete_mission(&mut self, storage: &Storage) {
        let Some(id) = self.selected else {
            return;
        };
        match storage.delete_mission(id) {
            Ok(()) => {
                self.selected = None;
                self.detail = None;
                self.selected_task = None;
                self.task_detail = None;
                self.mission_form = None;
                self.task_form = None;
                self.dependency_form = None;
                self.confirming_mission_delete = false;
                self.missing_selection = false;
                self.refresh_missions(storage);
            }
            Err(error) => {
                self.confirming_mission_delete = false;
                self.error = Some(save_error(
                    error,
                    "The mission could not be deleted from the local database.",
                ));
            }
        }
    }

    fn delete_task(&mut self, storage: &Storage) {
        let Some(id) = self.selected_task else {
            return;
        };
        match storage.delete_mission_task(id) {
            Ok(()) => {
                self.selected_task = None;
                self.task_detail = None;
                self.dependency_form = None;
                self.confirming_task_delete = false;
                self.refresh_selected(storage);
            }
            Err(error) => {
                self.confirming_task_delete = false;
                self.error = Some(save_error(
                    error,
                    "The task could not be deleted. Only unstarted tasks can be removed.",
                ));
            }
        }
    }

    fn refresh_missions(&mut self, storage: &Storage) {
        if let Some(message) = self.forced_error {
            self.error = Some(message.to_owned());
            self.loaded = true;
            return;
        }
        match storage.list_missions(MAX_MISSIONS) {
            Ok(missions) => {
                self.missions_overflow = missions.len() >= MAX_MISSIONS as usize;
                if self.selected.is_none() {
                    self.selected = missions.first().map(|mission| mission.id);
                }
                self.missions = missions;
                self.error = None;
                self.loaded = true;
                self.refresh_selected(storage);
            }
            Err(_) => {
                self.error = Some("Could not load missions from the local database.".to_owned());
                self.loaded = true;
                self.detail = None;
                self.task_detail = None;
            }
        }
    }

    fn refresh_selected(&mut self, storage: &Storage) {
        let Some(id) = self.selected else {
            self.detail = None;
            self.missing_selection = false;
            self.selected_task = None;
            self.task_detail = None;
            return;
        };
        match storage.load_mission(id) {
            Ok(Some(mission)) => {
                let tasks = match storage.list_mission_tasks(id, MAX_TASKS) {
                    Ok(tasks) => tasks,
                    Err(_) => {
                        self.error = Some("Could not load the tasks for this mission.".to_owned());
                        return;
                    }
                };
                if let Some(task_id) = self.selected_task
                    && !tasks.iter().any(|task| task.id == task_id)
                {
                    self.selected_task = None;
                    self.task_detail = None;
                }
                self.detail = Some(MissionDetail {
                    mission,
                    tasks_overflow: tasks.len() >= MAX_TASKS as usize,
                    tasks,
                });
                self.missing_selection = false;
                self.error = None;
                if let Some(task_id) = self.selected_task {
                    self.load_task(storage, id, task_id);
                }
            }
            Ok(None) => {
                self.detail = None;
                self.task_detail = None;
                self.missing_selection = true;
                self.error = None;
            }
            Err(_) => {
                self.error =
                    Some("Could not load this mission from the local database.".to_owned());
            }
        }
    }

    fn load_task(&mut self, storage: &Storage, mission_id: MissionId, task_id: MissionTaskId) {
        let task = match storage.load_mission_task(task_id) {
            Ok(Some(task)) if task.mission_id == mission_id => task,
            Ok(_) => {
                self.task_detail = None;
                self.error = Some("This task is no longer available.".to_owned());
                return;
            }
            Err(_) => {
                self.task_detail = None;
                self.error = Some("Could not load this task.".to_owned());
                return;
            }
        };
        let attempts = match storage.list_task_attempts(task_id, MAX_ATTEMPTS) {
            Ok(attempts) => attempts,
            Err(_) => {
                self.error = Some("Could not load task attempts.".to_owned());
                return;
            }
        };
        let history = match storage.list_task_status_history(task_id, MAX_STATUS_EVENTS) {
            Ok(history) => history,
            Err(_) => {
                self.error = Some("Could not load task status history.".to_owned());
                return;
            }
        };
        let checkpoints = match storage.list_checkpoints(mission_id, MAX_CHECKPOINTS) {
            Ok(checkpoints) => checkpoints
                .into_iter()
                .filter(|checkpoint| checkpoint.task_id == Some(task_id))
                .collect(),
            Err(_) => {
                self.error = Some("Could not load task checkpoints.".to_owned());
                return;
            }
        };
        self.task_detail = Some(TaskDetail {
            attempts_overflow: attempts.len() >= MAX_ATTEMPTS as usize,
            history_overflow: history.len() >= MAX_STATUS_EVENTS as usize,
            task,
            attempts,
            history,
            checkpoints,
        });
        self.error = None;
    }

    fn refresh_pending(&mut self, storage: &Storage) {
        if let Some(message) = self.forced_error {
            self.error = Some(message.to_owned());
            self.pending_loaded = true;
            return;
        }
        match storage.pending_approvals(MAX_APPROVALS) {
            Ok(pending) => {
                self.pending_overflow = pending.len() >= MAX_APPROVALS as usize;
                self.pending = pending;
                self.pending_loaded = true;
                self.error = None;
            }
            Err(_) => {
                self.error = Some("Could not load pending approvals.".to_owned());
                self.pending_loaded = true;
            }
        }
    }
}

impl MissionsScreen {
    fn show_missions(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        if let Some(form) = &mut self.mission_form {
            let height = ui.available_height().max(320.0);
            egui::ScrollArea::vertical()
                .id_salt("mission_form")
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| show_mission_form(ui, form, action));
            return;
        }
        if self.missions.is_empty() && self.missing_selection {
            ui.add_space(12.0);
            ui.label("This mission is no longer available.");
            ui.label("It may have been deleted. Refresh missions or clear the selection.");
            if ui.button("Clear selection").clicked() {
                *action = Some(Action::ClearSelection);
            }
            return;
        }
        if self.missions.is_empty() {
            ui.add_space(12.0);
            ui.label("No missions yet.");
            ui.label("Use New mission to define the first mission.");
            return;
        }

        let height = ui.available_height().max(320.0);
        ui.columns(2, |columns| {
            columns[0].set_min_width(230.0);
            columns[0].heading("Mission overview");
            egui::ScrollArea::vertical()
                .id_salt("mission_list")
                .max_height(height)
                .auto_shrink([false, false])
                .show(&mut columns[0], |ui| {
                    for mission in &self.missions {
                        let pending = if mission.pending_approval_count > 0 {
                            format!(" · {} pending", mission.pending_approval_count)
                        } else {
                            String::new()
                        };
                        let label = format!(
                            "{} · {}{}",
                            mission.title,
                            mission_status_label(mission.status),
                            pending
                        );
                        if ui
                            .selectable_label(self.selected == Some(mission.id), label)
                            .clicked()
                        {
                            *action = Some(Action::SelectMission(mission.id));
                        }
                    }
                    if self.missions_overflow {
                        ui.add_space(4.0);
                        ui.small(format!("Showing the first {MAX_MISSIONS} missions."));
                    }
                });

            columns[1].heading("Mission detail");
            let body = egui::ScrollArea::vertical()
                .id_salt("mission_detail")
                .max_height(height)
                .auto_shrink([false, false]);
            body.show(&mut columns[1], |ui| {
                if let Some(form) = &mut self.task_form {
                    show_task_form(ui, form, action);
                    return;
                }
                if let Some(form) = &mut self.dependency_form {
                    show_dependency_form(ui, form, action);
                    return;
                }
                if self.missing_selection {
                    ui.label("This mission is no longer available.");
                    ui.label("It may have been deleted. Refresh missions or clear the selection.");
                    if ui.button("Clear selection").clicked() {
                        *action = Some(Action::ClearSelection);
                    }
                    return;
                }
                let Some(detail) = &self.detail else {
                    ui.label("Select a mission.");
                    return;
                };
                self.show_mission_detail(ui, detail, action);
            });
        });
    }

    fn show_mission_detail(
        &self,
        ui: &mut egui::Ui,
        detail: &MissionDetail,
        action: &mut Option<Action>,
    ) {
        let mission = &detail.mission;
        ui.label(egui::RichText::new(&mission.title).heading());
        ui.horizontal(|ui| {
            let color = mission_status_color(ui, mission.status);
            ui.colored_label(
                color,
                egui::RichText::new(mission_status_label(mission.status)).strong(),
            );
            ui.small(format!("Mission #{}", mission.id));
        });
        ui.label(format!(
            "Created {} · Updated {}",
            format_time(mission.created_at),
            format_time(mission.updated_at)
        ));
        if let Some(started) = mission.started_at {
            ui.label(format!("Started {}", format_time(started)));
        }
        if let Some(completed) = mission.completed_at {
            ui.label(format!("Completed {}", format_time(completed)));
        }
        ui.label(format!("Privacy: {}", privacy_label(mission.privacy)));

        let editable = mission_is_editable(mission.status);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(editable, egui::Button::new("Edit mission"))
                .clicked()
            {
                *action = Some(Action::EditMission);
            }
            if ui
                .add_enabled(editable, egui::Button::new("Add task"))
                .clicked()
            {
                *action = Some(Action::NewTask);
            }
            if ui.button("Delete mission").clicked() {
                *action = Some(Action::RequestDeleteMission);
            }
        });
        if !editable {
            ui.weak("Mission definitions are locked after planning.");
        }
        if self.confirming_mission_delete {
            ui.group(|ui| {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "Permanently delete this mission?",
                );
                ui.label(
                    "All tasks, attempts, checkpoints, and approval records for this mission will                      also be deleted. This cannot be undone.",
                );
                ui.horizontal(|ui| {
                    if ui.button("Confirm delete mission").clicked() {
                        *action = Some(Action::ConfirmDeleteMission);
                    }
                    if ui.button("Cancel delete mission").clicked() {
                        *action = Some(Action::CancelDeleteMission);
                    }
                });
            });
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Objective").strong());
        ui.add(egui::Label::new(&mission.objective).wrap());

        if !mission.workspace.is_empty() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Workspace").strong());
            ui.monospace(&mission.workspace);
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Constraints").strong());
        if let Some(constraints) = mission.constraints.as_object() {
            if constraints.is_empty() {
                ui.weak("None.");
            } else {
                for (name, value) in constraints {
                    ui.horizontal_wrapped(|ui| {
                        ui.monospace(format!("{name}:"));
                        ui.label(json_preview(value));
                    });
                }
            }
        } else {
            ui.weak("No constraints recorded.");
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Deliverables").strong());
        if mission.deliverables.is_empty() {
            ui.weak("None.");
        } else {
            for deliverable in &mission.deliverables {
                ui.horizontal(|ui| {
                    ui.label("•");
                    ui.label(deliverable);
                });
            }
        }

        ui.add_space(10.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.heading("Tasks");
            ui.small(format!("{} ordered", detail.tasks.len()));
        });
        if detail.tasks.is_empty() {
            ui.label("No tasks recorded for this mission.");
        }
        for task in &detail.tasks {
            let selected = self.selected_task == Some(task.id);
            let label = format!(
                "#{} {} · {}",
                task.ordinal,
                task.title,
                task_status_label(task.status)
            );
            if ui.selectable_label(selected, label).clicked() {
                *action = Some(Action::SelectTask(task.id));
            }
            ui.small(format!(
                "Dependencies: {}",
                dependency_text(&detail.tasks, &task.depends_on)
            ));
        }
        if detail.tasks_overflow {
            ui.small(format!("Showing the first {MAX_TASKS} tasks."));
        }
        if let Some(selected) = self.selected_task
            && let Some(index) = detail.tasks.iter().position(|task| task.id == selected)
        {
            let selected_task = &detail.tasks[index];
            let task_editable = editable && selected_task.status == TaskStatus::Pending;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(editable && index > 0, egui::Button::new("Move task up"))
                    .clicked()
                {
                    *action = Some(Action::MoveTaskUp);
                }
                if ui
                    .add_enabled(
                        editable && index + 1 < detail.tasks.len(),
                        egui::Button::new("Move task down"),
                    )
                    .clicked()
                {
                    *action = Some(Action::MoveTaskDown);
                }
                if ui
                    .add_enabled(task_editable, egui::Button::new("Edit task"))
                    .clicked()
                {
                    *action = Some(Action::EditTask);
                }
                if ui
                    .add_enabled(task_editable, egui::Button::new("Delete task"))
                    .clicked()
                {
                    *action = Some(Action::RequestDeleteTask);
                }
            });
            if !task_editable {
                ui.weak("This task is locked because it has started or the mission is planned.");
            }
            if self.confirming_task_delete {
                ui.group(|ui| {
                    ui.colored_label(ui.visuals().error_fg_color, "Delete this task?");
                    ui.label(
                        "Dependencies on this task will be removed from other tasks. This cannot                          be undone.",
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Confirm delete task").clicked() {
                            *action = Some(Action::ConfirmDeleteTask);
                        }
                        if ui.button("Cancel delete task").clicked() {
                            *action = Some(Action::CancelDeleteTask);
                        }
                    });
                });
            }
        }

        ui.add_space(10.0);
        ui.separator();
        ui.heading("Task detail");
        let Some(task_detail) = &self.task_detail else {
            ui.label("Select a task to inspect attempts, checkpoints, and status history.");
            return;
        };
        let editable = mission_is_editable(detail.mission.status);
        ui.horizontal(|ui| {
            if ui.button("Refresh task").clicked() {
                *action = Some(Action::RefreshTask);
            }
            if ui
                .add_enabled(
                    editable && task_detail.task.status == TaskStatus::Pending,
                    egui::Button::new("Edit dependencies"),
                )
                .clicked()
            {
                *action = Some(Action::EditDependencies);
            }
        });
        self.show_task_detail(ui, task_detail);
    }

    fn show_task_detail(&self, ui: &mut egui::Ui, detail: &TaskDetail) {
        let task = &detail.task;
        ui.label(egui::RichText::new(format!("#{} {}", task.ordinal, task.title)).strong());
        ui.label(format!("Status: {}", task_status_label(task.status)));
        ui.add(egui::Label::new(&task.objective).wrap());
        ui.small(format!(
            "Dependencies: {}",
            dependency_text_from_detail(task, &self.detail)
        ));
        if let Some(detail) = &self.detail {
            let missing = task
                .depends_on
                .iter()
                .any(|id| !detail.tasks.iter().any(|task| task.id == *id));
            if missing {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "A saved dependency no longer belongs to this mission.",
                );
            }
            let edges: Vec<_> = detail
                .tasks
                .iter()
                .map(|task| (task.id, task.depends_on.clone()))
                .collect();
            if creates_dependency_cycle(task.id, &task.depends_on, &edges) {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "A dependency cycle exists in the saved mission data.",
                );
            }
        }
        if let Some(model) = &task.model {
            let provider = task
                .provider_id
                .map(|id| format!("provider #{id}"))
                .unwrap_or_else(|| "default provider".to_owned());
            ui.small(format!("Assignment: {provider} · {model}"));
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Attempts").strong());
        if detail.attempts.is_empty() {
            ui.weak("No attempts recorded.");
        }
        for attempt in &detail.attempts {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "Attempt {} · {}",
                    attempt.attempt_number,
                    attempt_status_label(attempt.status)
                ));
                ui.small(format!("started {}", format_time(attempt.started_at)));
                if let Some(completed) = attempt.completed_at {
                    ui.small(format!("· finished {}", format_time(completed)));
                }
            });
            if let Some(error) = &attempt.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        }
        if detail.attempts_overflow {
            ui.small(format!("Showing the first {MAX_ATTEMPTS} attempts."));
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Checkpoints").strong());
        if detail.checkpoints.is_empty() {
            ui.weak("No checkpoints for this task.");
        }
        for checkpoint in &detail.checkpoints {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "{} · {}",
                    checkpoint_kind_label(checkpoint.kind),
                    checkpoint.label
                ));
                ui.small(format_time(checkpoint.created_at));
            });
            ui.small(json_preview(&checkpoint.payload));
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Status history").strong());
        if detail.history.is_empty() {
            ui.weak("No status changes recorded.");
        }
        for event in &detail.history {
            ui.horizontal_wrapped(|ui| {
                ui.label(task_status_label(event.status));
                ui.small(format_time(event.created_at));
                if let Some(note) = &event.note {
                    ui.small(note);
                }
            });
        }
        if detail.history_overflow {
            ui.small(format!(
                "Showing the first {MAX_STATUS_EVENTS} status events."
            ));
        }
    }

    fn show_approvals(&self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.group(|ui| {
            ui.label(egui::RichText::new("Informational only").strong());
            ui.label(
                "These are pending approval records. This build cannot approve, deny, or execute them.",
            );
        });
        if self.pending.is_empty() {
            ui.add_space(12.0);
            ui.label("No pending approvals.");
            ui.label("Approval requests will appear here when a mission creates one.");
            return;
        }
        let height = ui.available_height().max(320.0);
        egui::ScrollArea::vertical()
            .id_salt("approval_queue")
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for approval in &self.pending {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(approval_kind_label(approval.kind)).strong(),
                            );
                            ui.small(format!("Mission #{}", approval.mission_id));
                            if let Some(task_id) = approval.task_id {
                                ui.small(format!("Task #{task_id}"));
                            }
                        });
                        ui.add(egui::Label::new(&approval.summary).wrap());
                        ui.small(format!("Requested {}", format_time(approval.requested_at)));
                        ui.label("Status: Pending · informational");
                        ui.small(json_preview(&approval.payload));
                    });
                }
                if self.pending_overflow {
                    ui.small(format!(
                        "Showing the first {MAX_APPROVALS} pending approvals."
                    ));
                }
            });
    }
}

fn show_mission_form(ui: &mut egui::Ui, form: &mut MissionForm, action: &mut Option<Action>) {
    ui.heading(if form.id.is_some() {
        "Edit mission"
    } else {
        "New mission"
    });
    ui.label(
        "Definition only. Saving changes metadata, order, and dependencies; it never rewrites          attempts, checkpoints, or status history.",
    );
    let errors = form.draft.errors();

    let label = ui.label("Title");
    let field = ui.add(
        egui::TextEdit::singleline(&mut form.draft.title)
            .desired_width(f32::INFINITY)
            .hint_text("Short mission title"),
    );
    field.labelled_by(label.id);
    field_error(ui, errors.title);

    let label = ui.label("Objective");
    let field = ui.add(
        egui::TextEdit::multiline(&mut form.draft.objective)
            .desired_rows(4)
            .desired_width(f32::INFINITY)
            .hint_text("What should this mission achieve?"),
    );
    field.labelled_by(label.id);
    field_error(ui, errors.objective);

    let label = ui.label("Workspace");
    let field = ui.add(
        egui::TextEdit::singleline(&mut form.draft.workspace)
            .desired_width(f32::INFINITY)
            .hint_text("Optional local workspace path"),
    );
    field.labelled_by(label.id);
    field_error(ui, errors.workspace);

    let label = ui.label("Privacy");
    let combo = egui::ComboBox::from_id_salt("mission_privacy")
        .selected_text(privacy_label(form.draft.privacy))
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut form.draft.privacy,
                MissionPrivacy::LocalOnly,
                privacy_label(MissionPrivacy::LocalOnly),
            );
            ui.selectable_value(
                &mut form.draft.privacy,
                MissionPrivacy::RemoteAllowed,
                privacy_label(MissionPrivacy::RemoteAllowed),
            );
        });
    combo.response.labelled_by(label.id);
    ui.small("Privacy is a recorded policy; this build does not execute work.");

    let label = ui.label("Constraints (JSON object)");
    let field = ui.add(
        egui::TextEdit::multiline(&mut form.draft.constraints)
            .desired_rows(5)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace)
            .hint_text("{\"offline\": true}"),
    );
    field.labelled_by(label.id);
    field_error(ui, errors.constraints);

    let label = ui.label("Deliverables (one per line)");
    let field = ui.add(
        egui::TextEdit::multiline(&mut form.draft.deliverables)
            .desired_rows(4)
            .desired_width(f32::INFINITY)
            .hint_text("A tested release\nRelease notes"),
    );
    field.labelled_by(label.id);
    field_error(ui, errors.deliverables);

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(errors.is_empty(), egui::Button::new("Save mission"))
            .clicked()
        {
            *action = Some(Action::SaveMission);
        }
        if ui.button("Cancel mission").clicked() {
            *action = Some(Action::CancelMissionForm);
        }
    });
    if !errors.is_empty() {
        ui.colored_label(
            ui.visuals().error_fg_color,
            "Correct the highlighted fields to enable saving.",
        );
    }
}

fn show_dependency_form(ui: &mut egui::Ui, form: &mut DependencyForm, action: &mut Option<Action>) {
    ui.heading("Edit dependencies");
    ui.label(format!("Task: {}", form.task_label));
    ui.label("A task becomes ready only after all selected dependencies complete.");
    let options: Vec<_> = form
        .options
        .iter()
        .filter(|option| option.id != form.task_id)
        .map(|option| (option.id, option.ordinal, option.title.clone()))
        .collect();
    if options.is_empty() {
        ui.weak("No other tasks are available in this mission.");
    }
    for (id, ordinal, title) in options {
        let mut checked = form.selected.contains(&id);
        if ui
            .checkbox(&mut checked, format!("Depends on #{ordinal} {title}"))
            .changed()
        {
            if checked {
                form.selected.push(id);
            } else {
                form.selected.retain(|dependency| *dependency != id);
            }
        }
    }
    let allowed = form.allowed();
    let missing: Vec<_> = form
        .selected
        .iter()
        .filter(|id| !allowed.contains(id))
        .copied()
        .collect();
    for id in missing {
        let mut keep = true;
        if ui
            .checkbox(&mut keep, format!("Missing dependency #{id} — remove"))
            .changed()
            && !keep
        {
            form.selected.retain(|dependency| *dependency != id);
        }
    }
    if let Some(error) = form.validation_error() {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                form.validation_error().is_none(),
                egui::Button::new("Save dependencies"),
            )
            .clicked()
        {
            *action = Some(Action::SaveDependencies);
        }
        if ui.button("Cancel dependencies").clicked() {
            *action = Some(Action::CancelDependencies);
        }
    });
}

fn show_task_form(ui: &mut egui::Ui, form: &mut TaskForm, action: &mut Option<Action>) {
    ui.heading(if form.id.is_some() {
        "Edit task"
    } else {
        "New task"
    });
    ui.label(format!("Mission: {}", form.mission_title));
    ui.label("Task definitions are locked once the task starts or the mission is planned.");
    let errors = form.draft.errors();

    let label = ui.label("Task title");
    let field = ui.add(
        egui::TextEdit::singleline(&mut form.draft.title)
            .desired_width(f32::INFINITY)
            .hint_text("Short task title"),
    );
    field.labelled_by(label.id);
    field_error(ui, errors.title);

    let label = ui.label("Task objective");
    let field = ui.add(
        egui::TextEdit::multiline(&mut form.draft.objective)
            .desired_rows(5)
            .desired_width(f32::INFINITY)
            .hint_text("What should this task produce?"),
    );
    field.labelled_by(label.id);
    field_error(ui, errors.objective);

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(errors.is_empty(), egui::Button::new("Save task"))
            .clicked()
        {
            *action = Some(Action::SaveTask);
        }
        if ui.button("Cancel task").clicked() {
            *action = Some(Action::CancelTaskForm);
        }
    });
}

fn field_error(ui: &mut egui::Ui, error: Option<&'static str>) {
    if let Some(error) = error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
}

fn privacy_label(privacy: MissionPrivacy) -> &'static str {
    match privacy {
        MissionPrivacy::LocalOnly => "Local only",
        MissionPrivacy::RemoteAllowed => "Remote providers allowed",
    }
}

fn mission_is_editable(status: MissionStatus) -> bool {
    matches!(status, MissionStatus::Draft | MissionStatus::Planned)
}

fn save_error(error: StorageError, fallback: &'static str) -> String {
    match error {
        StorageError::InvalidMission(_) => fallback.to_owned(),
        StorageError::HistoryDisabled => {
            "History is disabled, so mission changes cannot be saved.".to_owned()
        }
        _ => "The local database could not complete this change.".to_owned(),
    }
}

fn mission_status_label(status: crate::mission::MissionStatus) -> &'static str {
    match status {
        crate::mission::MissionStatus::Draft => "Draft",
        crate::mission::MissionStatus::Planned => "Planned",
        crate::mission::MissionStatus::Running => "Running",
        crate::mission::MissionStatus::Paused => "Paused",
        crate::mission::MissionStatus::AwaitingApproval => "Awaiting approval",
        crate::mission::MissionStatus::Completed => "Completed",
        crate::mission::MissionStatus::Failed => "Failed",
        crate::mission::MissionStatus::Cancelled => "Cancelled",
    }
}

fn mission_status_color(ui: &egui::Ui, status: crate::mission::MissionStatus) -> egui::Color32 {
    match status {
        crate::mission::MissionStatus::Completed => ui.visuals().hyperlink_color,
        crate::mission::MissionStatus::Failed | crate::mission::MissionStatus::Cancelled => {
            ui.visuals().error_fg_color
        }
        crate::mission::MissionStatus::Paused | crate::mission::MissionStatus::AwaitingApproval => {
            ui.visuals().warn_fg_color
        }
        _ => ui.visuals().text_color(),
    }
}

fn task_status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "Pending",
        TaskStatus::Ready => "Ready",
        TaskStatus::Running => "Running",
        TaskStatus::AwaitingApproval => "Awaiting approval",
        TaskStatus::Completed => "Completed",
        TaskStatus::Failed => "Failed",
        TaskStatus::Cancelled => "Cancelled",
        TaskStatus::Skipped => "Skipped",
    }
}

fn attempt_status_label(status: AttemptStatus) -> &'static str {
    match status {
        AttemptStatus::Running => "Running",
        AttemptStatus::Succeeded => "Succeeded",
        AttemptStatus::Failed => "Failed",
        AttemptStatus::Cancelled => "Cancelled",
    }
}

fn checkpoint_kind_label(kind: crate::mission::CheckpointKind) -> &'static str {
    match kind {
        crate::mission::CheckpointKind::Plan => "Plan",
        crate::mission::CheckpointKind::Progress => "Progress",
        crate::mission::CheckpointKind::TaskCompleted => "Task completed",
        crate::mission::CheckpointKind::Approval => "Approval",
        crate::mission::CheckpointKind::Manual => "Manual",
    }
}

fn approval_kind_label(kind: ApprovalKind) -> &'static str {
    match kind {
        ApprovalKind::ToolExecution => "Tool execution",
        ApprovalKind::Spending => "Spending",
        ApprovalKind::NetworkAccess => "Network access",
        ApprovalKind::FileWrite => "File write",
        ApprovalKind::Other => "Approval",
    }
}

fn dependency_text(tasks: &[MissionTask], dependencies: &[MissionTaskId]) -> String {
    if dependencies.is_empty() {
        return "None".to_owned();
    }
    dependencies
        .iter()
        .map(|id| {
            tasks
                .iter()
                .find(|task| task.id == *id)
                .map(|task| format!("#{} {}", task.ordinal, task.title))
                .unwrap_or_else(|| format!("Task #{id} (unlisted)"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn dependency_text_from_detail(task: &MissionTask, detail: &Option<MissionDetail>) -> String {
    match detail {
        Some(detail) => dependency_text(&detail.tasks, &task.depends_on),
        None => dependency_text(&[], &task.depends_on),
    }
}

fn json_preview(value: &serde_json::Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "Unavailable".to_owned());
    truncate(&text, 320)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut result: String = text.chars().take(max_chars).collect();
    result.push('…');
    result
}

fn format_time(seconds: u64) -> String {
    use time::OffsetDateTime;
    let Ok(timestamp) = i64::try_from(seconds) else {
        return "Unknown".to_owned();
    };
    let Ok(time) = OffsetDateTime::from_unix_timestamp(timestamp) else {
        return "Unknown".to_owned();
    };
    time.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "Unknown".to_owned())
}
