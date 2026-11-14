//! What the terminal asks the user to decide now (issue 571): an approval pending in the channel
//! talked in, or the next of the questions put to them, with what the choices they answer it by
//! mean - which both ways of drawing word alike (`screen/words.rs`), each styling them its own
//! way.

use caelumora_domain::approval::{ApprovalRequest, Reach, ToolMatcher};
use caelumora_domain::question::{Choice, Question, QuestionRequest, Response};

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

/// What a choice offered for what is asked means (`Asking::choices`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Offered {
    /// The call approved.
    Approve,
    /// The call approved, and the calls `matcher` is about - those alike, as the call's tool
    /// suggests - approved from now on within `reach`, by a rule of the user's.
    ApproveFromNowOn {
        /// The calls alike.
        matcher: ToolMatcher,
        /// Where the rule applies: the call's channel, or its session.
        reach: Reach,
    },
    /// The call denied.
    Deny,
    /// A choice of the question asked.
    Choice(Choice),
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

    /// The choices it is answered by, by what they mean: for an approval, the call approved - and,
    /// where its tool suggests the calls alike and no rule of a place asked for a person's own
    /// decision, those approved from now on in its channel or its session -, or denied, as the web
    /// UI offers them; for a question, its choices - none for one answered in words.
    pub(crate) fn choices(&self) -> Vec<Offered> {
        match self {
            Asking::Approval(request) => {
                let mut offered = vec![Offered::Approve];
                if let Some(alike) = request.suggested.as_ref().filter(|_| !request.explicit) {
                    for reach in
                        [Reach::Channel(request.channel_id), Reach::Session(request.session_id)]
                    {
                        offered.push(Offered::ApproveFromNowOn { matcher: alike.clone(), reach });
                    }
                }
                offered.push(Offered::Deny);
                offered
            }
            Asking::Question { .. } => self
                .question()
                .map(|(_, question)| {
                    question.choices.iter().cloned().map(Offered::Choice).collect()
                })
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
    use caelumora_domain::approval::testing::make_approval_request;
    use caelumora_domain::channel::ChannelId;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::question::Questions;
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
        let header = |asking: &Asking| asking.question().map(|(at, q)| (at, q.header.clone()));
        assert_eq!(header(&asking), Some((0, "Store".to_string())));
        let labels: Vec<String> = asking
            .choices()
            .into_iter()
            .map(|offered| match offered {
                Offered::Choice(choice) => choice.label,
                other => panic!("no choice of a question: {other:?}"),
            })
            .collect();
        assert_eq!(labels, ["Postgres", "SQLite"]);
        assert!(!asking.multi_select() && asking.takes_words());
        let Asking::Question { request, .. } = asking else { unreachable!("questions") };
        let next = Asking::Question { request, responses: vec![Response::Declined] };
        assert_eq!(header(&next), Some((1, "Name".to_string())));
        assert!(next.choices().is_empty(), "answered in words");
    }

    /// An approval is answered by yes or no - and, where its tool suggests the calls alike and no
    /// rule of a place asked for a person's own decision, by yes from now on in its channel or its
    /// session.
    #[test]
    fn an_approval_offers_the_calls_alike_from_now_on_where_suggested() {
        let request = make_approval_request(SessionId::new(), ChannelId::new(), AgentId::new());
        let plain = Asking::Approval(request.clone());
        assert_eq!(plain.choices(), [Offered::Approve, Offered::Deny], "none suggested");
        let matcher = ToolMatcher { tool: "test_tool".to_string(), arguments: Default::default() };
        let suggested = ApprovalRequest { suggested: Some(matcher.clone()), ..request };
        let alike = |reach| Offered::ApproveFromNowOn { matcher: matcher.clone(), reach };
        assert_eq!(
            Asking::Approval(suggested.clone()).choices(),
            [
                Offered::Approve,
                alike(Reach::Channel(suggested.channel_id)),
                alike(Reach::Session(suggested.session_id)),
                Offered::Deny,
            ]
        );
        let explicit = ApprovalRequest { explicit: true, ..suggested };
        assert_eq!(Asking::Approval(explicit).choices(), [Offered::Approve, Offered::Deny]);
    }
}
