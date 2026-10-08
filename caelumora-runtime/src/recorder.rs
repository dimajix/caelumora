use async_trait::async_trait;
use caelumora_model::actor::Envelope;
use std::fmt::Debug;

#[async_trait]
pub trait Recorder: Send + Sync + Debug {
    async fn record_send(&mut self, event: &Envelope) -> ();
    async fn record_receive(&mut self, event: &Envelope) -> ();
}

#[derive(Debug, Default)]
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
