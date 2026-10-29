//! The logged-in user's profile (issue 176) and their answers given in advance (issue 13).

use caelumora_model::approval::{ApprovalRule, Reach, RuleDecision, ToolMatcher};
use caelumora_model::user::UserProfile;
use caelumora_model::value::language::Language;
use caelumora_model::value::text::Introduction;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The logged-in user's profile (issue 176): how they introduce themselves
/// to the agents of every channel they take part in, and how long that may
/// be.
#[derive(Debug, Serialize)]
pub struct ProfileResponse {
    pub introduction: Option<Introduction>,
    pub max_introduction_chars: usize,
    /// The language the user prefers (issue 56), a BCP 47 tag such as `de`.
    pub preferred_language: Option<Language>,
    /// The user's answers to tool calls given in advance (issue 13): rules
    /// that allow or deny, each within a reach.
    pub approval_rules: Vec<ApprovalRule>,
}

impl ProfileResponse {
    /// `profile` as answered; one without anything in it if there is none.
    pub fn of(profile: Option<UserProfile>) -> Self {
        let approval_rules =
            profile.as_ref().map_or_else(Vec::new, |profile| profile.approval_rules().to_vec());
        let (introduction, preferred_language) = profile
            .map_or((None, None), |profile| (profile.introduction, profile.preferred_language));
        Self {
            introduction,
            max_introduction_chars: Introduction::MAX_CHARS,
            preferred_language,
            approval_rules,
        }
    }
}

/// A rule a user adds to their answers given in advance (issue 13): what
/// calls it is about, whether it allows or denies them, where - everywhere
/// unless named - and until when. Its id is made by the server.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRuleRequest {
    pub matcher: ToolMatcher,
    pub decision: RuleDecision,
    #[serde(default)]
    pub reach: Reach,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

/// The logged-in user's profile as it is to be: their introduction, `null`
/// or empty for none, and the language they prefer, `null` for none - both
/// replaced.
#[derive(Debug, Deserialize)]
pub struct ProfileRequest {
    #[serde(default)]
    pub introduction: Option<String>,
    /// A language tag, made into a language by the profile (issue 248).
    #[serde(default)]
    pub preferred_language: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::CreateChannelRequest;

    #[test]
    fn a_language_is_passed_on_as_it_was_sent() {
        let request: ProfileRequest =
            serde_json::from_str(r#"{"introduction": null, "preferred_language": "de_ch"}"#)
                .unwrap();
        assert_eq!(request.preferred_language, Some("de_ch".to_string()));
        let request: ProfileRequest =
            serde_json::from_str(r#"{"preferred_language": "Deutsch"}"#).unwrap();
        assert_eq!(
            request.preferred_language.as_deref(),
            Some("Deutsch"),
            "checked by the profile, not as the request is read"
        );
        let request: CreateChannelRequest = serde_json::from_str(r#"{"name": "side"}"#).unwrap();
        assert_eq!(
            request.language, None,
            "none when left out, whichever the user prefers (issue 331)"
        );
    }
}
