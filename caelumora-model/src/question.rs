//! Questions a tool asks a person before it runs, and the answers it runs
//! with (issue 165): the contract between the tool that asks, the runtime
//! that waits for the answers, and the frontends that show the questions and
//! send the answers.
//!
//! The answers go to the tool alone, never into a history or a transcript:
//! what the model learns of them is what the tool puts into its result. A
//! tool may thus ask for what the model must not see - its questions are
//! then `sensitive`, which changes what is kept and who is asked. Neither
//! the questions nor whom they go to are the model's to choose, beyond what
//! the tool lets it fill in.

use crate::actor::ActorId;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The most questions one call asks.
pub const MAX_QUESTIONS: usize = 5;
/// The fewest choices a question with choices offers: yes and no.
pub const MIN_CHOICES: usize = 2;
/// The most choices a question offers; beyond them there is "Other".
pub const MAX_CHOICES: usize = 4;
/// The longest header of a question, in characters: it labels the question
/// where it is shown collapsed.
pub const MAX_HEADER_CHARS: usize = 12;

/// What a call asks before it runs: its questions, in order, whom they are
/// put to, and whether the answers are sensitive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Questions {
    pub questions: Vec<Question>,
    /// Whom the questions are put to; only their answer counts.
    pub ask: Vec<Addressee>,
    /// Whether the answers are for the tool alone: not kept, not logged, and
    /// the questions shown to no one but those asked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sensitive: bool,
}

/// One question. Its kind follows from its shape: without choices it is
/// answered with text, with choices by choosing one of them - or several
/// with `multi_select` - or by writing another answer. Yes or no is a choice
/// of two.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    /// A short label, at most `MAX_HEADER_CHARS` characters.
    pub header: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<Choice>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multi_select: bool,
}

/// An answer offered to a question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// What is shown and what the answer names; unique within the question.
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The choice the asker recommends, marked as such.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recommended: bool,
}

/// Whom questions are put to, as the tool names them; the runtime finds
/// who that is in the channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Addressee {
    /// A participant by their handle, as a model writes it - `@alice` or
    /// `alice`.
    Handle(String),
    /// The owner of the agent whose call it is - for a tool that decides
    /// itself whom it asks.
    Owner,
}

/// The answers a call runs with: who gave them, and one response per
/// question, in their order. Its `Debug` shows how many there are, never
/// what they say.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Answers {
    /// Who answered, set by the runtime from whom the answers came - never
    /// from what they say.
    pub answered_by: ActorId,
    pub answered_by_name: String,
    pub responses: Vec<Response>,
}

/// The response to one question. Its `Debug` shows its kind, never its
/// content.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    /// To a question with choices: the labels chosen, and another answer
    /// written instead of or besides them ("Other").
    Chosen {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        chosen: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        other: Option<String>,
    },
    /// To a question without choices.
    Text { text: String },
    /// Not answered: the person declined this question.
    Declined,
}

impl fmt::Debug for Answers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Answers")
            .field("answered_by", &self.answered_by)
            .field("responses", &self.responses.len())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Response::Chosen { .. } => "Chosen",
            Response::Text { .. } => "Text",
            Response::Declined => "Declined",
        })
    }
}

/// Why questions cannot be asked as they are, or responses do not answer
/// them. The messages are meant for whoever wrote them - the model, through
/// the tool, or a person - and never repeat a response.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuestionError {
    #[error("ask between 1 and {MAX_QUESTIONS} questions")]
    QuestionCount,
    #[error("name whom the questions are put to")]
    NoAddressee,
    #[error("an addressee's handle must not be empty")]
    EmptyHandle,
    #[error("question {0} needs a header of 1 to {MAX_HEADER_CHARS} characters")]
    Header(usize),
    #[error("question {0} has no text")]
    NoQuestion(usize),
    #[error("question {0} must offer no choices, or between {MIN_CHOICES} and {MAX_CHOICES}")]
    ChoiceCount(usize),
    #[error("the choices of question {0} need labels, each a different one")]
    ChoiceLabels(usize),
    #[error("question {0} allows several answers but offers no choices")]
    MultiSelectWithoutChoices(usize),
    #[error("give one response per question, {0} in all")]
    ResponseCount(usize),
    #[error("question {0} is answered with text, not by choosing")]
    NotAChoice(usize),
    #[error("question {0} is answered by choosing, not with text")]
    NotAText(usize),
    #[error("the answer to question {0} is empty")]
    EmptyResponse(usize),
    #[error("the answer to question {0} names a choice it does not offer, or one twice")]
    UnknownChoice(usize),
    #[error("question {0} allows only one choice")]
    SeveralChosen(usize),
}

impl Questions {
    /// Whether they can be asked as they are: 1 to `MAX_QUESTIONS`
    /// questions put to someone, each with a header and a text, and either
    /// no choices or `MIN_CHOICES` to `MAX_CHOICES` with distinct labels.
    /// Questions are counted from 1 in the error.
    pub fn check(&self) -> Result<(), QuestionError> {
        if self.questions.is_empty() || self.questions.len() > MAX_QUESTIONS {
            return Err(QuestionError::QuestionCount);
        }
        if self.ask.is_empty() {
            return Err(QuestionError::NoAddressee);
        }
        if self.ask.iter().any(|a| matches!(a, Addressee::Handle(handle) if handle.is_empty())) {
            return Err(QuestionError::EmptyHandle);
        }
        for (index, question) in self.questions.iter().enumerate() {
            let number = index + 1;
            let header = question.header.chars().count();
            if header == 0 || header > MAX_HEADER_CHARS {
                return Err(QuestionError::Header(number));
            }
            if question.question.is_empty() {
                return Err(QuestionError::NoQuestion(number));
            }
            if question.choices.is_empty() {
                if question.multi_select {
                    return Err(QuestionError::MultiSelectWithoutChoices(number));
                }
                continue;
            }
            if !(MIN_CHOICES..=MAX_CHOICES).contains(&question.choices.len()) {
                return Err(QuestionError::ChoiceCount(number));
            }
            let labels: Vec<&str> = question.choices.iter().map(|c| c.label.as_str()).collect();
            if labels.iter().any(|l| l.is_empty()) || has_duplicates(&labels) {
                return Err(QuestionError::ChoiceLabels(number));
            }
        }
        Ok(())
    }

    /// Whether `responses` answer the questions: one per question, in
    /// their order, each of the kind its question asks - text for one
    /// without choices; for one with choices labels it offers, each once,
    /// only one without `multi_select`, and at least a label or another
    /// answer - or declined. Nothing given may be empty; nothing is trimmed.
    pub fn check_responses(&self, responses: &[Response]) -> Result<(), QuestionError> {
        if responses.len() != self.questions.len() {
            return Err(QuestionError::ResponseCount(self.questions.len()));
        }
        for (index, (question, response)) in self.questions.iter().zip(responses).enumerate() {
            let number = index + 1;
            match response {
                Response::Declined => {}
                Response::Text { .. } if !question.choices.is_empty() => return Err(QuestionError::NotAText(number)),
                Response::Text { text } => {
                    if text.is_empty() {
                        return Err(QuestionError::EmptyResponse(number));
                    }
                }
                Response::Chosen { .. } if question.choices.is_empty() => return Err(QuestionError::NotAChoice(number)),
                Response::Chosen { chosen, other } => {
                    if other.as_deref() == Some("") || (chosen.is_empty() && other.is_none()) {
                        return Err(QuestionError::EmptyResponse(number));
                    }
                    let labels: Vec<&str> = chosen.iter().map(String::as_str).collect();
                    if has_duplicates(&labels) || !labels.iter().all(|l| question.choices.iter().any(|c| c.label == *l)) {
                        return Err(QuestionError::UnknownChoice(number));
                    }
                    if !question.multi_select && chosen.len() > 1 {
                        return Err(QuestionError::SeveralChosen(number));
                    }
                }
            }
        }
        Ok(())
    }
}

fn has_duplicates(labels: &[&str]) -> bool {
    labels.iter().enumerate().any(|(i, label)| labels[..i].contains(label))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(label: &str) -> Choice {
        Choice { label: label.to_string(), description: None, recommended: false }
    }

    fn question(choices: &[&str], multi_select: bool) -> Question {
        Question { header: "Database".to_string(), question: "Which one?".to_string(), choices: choices.iter().map(|l| choice(l)).collect(), multi_select }
    }

    fn questions(questions: Vec<Question>) -> Questions {
        Questions { questions, ask: vec![Addressee::Handle("alice".to_string())], sensitive: false }
    }

    #[test]
    fn questions_are_asked_only_within_their_limits_and_of_someone() {
        assert_eq!(questions(vec![question(&[], false), question(&["yes", "no"], false), question(&["a", "b", "c", "d"], true)]).check(), Ok(()));

        assert_eq!(questions(vec![]).check(), Err(QuestionError::QuestionCount));
        assert_eq!(questions(vec![question(&[], false); MAX_QUESTIONS + 1]).check(), Err(QuestionError::QuestionCount));
        assert_eq!(Questions { ask: vec![], ..questions(vec![question(&[], false)]) }.check(), Err(QuestionError::NoAddressee));
        assert_eq!(Questions { ask: vec![Addressee::Handle(String::new())], ..questions(vec![question(&[], false)]) }.check(), Err(QuestionError::EmptyHandle));
        assert_eq!(Questions { ask: vec![Addressee::Owner], ..questions(vec![question(&[], false)]) }.check(), Ok(()));

        let long_header = Question { header: "Thirteen char".to_string(), ..question(&[], false) };
        assert_eq!(questions(vec![question(&[], false), long_header]).check(), Err(QuestionError::Header(2)), "counted from 1");
        assert_eq!(questions(vec![Question { header: "Zwölf Zeichn".to_string(), ..question(&[], false) }]).check(), Ok(()), "characters, not bytes");
        assert_eq!(questions(vec![Question { question: String::new(), ..question(&[], false) }]).check(), Err(QuestionError::NoQuestion(1)));
        assert_eq!(questions(vec![question(&["only"], false)]).check(), Err(QuestionError::ChoiceCount(1)));
        assert_eq!(questions(vec![question(&["a", "b", "c", "d", "e"], false)]).check(), Err(QuestionError::ChoiceCount(1)));
        assert_eq!(questions(vec![question(&["a", "a"], false)]).check(), Err(QuestionError::ChoiceLabels(1)));
        assert_eq!(questions(vec![question(&["a", ""], false)]).check(), Err(QuestionError::ChoiceLabels(1)));
        assert_eq!(questions(vec![question(&[], true)]).check(), Err(QuestionError::MultiSelectWithoutChoices(1)));
    }

    #[test]
    fn responses_answer_questions_only_as_each_asks() {
        let asked = questions(vec![question(&[], false), question(&["yes", "no"], false), question(&["a", "b", "c"], true)]);
        let text = |t: &str| Response::Text { text: t.to_string() };
        let chosen = |labels: &[&str], other: Option<&str>| Response::Chosen { chosen: labels.iter().map(|l| l.to_string()).collect(), other: other.map(str::to_string) };

        assert_eq!(asked.check_responses(&[text("Postgres"), chosen(&["yes"], None), chosen(&["a", "c"], Some("and d"))]), Ok(()));
        assert_eq!(asked.check_responses(&[Response::Declined, chosen(&[], Some("maybe")), Response::Declined]), Ok(()), "declined, or only another answer");

        assert_eq!(asked.check_responses(&[text("Postgres")]), Err(QuestionError::ResponseCount(3)));
        assert_eq!(asked.check_responses(&[chosen(&["yes"], None), chosen(&["yes"], None), Response::Declined]), Err(QuestionError::NotAChoice(1)));
        assert_eq!(asked.check_responses(&[text("x"), text("yes"), Response::Declined]), Err(QuestionError::NotAText(2)));
        assert_eq!(asked.check_responses(&[text(""), chosen(&["yes"], None), Response::Declined]), Err(QuestionError::EmptyResponse(1)));
        assert_eq!(asked.check_responses(&[text(" "), chosen(&["yes"], None), Response::Declined]), Ok(()), "nothing is trimmed");
        assert_eq!(asked.check_responses(&[text("x"), chosen(&[], None), Response::Declined]), Err(QuestionError::EmptyResponse(2)));
        assert_eq!(asked.check_responses(&[text("x"), chosen(&["yes"], Some("")), Response::Declined]), Err(QuestionError::EmptyResponse(2)));
        assert_eq!(asked.check_responses(&[text("x"), chosen(&["perhaps"], None), Response::Declined]), Err(QuestionError::UnknownChoice(2)));
        assert_eq!(asked.check_responses(&[text("x"), chosen(&["yes"], None), chosen(&["a", "a"], None)]), Err(QuestionError::UnknownChoice(3)));
        assert_eq!(asked.check_responses(&[text("x"), chosen(&["yes", "no"], None), Response::Declined]), Err(QuestionError::SeveralChosen(2)));
    }

    #[test]
    fn answers_are_read_back_as_written_and_never_shown_by_debug() {
        let answers = Answers {
            answered_by: ActorId::None,
            answered_by_name: "alice".to_string(),
            responses: vec![
                Response::Text { text: "hunter2".to_string() },
                Response::Chosen { chosen: vec!["secret choice".to_string()], other: Some("secret other".to_string()) },
                Response::Declined,
            ],
        };
        let json = serde_json::to_value(&answers).unwrap();
        assert_eq!(json["responses"][0], serde_json::json!({"kind": "text", "text": "hunter2"}));
        assert_eq!(json["responses"][2], serde_json::json!({"kind": "declined"}));
        assert_eq!(serde_json::from_value::<Answers>(json).unwrap(), answers);

        let shown = format!("{answers:?} {:?}", answers.responses);
        assert!(!shown.contains("hunter2") && !shown.contains("secret"), "{shown}");
    }

    #[test]
    fn questions_are_read_back_as_written() {
        let asked = Questions {
            questions: vec![Question { choices: vec![Choice { recommended: true, description: Some("the usual".to_string()), ..choice("yes") }, choice("no")], ..question(&[], false) }],
            ask: vec![Addressee::Handle("@alice".to_string()), Addressee::Owner],
            sensitive: true,
        };
        let json = serde_json::to_value(&asked).unwrap();
        assert_eq!(json["ask"], serde_json::json!([{"handle": "@alice"}, "owner"]));
        assert_eq!(serde_json::from_value::<Questions>(json).unwrap(), asked);
    }
}
