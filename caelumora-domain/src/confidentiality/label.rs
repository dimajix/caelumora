//! What set the label of a message (issue 170, `LabelCauses`): which of the classifications in its
//! context made the label what it is.

use super::Classification;
use crate::context::ModelProfile;
use crate::message::Message;

/// Which of the classifications in a message's context set its label
/// (issue 170). The label of a message is the join of everything its turn
/// had seen, at most the channel's classification (issue 168); what set it
/// is derived from that, nothing about it is stored:
///
/// - what was not classified counted as the channel, and so set the label
///   to the channel's;
/// - if the classified parts had no audience in common, the label is the
///   channel's, set by the parts with an audience;
/// - otherwise the parts at the label's level set it.
#[derive(Debug)]
pub struct LabelCauses {
    label: Classification,
    disjoint: bool,
}

impl LabelCauses {
    /// The causes of `label`, the classification of a message whose
    /// context's parts are classified `classifications` - those known.
    pub fn new<'a>(
        label: Classification,
        classifications: impl IntoIterator<Item = &'a Classification>,
    ) -> Self {
        let disjoint = Classification::join_all(classifications).is_none();
        Self { label, disjoint }
    }

    /// Whether a part classified `classification` - `None`: not known -
    /// set the label.
    pub fn set_by(&self, classification: Option<&Classification>) -> bool {
        match classification {
            None => true,
            Some(c) if self.disjoint => !c.compartments().is_empty(),
            Some(c) => c.level() == self.label.level(),
        }
    }

    /// Whether `message`, as the model of `profile` was given it, set the
    /// label: by its own classification, that of a tool result in it, or that
    /// of an attachment it was given along (issue 282).
    pub fn set_by_message(&self, message: &Message, profile: &ModelProfile) -> bool {
        profile
            .classifications_given(message)
            .any(|classification| self.set_by(Some(classification)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::testing::classified;
    use crate::confidentiality::Confidentiality::*;
    use crate::content::testing::make_content_ref;
    use crate::content::ContentRef;
    use crate::tool::{ToolCall, ToolCallResult};

    #[test]
    fn what_set_a_label_is_told_by_level_by_audience_and_by_what_is_not_known() {
        let internal = Classification::general(Internal);
        let confidential = Classification::general(Confidential);

        let causes = LabelCauses::new(confidential.clone(), [&internal, &confidential]);
        assert!(causes.set_by(Some(&confidential)), "a part at the label's level set it");
        assert!(!causes.set_by(Some(&internal)), "one below did not");
        assert!(causes.set_by(None), "what is not known counted as the channel");

        let sales = classified(Internal, &["sales"]);
        let causes =
            LabelCauses::new(confidential.clone(), [&sales, &classified(Internal, &["hr"])]);
        assert!(
            causes.set_by(Some(&sales)),
            "without an audience in common, the parts with one set the channel's label"
        );
        assert!(!causes.set_by(Some(&internal)), "a general part did not");

        let causes = LabelCauses::new(confidential.clone(), [&internal, &confidential]);
        let result = ToolCallResult::of(
            &ToolCall {
                id: Some("1".to_string()),
                function_name: "noop".to_string(),
                arguments: serde_json::json!({}),
                visibility: Default::default(),
                presentation: Default::default(),
            },
            Default::default(),
            "secret".to_string(),
            confidential.clone(),
        );
        let message = Message { tool_results: vec![result], ..Message::new(internal.clone()) };
        let profile = ModelProfile::default();
        assert!(
            causes.set_by_message(&message, &profile),
            "a tool result in a message sets the label as the message"
        );
        let attached = Message {
            attachments: vec![ContentRef {
                classification: confidential.clone(),
                ..make_content_ref("a.png", b"a")
            }],
            ..Message::new(internal.clone())
        };
        assert!(
            causes.set_by_message(&attached, &ModelProfile { images: true, ..profile }),
            "and an attachment given along (issue 282)"
        );
        assert!(!causes.set_by_message(&attached, &profile), "not one only noted");
        assert!(!causes.set_by_message(&Message::new(internal), &profile));
    }
}
