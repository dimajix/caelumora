//! Names read back as `&'static str` (issue 323): the name of an
//! `EntityKind` and of a custom `EntityChange` are constants of the code
//! that defines them, and an event that comes from another instance (issue
//! 122) names them as text. Each name read is kept once, for the rest of
//! the process, so that what is read compares and matches as the constant
//! does - and a name no code here defines is one nothing waits for.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};

/// The names read so far.
static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();

/// `name` as a `&'static str`: the one kept for it, or a new one kept from
/// now on. A set of names that grows only with names never read before -
/// the kinds and changes the instances of a deployment define.
pub(crate) fn intern(name: &str) -> &'static str {
    let mut names =
        NAMES.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(kept) = names.get(name) {
        return kept;
    }
    let kept: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(kept);
    kept
}

#[cfg(test)]
mod tests {
    use super::intern;
    use std::ptr;

    #[test]
    fn a_name_is_kept_once() {
        let first = intern("a_kind_of_this_test");
        let again = intern(&String::from("a_kind_of_this_test"));
        assert!(ptr::eq(first, again));
        assert_eq!(first, "a_kind_of_this_test");
    }
}
