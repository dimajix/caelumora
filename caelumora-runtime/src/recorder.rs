use std::fmt::Debug;
use async_trait::async_trait;
use crate::actor::Envelope;


#[async_trait]
pub trait Recorder: Send + Sync + Debug {
    fn record(&mut self, event: &Envelope) -> ();
}

#[derive(Debug)]
pub struct BlackholeRecorder {
}

impl BlackholeRecorder {
    pub fn new() -> Self {
        Self {}
    }
}

impl Recorder for BlackholeRecorder {
    fn record(&mut self, _event: &Envelope) -> () {}
}