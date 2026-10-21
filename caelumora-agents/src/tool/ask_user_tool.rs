//! The `ask_user_tool` component (issue 165): the tool `ask_user`, with which
//! an agent asks users of its channel and waits for their answers.
//!
//! The tool is the simplest one that asks: its arguments are its questions
//! and whom they are put to, and its result is the answers, which it gives
//! the model as they are. Asking, waiting and checking the answers are the
//! runtime's (see `ToolProvider::questions`). Its questions are never
//! sensitive: their answers go to the model.

use crate::prompt::NAMESPACE;
use crate::turn::Turn;
use caelumora_runtime::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::tool::{described, arguments, tool_text, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::prompt_catalogue::{context, PromptCatalogue, PromptError};
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

#[derive(Debug)]
pub struct AskUserToolProvider {
    /// What its tool set is for and its tools, as the prompt catalogue
    /// words them (issue 57).
    description: String,
    tools: Vec<Tool>,
}

impl AskUserToolProvider {
    /// The provider, its texts from `prompts`, told the limits of a call.
    ///
    /// # Errors
    /// `PromptError` if one of them cannot be had.
    pub fn new(prompts: &PromptCatalogue) -> Result<Self, PromptError> {
        let limits = context! { max_questions => MAX_QUESTIONS, max_choices => MAX_CHOICES, max_header_chars => MAX_HEADER_CHARS };
        let choice = ToolParameters::object(
            vec![
                ("label", ToolParameters::string()),
                ("description", ToolParameters::string()),
                ("recommended", ToolParameters::boolean()),
            ],
            vec!["label"],
        );
        let question = ToolParameters::object(
            vec![
                ("header", ToolParameters::string()),
                ("question", ToolParameters::string()),
                ("choices", ToolParameters::array(choice)),
                ("multi_select", ToolParameters::boolean()),
            ],
            vec!["header", "question"],
        );
        let tools = vec![described(prompts, NAMESPACE, ASK_USER_TOOL_KIND, Tool {
            name: ASK_USER_TOOL_NAME.to_string(),
            description: String::new(),
            parameters: ToolParameters::object(
                vec![
                    ("questions", ToolParameters::array(question)),
                    ("ask", ToolParameters::array(ToolParameters::string())),
                ],
                vec!["questions", "ask"],
            ),
            requires_approval: false,
            visibility: ToolVisibility::SHOWN,
            classification: None,
            presentation: QUESTIONS_PRESENTATION,
        }, &limits)?];
        Ok(Self { description: tool_text(prompts, NAMESPACE, ASK_USER_TOOL_KIND, "description", &limits)?, tools })
    }
}

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
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
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
        ComponentKind::new(ASK_USER_TOOL_KIND).provides::<dyn ToolProvider>().uses::<PromptCatalogue>(),
        |_descriptor, context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(AskUserToolProvider::new(context.service::<PromptCatalogue>()?)?));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::question::Response;

    #[test]
    fn its_arguments_are_its_questions_put_to_the_handles_named() {
        let tool = AskUserToolProvider::new(&testing::prompts()).unwrap();
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
        assert_eq!(AskUserToolProvider::new(&testing::prompts()).unwrap().tools().remove(0).presentation, QUESTIONS_PRESENTATION);
    }

    #[tokio::test]
    async fn its_result_is_the_answers_as_given() {
        let tool = AskUserToolProvider::new(&testing::prompts()).unwrap();
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
