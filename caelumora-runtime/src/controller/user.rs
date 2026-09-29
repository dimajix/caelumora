use arc_swap::ArcSwap;
use caelumora_model::event::GlobalEvent;
use caelumora_model::user::{User, UserId, UserUpdateEvent};
use std::sync::Arc;

/// The live user of a `UserActor`. It has no store: users are not changed
/// through it, only kept current when a change is announced.
#[derive(Debug)]
pub struct UserController {
    id: UserId,
    user: ArcSwap<User>,
}

impl UserController {
    pub fn new(user: User) -> Self {
        let id = user.id;
        Self {
            id,
            user: ArcSwap::from_pointee(user),
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
