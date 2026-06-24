use crate::agent::{AgentId, AgentUpdateEvent};
use crate::channel::{ChannelId, ChannelUpdateEvent};
use crate::session::{SessionId, SessionUpdateEvent};
use crate::user::{UserId, UserUpdateEvent};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GlobalEvent {
    SessionCreated(SessionId),
    SessionUpdated(SessionId, SessionUpdateEvent),
    SessionDeleted(SessionId),
    ChannelCreated(SessionId, ChannelId),
    ChannelUpdated(SessionId, ChannelId, ChannelUpdateEvent),
    ChannelDeleted(SessionId, ChannelId),
    AgentCreated(SessionId, ChannelId, AgentId),
    AgentUpdated(SessionId, ChannelId, AgentId, AgentUpdateEvent),
    AgentDeleted(SessionId, ChannelId, AgentId),
    UserCreated(UserId),
    UserUpdated(UserId, UserUpdateEvent),
    UserDeleted(UserId),
}

pub trait EventPublisher: Send + Sync {
    fn publish(&self, event: GlobalEvent);
}

use std::fmt;

pub trait EventSender<E>: Send + Sync + fmt::Debug {
    fn send(&self, event: E);
}

pub struct FnEventSender<E, F>
where
    F: Fn(E) + Send + Sync,
{
    f: F,
    _phantom: std::marker::PhantomData<fn(E)>,
}

impl<E, F> FnEventSender<E, F>
where
    F: Fn(E) + Send + Sync,
{
    pub fn new(f: F) -> Self {
        Self {
            f,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<E, F> EventSender<E> for FnEventSender<E, F>
where
    F: Fn(E) + Send + Sync,
    E: Send + Sync,
{
    fn send(&self, event: E) {
        (self.f)(event);
    }
}

impl<E, F> fmt::Debug for FnEventSender<E, F>
where
    F: Fn(E) + Send + Sync,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FnEventSender")
    }
}
