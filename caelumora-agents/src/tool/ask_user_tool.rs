//! The `ask_user_tool` component (issue 165): the tool `ask_user`, with which
//! an agent asks users of its channel and waits for their answers.
//!
//! The tool is the simplest one that asks: its arguments are its questions
//! and whom they are put to, and its result is the answers, which it gives
//! the model as they are. Asking, waiting and checking the answers are the
//! runtime's (see `ToolProvider::questions`). Its questions are never
//! sensitive: their answers go to the model.

use crate::turn::Turn;
use caelumora_runtime::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::tool::{arguments, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::question::{Addressee, Answers, Question, Questions, MAX_CHOICES, MAX_HEADER_CHARS, MAX_QUESTIONS};
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// Questions put to users (issue 268): its `questions` as they were
/// put, and whom they wait for (`ask`); once answered, nothing beyond
/// the record of the answers every call that asked shows.
pub const QUESTIONS_PRESENTATION: ToolPresentation = ToolPresentation::new("questions");

pub const ASK_USER_TOOL_KIND: &str = "ask_user_tool";

pub const ASK_USER_TOOL_NAME: &str = "ask_user";

#[derive(Debug, Default)]
pub struct AskUserToolProvider;

/// The arguments of `ask_user`: the questions, and the handles of the users
/// to ask.
#[derive(Deserialize)]
struct AskUser {
    questions: Vec<Question>,
    ask: Vec<String>,
}

#[async_trait]
impl ToolProvider for AskUserToolProvider {
    fn description(&self) -> String {
        "Ask users of the channel and wait for their answers.".to_string()
    }

    fn tools(&self) -> Vec<Tool> {
        let choice = ToolParameters::object(
            vec![
                ("label", ToolParameters::string("The answer as the user chooses it; unique within the question")),
                ("description", ToolParameters::string("What choosing it means, if the label does not say")),
                ("recommended", ToolParameters::boolean("Whether you recommend this choice; mark at most one")),
            ],
            vec!["label"],
        );
        let question = ToolParameters::object(
            vec![
                ("header", ToolParameters::string(format!("A short label for the question, at most {MAX_HEADER_CHARS} characters"))),
                ("question", ToolParameters::string("The question, without its possible answers")),
                ("choices", ToolParameters::array(choice, format!("The possible answers, 2 to {MAX_CHOICES}; leave out for a free-text answer"))),
                ("multi_select", ToolParameters::boolean("Whether several choices may be chosen")),
            ],
            vec!["header", "question"],
        );
        vec![Tool {
            name: ASK_USER_TOOL_NAME.to_string(),
            description: format!(
                "Asks users of the channel one or more questions and waits for their answer, then goes on with it. \
                 Use it when you need a decision or information from a user before you can go on. \
                 Put the possible answers of a question into `choices`, never into the question text: \
                 the user can only choose what is in `choices`, and may always write another answer instead. \
                 For yes or no, offer the two as choices. \
                 Ask questions that do not depend on each other together, at most {MAX_QUESTIONS} in one call; \
                 ask a question whose answer changes another in a call of its own. \
                 Name in `ask` the users to ask by their handles, e.g. `@alice`, or `@all` for every user here; only their answer counts. \
                 The result gives the answers in the order of the questions; a question the user did not answer is `declined`. \
                 If the user declines the whole call, or writes a message instead, the call is answered as declined: \
                 go on with what they wrote."
            ),
            parameters: ToolParameters::object(
                vec![
                    ("questions", ToolParameters::array(question, format!("The questions, 1 to {MAX_QUESTIONS}"))),
                    ("ask", ToolParameters::array(ToolParameters::string("A handle"), "The users to ask, by their handles; at least one")),
                ],
                vec!["questions", "ask"],
            ),
            requires_approval: false,
            visibility: ToolVisibility::SHOWN,
            classification: None,
            presentation: QUESTIONS_PRESENTATION,
        }]
    }

    async fn execute(&self, name: &str, _params: &Value, answers: Option<&Answers>, _turn: &mut Turn) -> ToolResult<ToolOutput> {
        if name != ASK_USER_TOOL_NAME {
            return Err(ToolError::UnknownTool(name.to_string()));
        }
        // The runtime runs it only with its answers.
        let answers = answers.ok_or_else(|| ToolError::ExecutionFailed("it ran without the answers to its questions".to_string()))?;
        // What users wrote: classified as the channel, as configured for none.
        Ok(serde_json::json!({ "answered_by": answers.answered_by_name, "responses": answers.responses }).into())
    }

    fn questions(&self, name: &str, params: &Value) -> ToolResult<Option<Questions>> {
        if name != ASK_USER_TOOL_NAME {
            return Err(ToolError::UnknownTool(name.to_string()));
        }
        let AskUser { questions, ask } = arguments(params)?;
        Ok(Some(Questions { questions, ask: ask.into_iter().map(Addressee::Handle).collect(), sensitive: false }))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(ASK_USER_TOOL_KIND).provides::<dyn ToolProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(AskUserToolProvider));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::question::Response;

    #[test]
    fn its_arguments_are_its_questions_put_to_the_handles_named() {
        let tool = AskUserToolProvider;
        let params = serde_json::json!({
            "questions": [
                { "header": "Database", "question": "Which one?", "choices": [{ "label": "Postgres", "recommended": true }, { "label": "SQLite" }] },
                { "header": "Name", "question": "What should it be called?" },
            ],
            "ask": ["@alice", "bob"],
        });
        let questions = tool.questions(ASK_USER_TOOL_NAME, &params).unwrap().expect("it asks");
        assert_eq!(questions.check(), Ok(()));
        assert_eq!(questions.ask, [Addressee::Handle("@alice".to_string()), Addressee::Handle("bob".to_string())]);
        assert!(!questions.sensitive, "the answers go to the model");
        assert!(questions.questions[0].choices[0].recommended && questions.questions[1].choices.is_empty());

        let without_ask = serde_json::json!({ "questions": [{ "header": "Name", "question": "Who?" }] });
        assert!(matches!(tool.questions(ASK_USER_TOOL_NAME, &without_ask), Err(ToolError::InvalidParameter(_))), "`ask` is required");
        assert!(matches!(tool.questions("other", &params), Err(ToolError::UnknownTool(_))));
    }

    #[test]
    fn its_calls_are_presented_as_questions() {
        assert_eq!(AskUserToolProvider.tools().remove(0).presentation, QUESTIONS_PRESENTATION);
    }

    #[tokio::test]
    async fn its_result_is_the_answers_as_given() {
        let tool = AskUserToolProvider;
        let mut state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut state);
        let params = serde_json::json!({ "questions": [{ "header": "Name", "question": "Who?" }, { "header": "Ok", "question": "Go on?", "choices": [{ "label": "yes" }, { "label": "no" }] }], "ask": ["@alice"] });
        let questions = tool.questions(ASK_USER_TOOL_NAME, &params).unwrap().expect("it asks");
        let answers = Answers {
            questions,
            answered_by: ActorId::None,
            answered_by_name: "alice".to_string(),
            responses: vec![Response::Text { text: "Kaya".to_string() }, Response::Chosen { chosen: vec!["yes".to_string()], other: None }],
        };

        let output = tool.execute(ASK_USER_TOOL_NAME, &params, Some(&answers), &mut turn).await.unwrap();
        assert_eq!(output.value, serde_json::json!({
            "answered_by": "alice",
            "responses": [{ "kind": "text", "text": "Kaya" }, { "kind": "chosen", "chosen": ["yes"] }],
        }));
        assert!(output.classification.is_none(), "classified as the channel, like what users write");
        assert!(matches!(tool.execute(ASK_USER_TOOL_NAME, &params, None, &mut turn).await, Err(ToolError::ExecutionFailed(_))));
    }
}
