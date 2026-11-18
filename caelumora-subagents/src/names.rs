//! The names of sub-agents (issue 558): an adjective and the handle of the personality -
//! `curious-researcher`, `steady-reviewer` -, never one their channel has had, among its
//! participants or its former ones, so that its history never shows two sub-agents under one name.
//! No names of real people.

use caelumora_domain::value::name::Name;
use rand::seq::IndexedRandom;

/// The adjectives a name is drawn from: of a manner of working, none of a person.
const ADJECTIVES: [&str; 60] = [
    "agile", "alert", "ardent", "astute", "brave", "bright", "brisk", "calm", "candid", "careful",
    "clever", "cool", "curious", "daring", "deft", "diligent", "eager", "earnest", "exact", "fair",
    "fearless", "fleet", "focused", "frank", "gentle", "glad", "honest", "humble", "keen", "kind",
    "lively", "loyal", "lucid", "mellow", "merry", "mindful", "modest", "neat", "nimble", "noble",
    "patient", "plucky", "polite", "prompt", "proud", "quick", "quiet", "ready", "sharp",
    "sincere", "skilled", "sober", "steady", "sturdy", "subtle", "swift", "tidy", "witty", "wise",
    "zealous",
];

/// How often a name is drawn before a number is added to make it one the channel has not had.
const DRAWS: usize = 20;

/// A name for a sub-agent of the personality `personality`, none of which `taken` holds: an
/// adjective and the personality's handle, drawn - with a number added after `DRAWS` draws.
pub(crate) fn draw(personality: &Name, taken: impl Fn(&Name) -> bool) -> Name {
    let handle = personality.handle().unwrap_or_else(|| "agent".to_string());
    let mut rng = rand::rng();
    let named = |text: String| Name::new(text).ok().filter(|name| !taken(name));
    for _ in 0..DRAWS {
        let adjective = ADJECTIVES.choose(&mut rng).copied().unwrap_or("steady");
        if let Some(name) = named(format!("{adjective}-{handle}")) {
            return name;
        }
    }
    (2..)
        .find_map(|number| named(format!("{handle}-{number}")))
        .unwrap_or_else(|| personality.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue 558: a name is an adjective and the personality's handle, one the channel has not had;
    /// once every adjective is taken, a number makes one.
    #[test]
    fn a_name_is_an_adjective_and_the_personality_never_one_taken() {
        let personality = Name::new("Researcher").expect("a name");
        let name = draw(&personality, |_| false);
        let (adjective, handle) = name.as_str().split_once('-').expect("an adjective and a handle");
        assert!(ADJECTIVES.contains(&adjective), "{name}");
        assert_eq!(handle, "researcher", "the personality's handle");

        let taken = |name: &Name| name.as_str() != "researcher-3";
        assert_eq!(
            draw(&personality, taken).as_str(),
            "researcher-3",
            "a number, once all are taken"
        );
    }
}
