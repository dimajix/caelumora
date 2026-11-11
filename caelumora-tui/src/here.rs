//! The place of the directory the terminal program was started in (issue 554): the storage of
//! workspaces that keeps every workspace there, and the project and the workspace that belong to
//! the directory, found by its name.

use anyhow::Result;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::project::ProjectId;
use caelumora_domain::scope::Scope;
use caelumora_domain::store::StoreResultExt;
use caelumora_domain::user::UserId;
use caelumora_domain::value::name::{Name, ScopedName};
use caelumora_domain::workspace::WorkspaceId;
use caelumora_kernel::kernel::Kernel;
use caelumora_kernel::project::ProjectManager;
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
/// the workspace in the storage `HERE`, which keeps every workspace in that directory.
///
/// # Errors
/// Whatever finding or making them reports.
pub(crate) async fn place_of(
    kernel: &Kernel,
    projects: &ProjectManager,
    context: &SecurityContext,
    owner: UserId,
    here: &Path,
) -> Result<(ProjectId, WorkspaceId)> {
    let name = ScopedName { name: name_of(here)?, scope: Scope::UserScope { user_id: owner } };
    let stores = kernel.entity_stores();
    let project = match stores.projects.find_by_name(context, &name).await.optional()? {
        Some(project) => project.id,
        None => projects.create(context, name.as_str(), name.scope.clone(), Some(WHAT)).await?.id,
    };
    let workspace = match stores.workspaces.find_by_name(context, &name).await.optional()? {
        Some(workspace) => workspace.id,
        None => {
            // As a channel of a template that names none is classified, so that the two flow
            // both ways.
            let classification = Classification::CHANNEL_DEFAULT;
            let scope = name.scope.clone();
            let workspaces = kernel.workspace_manager();
            workspaces
                .create(context, name.as_str(), scope, HERE, Some(WHAT), classification)
                .await?
                .id
        }
    };
    Ok((project, workspace))
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
