use std::fmt::Debug;


pub trait SecurityManager : Debug + Sync + Send {

}

#[derive(Debug)]
pub struct DummySecurityManager {

}
impl DummySecurityManager {
    pub fn new() -> Self {
        Self {}
    }
}
impl SecurityManager for DummySecurityManager {
}