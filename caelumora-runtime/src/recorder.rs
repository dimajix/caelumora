use std::fmt::Debug;
use std::sync::Arc;
use tokio::sync::Mutex;
use async_trait::async_trait;
use caelumora_model::session::ChannelState;
use crate::actor::{ActorEvent, Envelope};


#[async_trait]
pub trait Recorder: Send + Sync + Debug {
    async fn record(&mut self, event: &Envelope) -> ();
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
    async fn record(&mut self, _event: &Envelope) -> () {}
}

#[derive(Debug)]
pub struct StatefulRecorder {
    state: Arc<Mutex<ChannelState>>,
}

impl StatefulRecorder {
    pub fn new(state: Arc<Mutex<ChannelState>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Recorder for StatefulRecorder {
    async fn record(&mut self, envelope: &Envelope) {
        let mut state = self.state.lock().await;
        match &envelope.event {
            ActorEvent::CompleteMessageGenerated(msg) => {
                state.history.push(msg.clone(), "".to_string());
            }
            // Handle other event types as needed
            _ => {}
        }
    }
}