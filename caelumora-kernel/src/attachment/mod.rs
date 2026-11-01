//! What messages carry besides their text (issue 282): the one owner of the content of attachments,
//! the `AttachmentManager` - what is uploaded for a message not yet sent, what a message claims as
//! its channel admits it, what a tool gave its model (issue 512), what is read for a download or
//! given a model along - held in memory for its next calls -, and what goes with a channel purged.
//! An attachment is a `ContentRef` a message holds; its bytes lie under
//! `attachments/<channel>/<id>` in a storage of content.

mod cache;
mod manager;

pub use manager::AttachmentManager;
