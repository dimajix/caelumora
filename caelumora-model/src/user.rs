use serde::{Deserialize, Serialize};
use std::sync::Arc;
use arc_swap::ArcSwap;
use crate::event::EventSender;

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
    user: ArcSwap<User>,
    channel_name: ArcSwap<String>,
    event_sender: Arc<dyn EventSender<UserUpdateEvent>>,
}

impl UserController {
    pub fn new(user: User, channel_name: String, event_sender: Arc<dyn EventSender<UserUpdateEvent>>) -> Self {
        Self {
            user: ArcSwap::from_pointee(user),
            channel_name: ArcSwap::from_pointee(channel_name),
            event_sender,
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<User>> {
        self.user.load()
    }

    pub fn id(&self) -> UserId {
        self.user.load().id
    }

    pub fn username(&self) -> String {
        self.user.load().username.clone()
    }

    pub fn channel_name(&self) -> String {
        (**self.channel_name.load()).clone()
    }

    pub fn update_username(&self, new_name: String) {
        self.apply_update(UserUpdateEvent::NameChanged(new_name));
    }

    fn apply_update(&self, event: UserUpdateEvent) {
        self.process_update(&event);
        self.event_sender.send(event);
    }

    pub fn process_update(&self, event: &UserUpdateEvent) {
        match event {
            UserUpdateEvent::NameChanged(name) => {
                let mut user = (**self.user.load()).clone();
                user.username = name.clone();
                self.user.store(Arc::new(user));
            }
        }
    }

    pub fn process_channel_update(&self, event: &crate::channel::ChannelUpdateEvent) {
        match event {
            crate::channel::ChannelUpdateEvent::NameChanged(name) => {
                self.channel_name.store(Arc::new(name.clone()));
            }
        }
    }
}
