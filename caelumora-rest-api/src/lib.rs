//! The wire of Caelumora's REST API (issue 323): the requests a client
//! sends and the responses the server answers, as `serde` reads and writes
//! them. Nothing here serves or sends them - the server is
//! `caelumora-rest-server`, a client is `caelumora-rest-client` -, so that
//! both depend on these types alone and neither on the other.
//!
//! A response is made from what the model gives - an entity, a snapshot of
//! it as a reader sees it - by `From` or an `of`; what only the runtime
//! knows is put into it by the server.

pub mod access;
pub mod agent;
pub mod artifact;
pub mod channel;
pub mod compartment;
pub mod context;
pub mod group;
pub mod history;
pub mod profile;
pub mod scope;
pub mod session;
pub mod skill;
pub mod stream;
pub mod template;
pub mod trash;
pub mod workspace;
