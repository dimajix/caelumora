//! What names a secret (issue 310): `SecretRef`, its vault and its key, written `vault/KEY` - a
//! variant of the texts.

use super::identifier::Identifier;
use super::ValueError;

/// What a reference looks like, in the error for one that does not.
const EXPECTED: &str = "a vault and a key, written vault/KEY";

/// A secret as a configuration names it (issue 310): the vault it lies in - a name the
/// configuration gives, an `Identifier` - and its key there, written `vault/KEY`. A key alone is
/// no reference: which vault answers is never left to a search. Written down as a string, and
/// checked when read.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SecretRef {
    text: String,
    /// Where the `/` between the vault and the key is.
    slash: usize,
}

impl SecretRef {
    /// The vault of the principal - for an agent its owner's (issue 343) -, which no configuration
    /// may name.
    pub const USER_VAULT: &'static str = "user";

    /// `text` as a reference: a vault, a `/` and a key, neither empty.
    ///
    /// # Errors
    /// `ValueError` if it is empty, has no `/`, names no vault that is an `Identifier`, or no key.
    pub fn new(text: impl AsRef<str>) -> Result<Self, ValueError> {
        let text = text.as_ref();
        if text.is_empty() {
            return Err(ValueError::Empty { field: Self::FIELD });
        }
        let malformed = ValueError::Malformed { field: Self::FIELD, expected: EXPECTED };
        let Some((vault, key)) = text.split_once('/') else {
            return Err(malformed);
        };
        if Identifier::new(vault).is_err() || key.is_empty() {
            return Err(malformed);
        }
        Ok(Self { text: text.to_string(), slash: vault.len() })
    }

    /// The secret `key` of `vault`, a reference the code itself gives, such as the default of the
    /// signing key: taken unchecked, so that it needs no error that cannot happen; each is checked
    /// by a test instead.
    pub(crate) fn fixed(vault: &'static str, key: &'static str) -> Self {
        Self { text: format!("{vault}/{key}"), slash: vault.len() }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The name of the vault it lies in.
    pub fn vault(&self) -> &str {
        &self.text[..self.slash]
    }

    /// Its key in that vault.
    pub fn key(&self) -> &str {
        &self.text[self.slash + 1..]
    }
}

impl_value!([] SecretRef, "secret");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_names_its_vault_and_its_key() {
        let secret = SecretRef::new("local/GITHUB_TOKEN").unwrap();
        assert_eq!((secret.vault(), secret.key()), ("local", "GITHUB_TOKEN"));
        let nested = SecretRef::new("team/github/token").unwrap();
        assert_eq!((nested.vault(), nested.key()), ("team", "github/token"), "the first `/`");
        assert_eq!(nested.to_string(), "team/github/token", "shown as written");
    }

    #[test]
    fn a_key_alone_or_a_vault_alone_is_no_reference() {
        assert!(matches!(SecretRef::new(""), Err(ValueError::Empty { .. })));
        for text in ["GITHUB_TOKEN", "local/", "/GITHUB_TOKEN", "Local/KEY", "my vault/KEY"] {
            assert!(matches!(SecretRef::new(text), Err(ValueError::Malformed { .. })), "{text}");
        }
        let read = serde_json::from_value::<SecretRef>(serde_json::json!("GITHUB_TOKEN"));
        assert!(read.unwrap_err().to_string().contains("vault/KEY"), "refused when read");
    }
}
