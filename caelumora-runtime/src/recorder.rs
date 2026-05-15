use std::fmt::Debug;
use async_trait::async_trait;

#[async_trait]
pub trait Recorder: Send + Sync + Debug {
    
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

}