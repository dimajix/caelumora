//! What the terminal asks the user to decide now (issue 571): an approval pending in the channel
//! talked in, or the next of the questions put to them, with the choices they answer it by - which
//! each way of drawing shows its own way.

use crate::screen::{choice_words, question_line};
use caelumora_domain::approval::ApprovalRequest;
use caelumora_domain::question::{Question, QuestionRequest, Response};

/// What the terminal asks the user to decide now.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Asking {
    /// An approval pending in the channel talked in.
    Approval(ApprovalRequest),
    /// The next of the questions of `request` put to the user, `responses` given to those before.
    Question {
        /// The questions.
        request: QuestionRequest,
        /// The responses given so far, one per question before the next.
        responses: Vec<Response>,
    },
}

/// What is asked of the user, as the terminal holds it: shown, or put aside to type on.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Asked {
    /// Shown, and answered by a choice or a line typed.
    Shown(Asking),
    /// Put aside: shown as waiting only, and no line typed an answer to it.
    Aside(Asking),
}

impl Asked {
    /// What is asked.
    pub(crate) fn asking(&self) -> &Asking {
        match self {
            Asked::Shown(asking) | Asked::Aside(asking) => asking,
        }
    }

    /// What is asked, where it is shown; `None` where it is put aside.
    pub(crate) fn shown(&self) -> Option<&Asking> {
        match self {
            Asked::Shown(asking) => Some(asking),
            Asked::Aside(_) => None,
        }
    }

    /// It, put aside.
    pub(crate) fn put_aside(self) -> Self {
        Asked::Aside(self.into_asking())
    }

    /// It, taken up again.
    pub(crate) fn taken_up(self) -> Self {
        Asked::Shown(self.into_asking())
    }

    /// What is asked, taken out.
    pub(crate) fn into_asking(self) -> Asking {
        match self {
            Asked::Shown(asking) | Asked::Aside(asking) => asking,
        }
    }
}

impl Asking {
    /// Questions `request` put to the user, the first of them asked.
    pub(crate) fn questions(request: QuestionRequest) -> Self {
        Asking::Question { request, responses: Vec::new() }
    }

    /// What is asked, in words: the call to be approved, or the next question with its number.
    pub(crate) fn what(&self) -> String {
        match self {
            Asking::Approval(request) => format!("Approve? {}", request.description),
            Asking::Question { .. } => {
                self.question().map(|(index, q)| question_line(index, q)).unwrap_or_default()
            }
        }
    }

    /// The choices it is answered by, in words: for an approval "yes" and "no"; for a question
    /// its choices, recommended or described where they are - none for one answered in words.
    pub(crate) fn choices(&self) -> Vec<String> {
        match self {
            Asking::Approval(_) => vec!["Yes".to_string(), "No".to_string()],
            Asking::Question { .. } => self
                .question()
                .map(|(_, question)| question.choices.iter().map(choice_words).collect())
                .unwrap_or_default(),
        }
    }

    /// Whether several of its choices may be taken together.
    pub(crate) fn multi_select(&self) -> bool {
        self.question().is_some_and(|(_, question)| question.multi_select)
    }

    /// Whether it may be answered in words of one's own besides its choices: a question may.
    pub(crate) fn takes_words(&self) -> bool {
        matches!(self, Asking::Question { .. })
    }

    /// The question asked, with its index among those of its request; `None` for an approval.
    pub(crate) fn question(&self) -> Option<(usize, &Question)> {
        match self {
            Asking::Approval(_) => None,
            Asking::Question { request, responses } => {
                let index = responses.len();
                request.questions.questions.get(index).map(|question| (index, question))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::channel::ChannelId;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::question::{Choice, Questions};
    use caelumora_domain::session::SessionId;
    use caelumora_domain::user::UserId;

    fn request(questions: Vec<Question>) -> QuestionRequest {
        let me = ActorId::User(UserId::new());
        let request = make_question_request(SessionId::new(), ChannelId::new(), AgentId::new(), me);
        QuestionRequest { questions: Questions { questions, ..request.questions }, ..request }
    }

    fn question(header: &str, choices: &[&str], multi_select: bool) -> Question {
        Question {
            header: header.to_string(),
            question: "Which?".to_string(),
            choices: choices
                .iter()
                .map(|label| Choice {
                    label: label.to_string(),
                    description: None,
                    recommended: *label == "SQLite",
                })
                .collect(),
            multi_select,
        }
    }

    /// Questions are asked one after the other, each with its choices, the next once the one
    /// before is answered.
    #[test]
    fn questions_are_asked_one_after_the_other() {
        let questions =
            vec![question("Store", &["Postgres", "SQLite"], false), question("Name", &[], false)];
        let asking = Asking::questions(request(questions));
        assert_eq!(asking.what(), "1. Store: Which?");
        assert_eq!(asking.choices(), ["Postgres", "SQLite (recommended)"]);
        assert!(!asking.multi_select() && asking.takes_words());
        let Asking::Question { request, .. } = asking else { unreachable!("questions") };
        let next = Asking::Question { request, responses: vec![Response::Declined] };
        assert_eq!(next.what(), "2. Name: Which?");
        assert!(next.choices().is_empty(), "answered in words");
    }
}
