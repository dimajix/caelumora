//! Changing a channel: its name, language, description, use of the workspace, approval rules and
//! classification.

use anyhow::Result;
use caelumora_model::approval::ApprovalRule;
use caelumora_model::confidentiality::{Classification, ConfidentialityQuestions};
use caelumora_model::security::{ActionType, Reclassified, SecurityContext};
use tracing::info;
use super::{ChannelActor, ChannelError, ChannelLoop};

impl ChannelLoop {
    /// Renames the channel on behalf of `context`: stored first, which is
    /// where the permission is checked (`UpdateChannel`, issue 70).
    ///
    /// # Errors
    /// `StoreError::Invalid` if `new_name` is no name,
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// channel, or whatever else storing it reports.
    pub async fn update_name(&self, context: &SecurityContext, new_name: &str) -> Result<()> {
        Ok(self.channel.update_name(context, new_name).await?)
    }

    /// Has the channel's conversation held in `language`, or in none in
    /// particular, on behalf of `context` (issue 56): stored first, which is
    /// where the permission is checked (`UpdateChannel`), like a new name.
    /// Its agents are told from their next turn on; nothing said before is
    /// translated.
    ///
    /// # Errors
    /// `StoreError::Invalid` if `language` is no language tag,
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// channel, or whatever else storing it reports.
    pub async fn update_language(&self, context: &SecurityContext, language: Option<&str>) -> Result<()> {
        Ok(self.channel.update_language(context, language).await?)
    }

    /// Says what the channel is for (issue 245) on behalf of `context`:
    /// stored first, which is where the permission is checked
    /// (`UpdateChannel`), like a new name. Its agents are told from their
    /// next turn on.
    ///
    /// # Errors
    /// `StoreError::Validation` if `summary` or `briefing` is too long or
    /// holds what it may not, `StoreError::PermissionDenied` if `context`
    /// may not change the channel, or whatever else storing it reports.
    pub async fn update_description(&self, context: &SecurityContext, summary: Option<&str>, briefing: Option<&str>) -> Result<()> {
        Ok(self.channel.update_description(context, summary, briefing).await?)
    }

    /// Sets whether the channel's participants work on the workspace of its
    /// session here (issue 251) on behalf of `context`: stored first, which
    /// is where the permission is checked (`UpdateChannel`). It permits
    /// nothing the classifications refuse; it takes effect with the next
    /// use.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// channel, or whatever else storing it reports.
    pub async fn update_uses_workspace(&self, context: &SecurityContext, uses: bool) -> Result<()> {
        Ok(self.channel.update_uses_workspace(context, uses).await?)
    }

    /// Sets what the channel restricts of the tool calls made in it (issue
    /// 13) on behalf of `context`: stored first, which is where the
    /// permission is checked (`UpdateChannel`). Its agents' next calls are
    /// decided by them.
    ///
    /// # Errors
    /// `StoreError::Validation` if a rule does not only ask or deny,
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// channel, or whatever else storing it reports.
    pub async fn update_approval_rules(&self, context: &SecurityContext, rules: Vec<ApprovalRule>) -> Result<()> {
        Ok(self.channel.update_approval_rules(context, rules).await?)
    }

    /// Reclassifies the channel as `classification` on behalf of `context`
    /// (issue 133): asked as `ActionType::Reclassify` - its owner's -, the
    /// classification checked as one that can be given at all, then stored
    /// (`UpdateChannel`) and announced. Lowering it releases what is in it:
    /// every message is fitted into the new classification
    /// (`History::fit_into`), since the agents in the channel carry
    /// what they know into what they write next. Raising it changes nothing
    /// already written.
    ///
    /// A turn that is running keeps the classification it began with. What
    /// it saw is released by nobody, so lowering the channel ends it (issue
    /// 141): once stored, the channel's turns are cancelled and the channel
    /// kept quiet, as `cancel` does. Each agent then finds at its next step
    /// that its turn does not flow into the channel any more and gives it
    /// up (`GivenUpReason::Lowered`) instead of taking it over. Lowered
    /// means: what the channel held, classified as it was, may not be put
    /// into it as it is now (`ConfidentialityPolicy::may_publish`, in
    /// `context`'s name) - a question, not a permission.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if `context` may not reclassify the
    /// channel, `ClassificationRefused` if `classification` names
    /// compartments that are not configured, `NotCleared` if it is raised
    /// above a participant; whatever storing it reports.
    pub async fn reclassify(&self, context: &SecurityContext, classification: Classification) -> Result<()> {
        let channel = self.classification();
        self.require(context, |attributes| ActionType::Reclassify(Reclassified::Channel { attributes }), "reclassifying").await?;
        self.confidentiality.require_classifiable(&classification, || ChannelError::ClassificationRefused { classification: classification.clone(), channel: self.id })?;
        // Raised above a participant, the channel would show them what they
        // may not see: refused rather than anyone removed.
        for ChannelActor { participant, .. } in self.actors.load().values() {
            self.participation.require_cleared(participant, context, &classification)?;
        }
        self.channel.update_classification(context, classification.clone()).await?;
        let lowered = !self.confidentiality.may_publish(context, &channel, &classification);
        if lowered {
            self.cancel_actors();
        }
        let mut state = self.state.lock().await;
        state.history.fit_into(&classification);
        self.store_state(&mut state).await?;
        info!(channel = %self.id, %classification, lowered, by = %context.caller_id, "Channel reclassified");
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use crate::channel::ChannelLoop;

    use caelumora_model::confidentiality::Classification;
    use caelumora_model::security::{SecurityContext, Grant, Subject};
    use caelumora_model::store::StoreError;
    use std::sync::Arc;

    
    use caelumora_model::actor::Rejection;
    use crate::session::{ChannelSpec, SessionManager};
    use crate::session::testing::*;
    use caelumora_model::message::{Message, MessageId};
    use crate::error::is_forbidden;
    use std::time::Duration;
    use tokio::time;

    #[tokio::test]
    async fn a_channel_is_not_renamed_to_a_name_its_session_has() {
        // Kept by the store (issue 250): in another session the name is
        // free, which the store suite says.
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let side = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();

        let refused = side.update_name(&ctx, "Main").await.expect_err("the session has a main");
        assert!(matches!(refused.downcast_ref::<StoreError>(), Some(StoreError::NameTaken { entity: "channel" })), "{refused}");
        assert_eq!(side.name().as_str(), "side", "unchanged");
        assert_eq!(runtime.entity_stores().channels.get(&ctx, side.id()).await.unwrap().name.as_str(), "side", "also as stored");
        side.update_name(&ctx, "Side").await.expect("its own name, written otherwise");
        assert_eq!(side.name().as_str(), "Side");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_and_its_messages_are_reclassified_by_who_may() {
        use crate::channel::ChannelError;
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Compartment, Compartments, ConfidentialityRules, Confidentiality};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![]), (Compartment::new("finance"), vec![])]).unwrap();
        let (_, manager) = owned_runtime(ConfidentialityRules::new(compartments));
        let runtime = Arc::clone(runtime_of(&manager));
        let (alice, bob) = (user("alice"), user("bob"));
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(bob.id));
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());
        let bob_ctx = SecurityContext::new(bob_id, Default::default(), Default::default());
        let mut session = make_session("hers", alice_id);
        session.security_attributes = session.security_attributes.clone().granting(Subject::Actor(bob_id), Grant::Read);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();

        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let spec = ChannelSpec { classification: Some(classified(Confidentiality::Confidential, &["hr"])), ..spec("hr", vec![bob_id]) };
        let hr = session.create_channel(&alice_ctx, spec).await.unwrap();
        hr.user_actor(bob.id).unwrap().send_message(Some("from bob".to_string()), None, Vec::new()).await.unwrap();
        hr.user_actor(alice.id).unwrap().send_message(Some("from alice".to_string()), None, Vec::new()).await.unwrap();
        let history = history_of(&hr, 2).await;
        // Two users' messages arrive in either order.
        let id_of = |history: &[Message], text: &str| history.iter().find(|m| m.content.as_deref() == Some(text)).map(|m| m.id).unwrap();
        let (bobs, alices) = (id_of(&history, "from bob"), id_of(&history, "from alice"));

        // A change to what was said, through the actor of whoever makes it
        // (issue 239).
        let (as_alice, as_bob) = (hr.user_actor(alice.id).unwrap(), hr.user_actor(bob.id).unwrap());
        let released = classified(Confidentiality::Internal, &["hr", "finance"]);
        as_bob.reclassify_message(bobs, released.clone()).await.expect("the author releases their message");
        let mut bobs_events = as_bob.watch();
        as_bob.reclassify_message(alices, released.clone()).await.expect("sent: the channel decides");
        assert_eq!(rejection(&mut bobs_events).await, (Some(alices), Rejection::Forbidden), "not another's, and he is told (issue 304)");
        let missing = as_alice.reclassify_message(MessageId::new(), released.clone()).await.expect_err("no such message");
        assert!(matches!(missing.downcast_ref::<ChannelError>(), Some(ChannelError::MessageNotFound { .. })), "{missing}");
        classified_as(&hr, bobs, &released).await;

        let lowered = classified(Confidentiality::Internal, &["hr"]);
        assert!(hr.reclassify(&bob_ctx, lowered.clone()).await.is_err(), "only the owner reclassifies the channel");
        hr.reclassify(&alice_ctx, lowered.clone()).await.expect("the owner lowers it");
        assert_eq!(hr.classification(), lowered);
        assert_eq!(runtime.entity_stores().channels.get(&alice_ctx, hr.id()).await.unwrap().classification, lowered, "stored");
        let history = hr.history().await;
        let classification_of = |id| history.iter().find(|m| m.id == id).map(|m| m.classification.clone()).unwrap();
        assert_eq!(classification_of(bobs), released, "what was released stays released");
        assert_eq!(classification_of(alices), lowered, "the rest is released with the channel");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn renaming_a_channel_is_stored_and_only_the_owner_may() {
        use caelumora_model::actor::ActorId;

        let (store, manager) = owned_runtime(Default::default());
        let users = (user("alice"), user("bob"));
        let (alice, bob) = (ActorId::User(users.0.id), ActorId::User(users.1.id));
        let as_user = |actor| SecurityContext::new(actor, Default::default(), Default::default());
        let mut session = make_session("hers", alice);
        session.participants = record_users(&manager, &[&users.0, &users.1]).await;
        let running = create(&manager, &as_user(alice), session).await.unwrap();
        let main = running.channel_named("main").unwrap();

        let refused = main.update_name(&as_user(bob), "bobs").await.expect_err("a contributor may not");
        assert!(is_forbidden(&refused), "{refused}");
        assert_eq!(main.name(), "main", "unchanged");

        main.update_name(&as_user(alice), "lobby").await.expect("the owner may");
        assert_eq!(main.name(), "lobby");
        assert_eq!(store.entity_stores().unwrap().channels.get(&as_user(alice), main.id()).await.unwrap().name, "lobby", "stored, not only in memory");
        manager.stop(&SecurityContext::system(), running.id()).await.unwrap();
    }

    /// Waits until `channel`'s history holds `count` messages.
    /// Waits until the message `id` of `channel` is classified `classification`.
    async fn classified_as(channel: &ChannelLoop, id: MessageId, classification: &Classification) {
        for _ in 0..100 {
            if channel.message(id).await.is_some_and(|m| m.classification == *classification) {
                return;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
        panic!("message {id} was never classified {classification}");
    }
}
