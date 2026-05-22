use std::fmt::Debug;
use async_trait::async_trait;
use crate::actor::ActorEvent;


#[async_trait]
pub trait Recorder: Send + Sync + Debug {
    fn record(&self, event: &ActorEvent) -> ();
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
    fn record(&self, _event: &ActorEvent)  -> () {}
}