//! `FileUserProvider`: the users of a YAML file written by hand (issue 25) - the smallest source
//! of identities that verifies a credential.

use crate::config::parse_yaml;
use crate::role::Roles;
use crate::user::{User, UserId};
use crate::user_provider::{AuthError, AuthResult, Credential, UserProvider};
use crate::value::name::Name;
use argon2::password_hash::Error as HashError;
use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordVerifier};
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// The users of a file written by hand, each with their id, name, email, roles and the Argon2
/// hash of their password, as a PHC string:
///
/// ```yaml
/// users:
///   - id: 6f1c4f0e-3a51-4c5e-9b0e-2d8f3c1a7b45
///     username: alice
///     email: alice@example.com
///     roles: [staff, hr]
///     password_hash: "$argon2id$v=19$m=19456,t=2,p=1$…$…"
/// ```
///
/// The id is written into the file, so that a user keeps it when renamed. Roles are named by the
/// names of configured roles. The file is read anew at every question, so that a user added,
/// changed or removed counts without a restart - removed, they can no longer log in, and a token
/// of theirs is not renewed.
///
/// A file that cannot be read or is no valid user file - a missing field, an unknown one, a name
/// or id given twice, a role not configured, a hash that is no Argon2 hash - answers every
/// question with `AuthError::Backend`, which denies rather than passing the question on. It is
/// read once when the provider is made, too, so that such a file is refused at startup.
#[derive(Debug)]
pub struct FileUserProvider {
    path: PathBuf,
    /// The configured roles, which the file names by their names.
    roles: Roles,
}

/// The parameters of `file` (issue 324): the file the users are read from.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileUserConfig {
    pub path: PathBuf,
}

impl FileUserConfig {
    /// The provider it describes, naming roles of the configured `roles` - none, if no roles are.
    ///
    /// # Errors
    /// `AuthError::Backend` if its file cannot be read or is no valid user file.
    pub fn provider(&self, roles: Option<&Roles>) -> AuthResult<FileUserProvider> {
        FileUserProvider::open(&self.path, roles.cloned().unwrap_or_default())
    }
}

/// The file as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserFile {
    #[serde(default)]
    users: Vec<UserEntry>,
}

/// One user of the file, as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserEntry {
    id: UserId,
    username: Name,
    #[serde(default)]
    email: String,
    #[serde(default)]
    roles: Vec<String>,
    password_hash: String,
}

/// A user of the file, with the hash their password is verified against.
struct Known {
    user: User,
    password_hash: PasswordHash,
}

impl FileUserProvider {
    /// A provider of the users in the file at `path`, naming roles of `roles`. The file is read
    /// now, and it is warned if others may read or change it.
    ///
    /// # Errors
    /// `AuthError::Backend` if the file cannot be read or is no valid user file.
    pub fn open(path: impl Into<PathBuf>, roles: Roles) -> AuthResult<Self> {
        let provider = Self { path: path.into(), roles };
        let content = std::fs::read_to_string(&provider.path).map_err(unreadable)?;
        provider.parse(&content)?;
        warn_if_exposed(&provider.path);
        Ok(provider)
    }

    /// The users of the file as it is now.
    async fn users(&self) -> AuthResult<Vec<Known>> {
        let content = tokio::fs::read_to_string(&self.path).await.map_err(unreadable)?;
        self.parse(&content)
    }

    /// The users `content` holds, every entry checked.
    fn parse(&self, content: &str) -> AuthResult<Vec<Known>> {
        let file: UserFile = parse_yaml(content).map_err(|e| invalid(&e))?;
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        file.users.into_iter()
            .map(|entry| {
                if !ids.insert(entry.id) {
                    return Err(invalid(&format!("the id {} is given twice", entry.id)));
                }
                if !names.insert(entry.username.clone()) {
                    return Err(invalid(&format!("the username '{}' is given twice", entry.username)));
                }
                let of_user = |e: &dyn ToString| invalid(&format!("user '{}': {}", entry.username, e.to_string()));
                let roles = self.roles.ids_of(&entry.roles).map_err(|e| of_user(&e))?;
                let password_hash = argon2_hash(&entry.password_hash).map_err(|e| of_user(&e))?;
                let user = User { id: entry.id, username: entry.username, email: entry.email, roles, deleted: None };
                Ok(Known { user, password_hash })
            })
            .collect()
    }
}

#[async_trait]
impl UserProvider for FileUserProvider {
    async fn find_user(&self, username: &str) -> AuthResult<Option<User>> {
        Ok(self.users().await?.into_iter().find(|known| known.user.username == username).map(|known| known.user))
    }

    async fn get_user(&self, id: UserId) -> AuthResult<Option<User>> {
        Ok(self.users().await?.into_iter().find(|known| known.user.id == id).map(|known| known.user))
    }

    /// A password is verified against the user's hash; `Credential::Trusted` is taken for every
    /// user of the file. A name the file does not know is not its own.
    async fn authenticate(&self, username: &str, credential: &Credential<'_>) -> AuthResult<Option<User>> {
        let mut users = self.users().await?;
        let found = users.iter().position(|known| known.user.username == username).map(|at| users.swap_remove(at));
        match credential {
            Credential::Trusted => Ok(found.map(|known| known.user)),
            Credential::Password(password) => {
                // For a name the file does not know, another user's hash is verified all the
                // same, and the answer dropped: the time a login takes does not tell which names
                // exist.
                let Some(hash) = found.as_ref().or(users.first()).map(|known| known.password_hash.clone()) else {
                    return Ok(None);
                };
                let verified = verify(password, hash).await?;
                match found {
                    Some(known) if verified => Ok(Some(known.user)),
                    Some(_) => Err(AuthError::InvalidCredentials),
                    None => Ok(None),
                }
            }
        }
    }
}

/// `text` as an Argon2 hash whose parameters Argon2 takes.
fn argon2_hash(text: &str) -> Result<PasswordHash, String> {
    let hash = PasswordHash::new(text).map_err(|e| format!("the password hash is no Argon2 hash: {e}"))?;
    Algorithm::try_from(hash.algorithm.as_str()).map_err(|_| "the password hash is no Argon2 hash".to_string())?;
    Params::try_from(&hash).map_err(|e| format!("the password hash has invalid parameters: {e}"))?;
    Ok(hash)
}

/// Whether `password` is what `hash` was made of. Computed on a blocking thread: a password hash
/// is made to take long.
async fn verify(password: &str, hash: PasswordHash) -> AuthResult<bool> {
    let failed = |e: &dyn ToString| AuthError::Backend(format!("the password could not be verified: {}", e.to_string()));
    let password = password.to_owned();
    let verified = tokio::task::spawn_blocking(move || Argon2::default().verify_password(password.as_bytes(), &hash))
        .await
        .map_err(|e| failed(&e))?;
    match verified {
        Ok(()) => Ok(true),
        Err(HashError::PasswordInvalid) => Ok(false),
        Err(e) => Err(failed(&e)),
    }
}

/// The error of a user file that cannot be read - naming no path, as no error does.
fn unreadable(e: io::Error) -> AuthError {
    AuthError::Backend(format!("the user file cannot be read: {e}"))
}

/// The error of a user file that is not valid, for `why`.
fn invalid(why: &dyn ToString) -> AuthError {
    AuthError::Backend(format!("the user file is invalid: {}", why.to_string()))
}

/// Warns if the file at `path` may be read by others than its owner and group, or changed by
/// others than its owner: who may change it may log in as anyone.
#[cfg(unix)]
fn warn_if_exposed(path: &Path) {
    if let Ok(metadata) = std::fs::metadata(path) {
        if exposed(metadata.permissions().mode()) {
            tracing::warn!("The user file {} may be read by others or changed by others than its owner - \
                restrict it, e.g. with chmod 600", path.display());
        }
    }
}

#[cfg(not(unix))]
fn warn_if_exposed(_path: &Path) {}

/// Whether a file of the Unix permissions `mode` may be read by others, or changed by others
/// than its owner.
#[cfg(unix)]
fn exposed(mode: u32) -> bool {
    mode & 0o026 != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::role::{RoleError, RoleId};
    use argon2::{PasswordHasher, Version};
    use std::fs;
    use tempfile::TempDir;

    /// The hash of `password`, made cheaply - a real one takes long on purpose.
    fn hash(password: &str) -> String {
        let params = Params::new(8, 1, 1, None).expect("valid parameters");
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_with_salt(password.as_bytes(), b"caelumora-tests!")
            .expect("hashed")
            .to_string()
    }

    /// The YAML entry of a user of that id, name, roles and password.
    fn entry(id: UserId, username: &str, roles: &str, password: &str) -> String {
        format!("  - id: {id}\n    username: {username}\n    roles: {roles}\n    password_hash: \"{}\"\n", hash(password))
    }

    /// A directory with the user file `users.yaml` of `content`, and its path.
    fn user_file(content: &str) -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("users.yaml");
        fs::write(&path, content).expect("written");
        (dir, path)
    }

    /// The roles `staff` and `hr`, and their ids.
    fn roles() -> (Roles, RoleId, RoleId) {
        let staff = RoleId::new();
        let hr = RoleId::new();
        let roles = Roles::new([(staff, "staff".to_string(), vec![]), (hr, "hr".to_string(), vec![])]).expect("valid roles");
        (roles, staff, hr)
    }

    #[tokio::test]
    async fn a_user_logs_in_with_their_password_only() {
        let (roles, staff, hr) = roles();
        let alice = UserId::new();
        let (_dir, path) = user_file(&format!("users:\n{}", entry(alice, "alice", "[staff, hr]", "wonderland")));
        let users = FileUserProvider::open(&path, roles).expect("a valid file");

        let found = users.authenticate("alice", &Credential::Password("wonderland")).await.unwrap().expect("her password");
        assert_eq!((found.id, found.roles), (alice, vec![staff, hr]), "her id, and her roles by their names");
        let shouted = users.authenticate("ALICE", &Credential::Password("wonderland")).await.unwrap().expect("her password");
        assert_eq!(shouted.id, alice, "a username is compared as a name");
        assert!(matches!(users.authenticate("alice", &Credential::Password("looking-glass")).await, Err(AuthError::InvalidCredentials)),
            "a wrong password ends the chain");
        assert!(users.authenticate("bob", &Credential::Password("wonderland")).await.unwrap().is_none(), "a name it does not know is not its own");
    }

    #[tokio::test]
    async fn a_trusted_frontend_is_answered_for_the_users_of_the_file_only() {
        let alice = UserId::new();
        let (_dir, path) = user_file(&format!("users:\n{}", entry(alice, "alice", "[]", "wonderland")));
        let users = FileUserProvider::open(&path, Roles::default()).expect("a valid file");
        assert_eq!(users.authenticate("alice", &Credential::Trusted).await.unwrap().map(|user| user.id), Some(alice));
        assert!(users.authenticate("bob", &Credential::Trusted).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn an_empty_file_knows_nobody() {
        let (_dir, path) = user_file("users: []\n");
        let users = FileUserProvider::open(&path, Roles::default()).expect("a valid file");
        assert!(users.authenticate("alice", &Credential::Password("x")).await.unwrap().is_none());
        assert!(users.find_user("alice").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn the_file_is_read_anew_at_every_question() {
        let alice = UserId::new();
        let bob = UserId::new();
        let (_dir, path) = user_file(&format!("users:\n{}", entry(alice, "alice", "[]", "a")));
        let users = FileUserProvider::open(&path, Roles::default()).expect("a valid file");
        assert_eq!(users.get_user(alice).await.unwrap().map(|user| user.username.to_string()), Some("alice".to_string()));

        fs::write(&path, format!("users:\n{}", entry(bob, "bob", "[]", "b"))).unwrap();
        assert!(users.get_user(alice).await.unwrap().is_none(), "removed, her token is not renewed");
        assert_eq!(users.find_user("bob").await.unwrap().map(|user| user.id), Some(bob), "added, he is known");

        fs::write(&path, "users: [").unwrap();
        assert!(matches!(users.find_user("bob").await, Err(AuthError::Backend(_))), "broken, it denies rather than knowing nobody");
        fs::remove_file(&path).unwrap();
        assert!(matches!(users.authenticate("bob", &Credential::Password("b")).await, Err(AuthError::Backend(_))), "and so when gone");
    }

    #[test]
    fn a_file_that_is_no_valid_user_file_is_refused_when_the_provider_is_made() {
        let (roles, _, _) = roles();
        let id = UserId::new();
        let refused = |content: String| {
            let (_dir, path) = user_file(&content);
            let error = FileUserProvider::open(&path, roles.clone()).map(|_| ()).expect_err("refused");
            let message = error.to_string();
            assert!(!message.contains(path.to_string_lossy().as_ref()), "no path in {message}");
            message
        };
        assert!(refused("users: [".to_string()).contains("invalid"), "no YAML");
        assert!(refused(format!("users:\n{}{}", entry(id, "alice", "[]", "a"), entry(UserId::new(), "Alice", "[]", "b"))).contains("username"),
            "a name given twice, as names are compared");
        assert!(refused(format!("users:\n{}{}", entry(id, "alice", "[]", "a"), entry(id, "bob", "[]", "b"))).contains("id"), "an id given twice");
        assert!(refused(format!("users:\n{}", entry(id, "alice", "[boss]", "a"))).contains("boss"), "a role not configured");
        assert!(refused(format!("users:\n  - id: {id}\n    username: alice\n    password: wonderland\n")).contains("password"),
            "a password in plain text is no field of it");
        let hashed = |hash: &str| refused(format!("users:\n  - id: {id}\n    username: alice\n    password_hash: \"{hash}\"\n"));
        let bcrypt = hashed("$2b$12$R9h/cIPz0gi.URNNX3kh2OPST9/PgBkqquzi.Ss7KIUgO2t0jWMUW");
        assert!(bcrypt.contains("Argon2") && !bcrypt.contains("R9h/cIPz0gi"), "no Argon2 hash, and the hash not quoted: {bcrypt}");
        let scrypt = hashed("$scrypt$ln=16,r=8,p=1$aM15713r3Xsvxbi31lqr1Q$nFNh2CVHVjNldFVKDHDlm4CbdRSCdEBsjjJxD+iCs5E");
        assert!(scrypt.contains("Argon2") && !scrypt.contains("aM15713r3X"), "a hash of another kind: {scrypt}");
        assert!(hashed("wonderland").contains("Argon2"), "a password where its hash belongs");

        let dir = tempfile::tempdir().unwrap();
        let missing = FileUserProvider::open(dir.path().join("users.yaml"), Roles::default());
        assert!(matches!(missing, Err(AuthError::Backend(_))), "a file that is not there is no empty one");
    }

    #[test]
    fn its_parameters_name_the_file_and_its_roles_must_be_configured() {
        let (roles, _, _) = roles();
        let (_dir, path) = user_file(&format!("users:\n{}", entry(UserId::new(), "alice", "[hr]", "a")));
        let config = FileUserConfig { path };
        assert!(config.provider(Some(&roles)).is_ok());
        assert!(config.provider(None).is_err_and(|e| e.to_string().contains(&RoleError::UnknownRole("hr".to_string()).to_string())),
            "roles named, but none configured");
    }

    #[tokio::test]
    async fn the_example_users_log_in_with_their_names_as_passwords() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples");
        let config = AppConfig::load(examples.join("config.yaml")).expect("examples/config.yaml is a valid configuration");
        let users = FileUserProvider::open(examples.join("users.yaml"), config.roles.unwrap_or_default())
            .expect("examples/users.yaml names the roles of examples/config.yaml");
        for name in ["alice", "bob", "user"] {
            let found = users.authenticate(name, &Credential::Password(name)).await;
            assert!(matches!(found, Ok(Some(ref user)) if user.username == name), "{name}: {found:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_file_others_may_read_or_change_is_exposed() {
        assert!(!exposed(0o600) && !exposed(0o640), "its owner's, read by its group at most");
        assert!(exposed(0o644) && exposed(0o660) && exposed(0o602));
    }
}
