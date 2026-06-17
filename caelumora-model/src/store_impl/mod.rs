use crate::store::{StateStore, TemplateStore};

pub mod json;
pub mod memory;

pub trait Store: TemplateStore + StateStore {}
impl<T: TemplateStore + StateStore> Store for T {}