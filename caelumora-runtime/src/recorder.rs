use std::fmt::Debug;
use std::sync::Arc;
use tokio::sync::Mutex;
use async_trait::async_trait;
use caelumora_model::session::ChannelState;
use crate::actor::{ActorEvent, Envelope};


#[async_trait]
pub trait Recorder: Send + Sync + Debug {
    async fn record_send(&mut self, event: &Envelope) -> ();
    async fn record_receive(&mut self, event: &Envelope) -> ();
}

#[derive(Debug)]
pub struct BlackholeRecorder {
}

impl BlackholeRecorder {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl Recorder for BlackholeRecorder {
    async fn record_send(&mut self, _event: &Envelope) -> () {}
    async fn record_receive(&mut self, _event: &Envelope) -> () {}
}

#[derive(Debug)]
pub struct ChannelHistoryRecorder {
    state: Arc<Mutex<ChannelState>>,
}

impl ChannelHistoryRecorder {
    pub fn new(state: Arc<Mutex<ChannelState>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Recorder for ChannelHistoryRecorder {
    async fn record_send(&mut self, envelope: &Envelope) {
    }
    async fn record_receive(&mut self, envelope: &Envelope) {
        let mut state = self.state.lock().await;
        match &envelope.event {
            ActorEvent::FullResponseGenerated(msgs) => {
                state.history.extend(msgs.clone());
            }
            // Handle other event types as needed
            _ => {}
        }
    }
}