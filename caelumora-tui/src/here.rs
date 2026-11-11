//! The place of the directory the terminal program was started in (issue 554): the storage of
//! workspaces that keeps every workspace there, and the project and the workspace that belong to
//! the directory, found by its name.

use anyhow::Result;
use caelumora_domain::project::ProjectId;
use caelumora_domain::scope::Scope;
use caelumora_domain::user::UserId;
use caelumora_domain::value::name::Name;
use caelumora_domain::workspace::WorkspaceId;
use caelumora_kernel::kernel::Kernel;
use caelumora_kernel::project::{ProjectManager, ProjectSpec};
use caelumora_kernel::workspace::WorkspaceSpec;
use caelumora_security::SecurityContext;
use std::path::Path;
use uuid::Uuid;

/// The storage of workspaces of the directory the program was started in, as its defaults name it.
pub(crate) const HERE: &str = "here";

/// What the project and the workspace of the directory say they are.
const WHAT: &str = "The directory the terminal program was started in";

/// The name of what belongs to the directory `dir` - its project, its workspace -: its own name,
/// and beside it a part of an id of its path, so that two directories of one name differ, and one
/// directory has the same name after every start. A directory moved or renamed is another.
///
/// # Errors
/// If what makes it is no name.
pub(crate) fn name_of(dir: &Path) -> Result<Name> {
    let own = dir.file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
    let own: String = own.chars().filter(|c| !c.is_control()).take(60).collect();
    let id = Uuid::new_v5(&Uuid::NAMESPACE_URL, dir.as_os_str().as_encoded_bytes()).simple();
    let tag = &id.to_string()[..8];
    let name = match own.trim() {
        "" => format!("root-{tag}"),
        own => format!("{own}-{tag}"),
    };
    Ok(Name::new(name)?)
}

/// The project and the workspace of the directory `here`, in the own scope of `owner`, for whom
/// `context` acts: found by the directory's name (`name_of`), or made - the project by `projects`,
/// the workspace in the storage `HERE`, which keeps every workspace in that directory. Two programs
/// started at once in a new directory find what the first made.
///
/// # Errors
/// What finding or making them reports - `InAnotherStorage` if the workspace of the directory's
/// name lies in another storage.
pub(crate) async fn place_of(
    kernel: &Kernel,
    projects: &ProjectManager,
    context: &SecurityContext,
    owner: UserId,
    here: &Path,
) -> Result<(ProjectId, WorkspaceId)> {
    let name = name_of(here)?;
    let scope = Scope::UserScope { user_id: owner };
    let project = ProjectSpec {
        description: Some(WHAT.to_string()),
        ..ProjectSpec::new(name.as_str(), scope.clone())
    };
    let (project, _) = projects.find_or_create(context, &project).await?;
    let workspace = WorkspaceSpec {
        description: Some(WHAT.to_string()),
        ..WorkspaceSpec::new(name.as_str(), scope, HERE)
    };
    let (workspace, _) = kernel.workspace_manager().find_or_create(context, &workspace).await?;
    Ok((project.id, workspace.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_has_one_name_and_another_of_its_name_another() {
        let one = name_of(Path::new("/home/a/caelumora")).unwrap();
        assert!(one.as_str().starts_with("caelumora-"), "its own name first: {one}");
        assert_eq!(one, name_of(Path::new("/home/a/caelumora")).unwrap(), "the same always");
        assert_ne!(one, name_of(Path::new("/home/b/caelumora")).unwrap(), "another elsewhere");
        let root = name_of(Path::new("/")).unwrap();
        assert!(root.as_str().starts_with("root-"), "the root has a name too: {root}");
        let odd = name_of(Path::new(&format!("/x/{}\nline", "a".repeat(200)))).unwrap();
        assert!(odd.as_str().chars().count() <= 69 && !odd.as_str().contains('\n'), "{odd}");
    }
}
