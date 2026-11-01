//! How envelopes are carried within one process (issue 223): through queues in memory, which
//! the channel and its actors send and receive by.

pub mod memory;
pub use memory::{InMemoryEventReceiver, InMemoryEventSender, OriginEventSender};
