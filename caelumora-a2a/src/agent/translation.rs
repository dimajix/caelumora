//! Between the wire and the facet (`caelumora_runtime::external`, issue
//! 210): what is said, as the message sent to the remote agent; and what
//! the remote agent sends - a task, its status and its artifacts, or a
//! message of its own - as `ExternalEvent`s.

use super::wire::{text_of, StreamResponse, WireMessage, WirePart, WireRole, WireStatus, WireTask, WireTaskState};
use caelumora_model::external_agent::{ConversationId, TaskId};
use caelumora_runtime::external::{ExternalEvent, OutboundMessage};
use std::collections::HashMap;
use uuid::Uuid;

/// The media types of an answer that are taken: only text is read of it.
pub(super) const ACCEPTED_OUTPUT_MODES: [&str; 1] = ["text/plain"];

/// The message that gives the remote agent `message`, in `conversation` if
/// it opened one: its prompt parts first, as they are, then what each said,
/// as the remote agent's user - marked with its sender as the component's
/// profile says (issue 135), one part each - and the task it
/// answers, if it asked back. A2A has no system message (issue 204): the
/// prompt parts are text of the user's message too, set apart by the
/// heading each has.
pub(super) fn wire_message(message: &OutboundMessage, conversation: Option<&ConversationId>) -> WireMessage {
    WireMessage {
        message_id: Some(Uuid::new_v4().to_string()),
        role: Some(WireRole::User),
        context_id: conversation.map(|c| c.0.clone()),
        task_id: message.answers.as_ref().map(|t| t.0.clone()),
        parts: message.prompts.iter().map(|p| WirePart { text: Some(p.clone()) })
            // A2A has no sender of a part: the text names it (issue 135).
            .chain(message.said.iter().map(|said| WirePart { text: Some(said.text.clone()) }))
            .collect(),
    }
}

/// What a stream's responses become, as `ExternalEvent`s: keeps what it
/// told already - the conversation, the task - and the pieces of artifacts
/// sent in chunks.
#[derive(Debug, Default)]
pub(super) struct Translation {
    conversation: Option<String>,
    task: Option<String>,
    artifacts: HashMap<String, String>,
}

impl Translation {
    /// That `task`, which still works, was started.
    pub(super) fn started(&mut self, task: &WireTask) -> Vec<ExternalEvent> {
        let mut events = Vec::new();
        self.conversation(task.context_id.as_deref(), &mut events);
        self.task(&task.id, &mut events);
        events
    }

    /// What `response` tells that was not told before.
    pub(super) fn events(&mut self, response: StreamResponse) -> Vec<ExternalEvent> {
        let mut events = Vec::new();
        if let Some(message) = response.message {
            // An answer without a task: it is the whole of it.
            self.conversation(message.context_id.as_deref(), &mut events);
            let text = message.text();
            if !text.is_empty() {
                events.push(ExternalEvent::Message(text));
            }
            events.push(ExternalEvent::Completed);
        }
        if let Some(task) = response.task {
            self.conversation(task.context_id.as_deref(), &mut events);
            self.task(&task.id, &mut events);
            for artifact in task.artifacts {
                let text = text_of(&artifact.parts);
                if !text.is_empty() {
                    events.push(ExternalEvent::Message(text));
                }
            }
            status_events(task.status, &mut events);
        }
        if let Some(update) = response.status_update {
            self.conversation(update.context_id.as_deref(), &mut events);
            status_events(update.status, &mut events);
        }
        if let Some(update) = response.artifact_update {
            self.conversation(update.context_id.as_deref(), &mut events);
            let piece = text_of(&update.artifact.parts);
            let whole = self.artifacts.entry(update.artifact.artifact_id.clone()).or_default();
            if !update.append {
                whole.clear();
            }
            whole.push_str(&piece);
            if update.last_chunk {
                let whole = self.artifacts.remove(&update.artifact.artifact_id).unwrap_or_default();
                if !whole.is_empty() {
                    events.push(ExternalEvent::Message(whole));
                }
            } else if !piece.is_empty() {
                events.push(ExternalEvent::Partial(piece));
            }
        }
        events
    }

    /// The task, told once - and again if it changes.
    fn task(&mut self, task: &str, events: &mut Vec<ExternalEvent>) {
        if self.task.as_deref() != Some(task) {
            self.task = Some(task.to_string());
            events.push(ExternalEvent::Started(TaskId(task.to_string())));
        }
    }

    /// The conversation, told once - and again if it changes.
    fn conversation(&mut self, context: Option<&str>, events: &mut Vec<ExternalEvent>) {
        if let Some(context) = context.filter(|c| !c.is_empty()) {
            if self.conversation.as_deref() != Some(context) {
                self.conversation = Some(context.to_string());
                events.push(ExternalEvent::Conversation(ConversationId(context.to_string())));
            }
        }
    }
}

/// What a task's `status` tells: nothing while it works; its end, its
/// question, or why it failed.
fn status_events(status: WireStatus, events: &mut Vec<ExternalEvent>) {
    let text = status.message.as_ref().map(WireMessage::text).unwrap_or_default();
    let or = |fallback: &str| if text.is_empty() { fallback.to_string() } else { text.clone() };
    match status.state {
        WireTaskState::Completed => {
            if !text.is_empty() {
                events.push(ExternalEvent::Message(text.clone()));
            }
            events.push(ExternalEvent::Completed);
        }
        WireTaskState::Canceled => events.push(ExternalEvent::Canceled),
        WireTaskState::Failed => events.push(ExternalEvent::Failed(or("the task failed"))),
        WireTaskState::Rejected => events.push(ExternalEvent::Failed(or("the task was refused"))),
        WireTaskState::InputRequired => events.push(ExternalEvent::InputRequired(or("it needs more to go on"))),
        WireTaskState::AuthRequired => events.push(ExternalEvent::Failed("it asks for an authentication it was not given".to_string())),
        // Submitted, working: what it says meanwhile is not the answer.
        WireTaskState::Other => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn what_is_said_is_sent_as_the_users_message_one_part_each_as_it_is_given() {
        use caelumora_runtime::external::Said;
        let said = |text: &str| Said { sender: "alice".to_string(), text: text.to_string() };
        let first = OutboundMessage { prompts: Vec::new(), said: vec![said("alice: hello"), said("bob: hi")], answers: None };
        let sent = serde_json::to_value(wire_message(&first, None)).expect("json");
        assert!(sent["messageId"].as_str().is_some_and(|id| !id.is_empty()));
        assert_eq!(sent["role"], "ROLE_USER");
        assert_eq!(sent["parts"], json!([{ "text": "alice: hello" }, { "text": "bob: hi" }]));
        assert!(sent.get("contextId").is_none() && sent.get("taskId").is_none(), "no conversation yet, no task answered");

        let reply = OutboundMessage { prompts: Vec::new(), said: vec![said("alice: Berlin")], answers: Some(task("t-1")) };
        let sent = serde_json::to_value(wire_message(&reply, Some(&conversation("c-1")))).expect("json");
        assert_eq!((sent["contextId"].as_str(), sent["taskId"].as_str()), (Some("c-1"), Some("t-1")));
    }

    #[test]
    fn the_prompt_parts_come_first_as_they_are() {
        use caelumora_runtime::external::Said;
        let message = OutboundMessage {
            prompts: vec!["[Who is in this channel]\nYou are Hermes (@hermes).".to_string()],
            said: vec![Said { sender: "alice".to_string(), text: "alice: hello".to_string() }],
            answers: None,
        };
        let sent = serde_json::to_value(wire_message(&message, None)).expect("json");
        assert_eq!(sent["parts"], json!([{ "text": "[Who is in this channel]\nYou are Hermes (@hermes)." }, { "text": "alice: hello" }]));
    }

    fn response(value: Value) -> StreamResponse {
        serde_json::from_value(value).expect("a response")
    }

    fn task(id: &str) -> TaskId {
        TaskId(id.to_string())
    }

    fn conversation(id: &str) -> ConversationId {
        ConversationId(id.to_string())
    }

    fn chunk(id: &str, text: &str, append: bool, last: bool) -> StreamResponse {
        response(json!({ "artifactUpdate": { "contextId": "c-1", "artifact": { "artifactId": id, "parts": [{ "text": text }] }, "append": append, "lastChunk": last } }))
    }

    fn ended_in(state: &str, text: Option<&str>) -> Vec<ExternalEvent> {
        let message = text.map(|t| json!({ "parts": [{ "text": t }] }));
        Translation::default().events(response(json!({ "statusUpdate": { "status": { "state": state, "message": message } } })))
    }

    #[test]
    fn an_artifact_in_chunks_is_told_piece_by_piece_and_whole_at_its_last() {
        let mut translation = Translation::default();
        assert_eq!(translation.events(chunk("a", "Hel", false, false)), vec![ExternalEvent::Conversation(conversation("c-1")), ExternalEvent::Partial("Hel".to_string())]);
        assert_eq!(translation.events(chunk("a", "lo.", true, true)), vec![ExternalEvent::Message("Hello.".to_string())]);

        assert_eq!(translation.events(chunk("b", "Draft", false, false)), vec![ExternalEvent::Partial("Draft".to_string())]);
        assert_eq!(translation.events(chunk("b", "Final.", false, true)), vec![ExternalEvent::Message("Final.".to_string())], "a chunk not appended starts over");
    }

    #[test]
    fn the_conversation_and_the_task_are_told_once_and_again_when_they_change() {
        let mut translation = Translation::default();
        let working = |task: &str, context: &str| response(json!({ "task": { "id": task, "contextId": context, "status": { "state": "TASK_STATE_WORKING" } } }));
        assert_eq!(translation.events(working("t-1", "c-1")), vec![ExternalEvent::Conversation(conversation("c-1")), ExternalEvent::Started(task("t-1"))]);
        assert!(translation.events(working("t-1", "c-1")).is_empty(), "nothing new");
        assert_eq!(translation.events(working("t-2", "c-2")), vec![ExternalEvent::Conversation(conversation("c-2")), ExternalEvent::Started(task("t-2"))]);
        assert!(translation.events(response(json!({ "statusUpdate": { "contextId": "", "status": { "state": "TASK_STATE_WORKING" } } }))).is_empty(), "an empty context is none");
    }

    #[test]
    fn a_message_without_a_task_is_the_whole_answer() {
        let events = Translation::default().events(response(json!({ "message": { "contextId": "c-1", "parts": [{ "text": "Hi." }, { "data": {} }, { "text": "Bye." }] } })));
        assert_eq!(events, vec![ExternalEvent::Conversation(conversation("c-1")), ExternalEvent::Message("Hi.\nBye.".to_string()), ExternalEvent::Completed], "its text parts, one a line");
        assert_eq!(Translation::default().events(response(json!({ "message": { "parts": [] } }))), vec![ExternalEvent::Completed], "nothing to say");
    }

    #[test]
    fn a_task_tells_its_end_its_question_or_why_it_failed_and_nothing_while_it_works() {
        assert_eq!(ended_in("TASK_STATE_COMPLETED", Some("Done.")), vec![ExternalEvent::Message("Done.".to_string()), ExternalEvent::Completed]);
        assert_eq!(ended_in("TASK_STATE_COMPLETED", None), vec![ExternalEvent::Completed]);
        assert_eq!(ended_in("TASK_STATE_CANCELED", Some("ignored")), vec![ExternalEvent::Canceled]);
        assert_eq!(ended_in("TASK_STATE_FAILED", Some("out of coffee")), vec![ExternalEvent::Failed("out of coffee".to_string())]);
        assert_eq!(ended_in("TASK_STATE_FAILED", None), vec![ExternalEvent::Failed("the task failed".to_string())]);
        assert_eq!(ended_in("TASK_STATE_REJECTED", None), vec![ExternalEvent::Failed("the task was refused".to_string())]);
        assert_eq!(ended_in("TASK_STATE_INPUT_REQUIRED", Some("Which city?")), vec![ExternalEvent::InputRequired("Which city?".to_string())]);
        assert_eq!(ended_in("TASK_STATE_INPUT_REQUIRED", None), vec![ExternalEvent::InputRequired("it needs more to go on".to_string())]);
        assert_eq!(ended_in("TASK_STATE_AUTH_REQUIRED", Some("log in")), vec![ExternalEvent::Failed("it asks for an authentication it was not given".to_string())]);
        assert!(ended_in("TASK_STATE_WORKING", Some("thinking")).is_empty(), "what it says meanwhile is not the answer");
        assert!(ended_in("TASK_STATE_SOMETHING_NEW", Some("?")).is_empty(), "nor is a state not known");
    }

    #[test]
    fn a_task_that_was_started_is_told_without_its_artifacts() {
        let found: WireTask = serde_json::from_value(json!({
            "id": "t-1", "contextId": "c-1", "status": { "state": "TASK_STATE_WORKING" },
            "artifacts": [{ "artifactId": "a", "parts": [{ "text": "so far" }] }],
        })).expect("a task");
        let mut translation = Translation::default();
        assert_eq!(translation.started(&found), vec![ExternalEvent::Conversation(conversation("c-1")), ExternalEvent::Started(task("t-1"))]);
        assert!(translation.started(&found).is_empty(), "told once");
    }
}
