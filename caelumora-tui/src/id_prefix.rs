//! What the terminal names by the beginning of its id (issue 571) - a session, an approval,
//! questions -, since no id is typed in full: the one alone that begins so, or why there is none.

/// Why the beginning of an id given names none of what is looked among (`beginning_with`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotNamed {
    /// No beginning was given.
    Blank,
    /// None begins with it, the beginning given.
    None(String),
    /// Several begin with it, the beginning given.
    Several(String),
}

impl NotNamed {
    /// What tells why, of what is looked among, `among` - in the plural.
    pub(crate) fn told(&self, among: &str) -> String {
        match self {
            NotNamed::Blank => format!("None of the {among} named: give the beginning of its id"),
            NotNamed::None(prefix) => format!("None of the {among} begins with '{prefix}'"),
            NotNamed::Several(prefix) => format!("Several of the {among} begin with '{prefix}'"),
        }
    }
}

/// The one of `items` whose id - as `id_of` writes it - begins with `prefix`.
///
/// # Errors
/// If `prefix` is blank, none of them begins so, or several do.
pub(crate) fn beginning_with<T>(
    items: impl IntoIterator<Item = T>,
    id_of: impl Fn(&T) -> String,
    prefix: &str,
) -> Result<T, NotNamed> {
    let prefix = prefix.trim().to_lowercase();
    if prefix.is_empty() {
        return Err(NotNamed::Blank);
    }
    let mut found = items.into_iter().filter(|item| id_of(item).starts_with(&prefix));
    match (found.next(), found.next()) {
        (Some(item), None) => Ok(item),
        (Some(_), Some(_)) => Err(NotNamed::Several(prefix)),
        (None, _) => Err(NotNamed::None(prefix)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the terminal names by the beginning of an id is the one alone that begins so, whatever
    /// the case of what is typed.
    #[test]
    fn the_one_beginning_so_is_named() {
        let ids = ["ab12", "ab34", "cd56"];
        let named = |prefix| beginning_with(ids, |id| id.to_string(), prefix);
        assert_eq!(named(" AB1 "), Ok("ab12"));
        assert_eq!(named("ab"), Err(NotNamed::Several("ab".to_string())));
        assert_eq!(named("ef"), Err(NotNamed::None("ef".to_string())));
        assert_eq!(named(" "), Err(NotNamed::Blank));
    }
}
