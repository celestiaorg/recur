use async_trait::async_trait;
use recur_domain::{BranchChoice, CopyFilesPreview, Project, ProjectGroup, ScheduledPrompt};

use crate::Result;

#[async_trait]
pub trait ProjectsService: Send + Sync {
    /// AddProject persists a new project and returns the saved copy (PRD §15.1). A
    /// stable slug id is derived from the name (or repo path) and de-duplicated; the
    /// avatar letter and a color are assigned if the caller left them empty.
    /// New projects default to expanded in the sidebar tree (Open=true).
    /// Empty BaseBranch is filled via SuggestBaseBranch (origin/main → main → …).
    async fn add_project(&self, p: &Project) -> Result<Project>;

    /// AddProjectGroup creates a sidebar project group. A UUID is assigned when id is
    /// empty; new groups default to Open=true.
    async fn add_project_group(&self, g: &ProjectGroup) -> Result<ProjectGroup>;

    /// DeleteProject removes a project and everything under it (loops, PRs, inbox rows),
    /// stopping any running loops first. The UI re-fetches its lists afterward.
    async fn delete_project(&self, id: &str) -> Result<()>;

    /// DeleteProjectGroup removes a group; its projects become ungrouped.
    async fn delete_project_group(&self, id: &str) -> Result<()>;

    /// DeleteScheduledPrompt keeps any workspaces already created by the schedule.
    async fn delete_scheduled_prompt(&self, id: &str) -> Result<()>;

    async fn get_project(&self, id: &str) -> Result<Project>;

    /// ListBranches returns base-branch picker choices for a project repo: remote-
    /// tracking refs first (one group per remote name), then local heads under
    /// "Local". Within each group tips are newest-first; the project's configured
    /// BaseBranch is promoted to the front of its group when present. An empty /
    /// unborn repo returns the intended base branch as a Local choice —
    /// StartWorkspace will create an empty initial commit on it.
    async fn list_branches(&self, project_id: &str) -> Result<Vec<BranchChoice>>;

    /// ListBranchesForPath is ListBranches for an unsaved new-project form: the
    /// picker needs remotes/locals before a project id exists. preferred is only
    /// used to promote an already-existing tip; it is never injected as a phantom
    /// choice (empty/stale preferred must not invent "main" or a prior repo's base).
    async fn list_branches_for_path(
        &self,
        repo_path: &str,
        preferred: &str,
    ) -> Result<Vec<BranchChoice>>;

    async fn list_project_groups(&self) -> Result<Vec<ProjectGroup>>;

    async fn list_projects(&self) -> Result<Vec<Project>>;

    /// ListScheduledPrompts lists schedules and their latest outcomes. An empty
    /// projectID returns schedules across all projects.
    async fn list_scheduled_prompts(&self, project_id: &str) -> Result<Vec<ScheduledPrompt>>;

    /// PauseScheduledPrompt changes whether future occurrences may launch.
    async fn pause_scheduled_prompt(&self, id: &str, paused: bool) -> Result<()>;

    /// PreviewCopyFiles lists files in the project's checkout that match the given
    /// glob patterns (same syntax as Project.CopyFiles). Used by project settings
    /// to preview what will be copied into new workspaces.
    async fn preview_copy_files(
        &self,
        project_id: &str,
        patterns: &str,
    ) -> Result<CopyFilesPreview>;

    /// PreviewScheduledPrompt validates timing and returns the next three run times.
    async fn preview_scheduled_prompt(&self, p: &ScheduledPrompt) -> Result<Vec<i64>>;

    /// ReorderProjectGroups persists a new sidebar group order.
    async fn reorder_project_groups(&self, ids: &[String]) -> Result<()>;

    /// ReorderProjects persists a new sidebar project order. ids must be a complete
    /// permutation of the current project ids.
    async fn reorder_projects(&self, ids: &[String]) -> Result<()>;

    /// RunScheduledPromptNow returns the workspace while setup proceeds in the background.
    async fn run_scheduled_prompt_now(&self, id: &str) -> Result<String>;

    /// SaveScheduledPrompt accepts only models offered by the existing model picker.
    async fn save_scheduled_prompt(&self, p: &ScheduledPrompt) -> Result<ScheduledPrompt>;

    /// SetProjectGroup assigns a project to a group (empty groupID = ungrouped).
    async fn set_project_group(&self, project_id: &str, group_id: &str) -> Result<Project>;

    /// SuggestBaseBranch returns the preferred default base for a repo path (used by
    /// the new-project form before the project exists). Non-git paths return "main".
    async fn suggest_base_branch(&self, repo_path: &str) -> Result<String>;

    /// UpdateProjectGroup persists name/open edits for an existing group.
    async fn update_project_group(&self, g: &ProjectGroup) -> Result<ProjectGroup>;

    /// UpdateProjectRunConfig persists edits to an existing project (PRD §15.1, §18).
    async fn update_project_run_config(&self, p: &Project) -> Result<Project>;
}
