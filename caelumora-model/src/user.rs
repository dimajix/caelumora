use crate::event::{EventPublisher, GlobalEvent};
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

define_id!(UserId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub email: String,
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserUpdateEvent {
    NameChanged(String),
}

#[derive(Debug)]
pub struct UserController {
    id: UserId,
    user: ArcSwap<User>,
    channel_name: ArcSwap<String>,
    event_publisher: Arc<dyn EventPublisher>,
}

impl UserController {
    pub fn new(user: User, channel_name: String, event_publisher: Arc<dyn EventPublisher>) -> Self {
        let id = user.id;
        Self {
            id,
            user: ArcSwap::from_pointee(user),
            channel_name: ArcSwap::from_pointee(channel_name),
            event_publisher,
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<User>> {
        self.user.load()
    }

    pub fn id(&self) -> UserId {
        self.id
    }

    pub fn username(&self) -> String {
        self.user.load().username.clone()
    }

    pub fn channel_name(&self) -> String {
        (**self.channel_name.load()).clone()
    }

    pub fn update_username(&self, new_name: String) {
        let event = UserUpdateEvent::NameChanged(new_name);
        self.process_user_update(&event);
        self.event_publisher.publish(GlobalEvent::UserUpdated(self.id, event));
    }

    fn process_user_update(&self, event: &UserUpdateEvent) {
        match event {
            UserUpdateEvent::NameChanged(name) => {
                let mut user = (**self.user.load()).clone();
                user.username = name.clone();
                self.user.store(Arc::new(user));
            }
        }
    }

    fn process_channel_update(&self, event: &crate::channel::ChannelUpdateEvent) {
        match event {
            crate::channel::ChannelUpdateEvent::NameChanged(name) => {
                self.channel_name.store(Arc::new(name.clone()));
            }
        }
    }

    pub fn process_global_event(&self, event: &GlobalEvent) {
        match event {
            GlobalEvent::UserUpdated(uid, update) if *uid == self.id => {
                self.process_user_update(update);
            }
            // Note: We don't know the channel ID here easily without adding it to UserController,
            // but for now we can just check if it's a channel update and let the Actor handle the ID check
            // Or better, let the Actor call this only for relevant events.
            // Actually, for UserActor, it knows its channel_id.
            //GlobalEvent::ChannelUpdated(_, cid, update) => if *cid == self.channel_id {
            //    self.process_channel_update(update);
            //}
            _ => {}
        }
    }
}
