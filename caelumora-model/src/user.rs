use crate::event::{EventPublisher, GlobalEvent};
use crate::actor::ActorId;
use crate::security::{SecurityAttributes, SecurityContext};
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
    pub security: SecurityContext,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserUpdateEvent {
    NameChanged(String),
}

#[derive(Debug)]
pub struct UserController {
    id: UserId,
    user: ArcSwap<User>,
    event_publisher: Arc<dyn EventPublisher>,
}

impl UserController {
    pub fn new(user: User, event_publisher: Arc<dyn EventPublisher>) -> Self {
        let id = user.id;
        Self {
            id,
            user: ArcSwap::from_pointee(user),
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

    pub fn update_username(&self, new_name: String) {
        let event = UserUpdateEvent::NameChanged(new_name);
        self.process_user_update(&event);
        // A user's events are the user's own business (issue 39).
        let attributes = SecurityAttributes::owned_by(ActorId::User(self.id));
        self.event_publisher.publish(GlobalEvent::UserUpdated(self.id, event), attributes);
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

    pub fn process_global_event(&self, event: &GlobalEvent) {
        match event {
            GlobalEvent::UserUpdated(uid, update) if *uid == self.id => {
                self.process_user_update(update);
            }
            _ => {}
        }
    }
}
