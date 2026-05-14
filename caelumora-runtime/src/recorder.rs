use std::fmt::Debug;
use async_trait::async_trait;

#[async_trait]
pub trait Recorder: Send + Sync + Debug {
    
}