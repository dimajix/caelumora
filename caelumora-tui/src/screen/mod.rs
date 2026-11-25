//! How the terminal shows what it prints and reads what the user types: one way of drawing per
//! implementation of [`Screen`] (issue 569).
//!
//! Both tell a tool call by what its tool says it does, else by its arguments, and how it ended
//! by its result's presentation - a command by where it stands and how much it printed - (`words`,
//! issue 586); `full` keeps a call with its result as one entry, folded, which unfolds to what the
//! presentation shows of it - for a command, what it printed -, and `line` prints no more than the
//! two lines.

mod full;
mod line;
mod words;

pub(crate) use full::FullScreen;
pub(crate) use line::LineScreen;

use crate::activity::Activity;
use crate::asking::Asked;
use crate::printed::Printed;
use crate::typeable::Typeable;
use async_trait::async_trait;
use std::sync::Arc;

/// A way of drawing the terminal: it shows what is printed and the answers under way, and reads
/// what the user types.
#[async_trait]
pub(crate) trait Screen: Send {
    /// Shows `printed`, after what was shown before.
    fn print(&mut self, printed: Vec<Printed>);

    /// Shows what is at work in the channels followed, `activity`, as it stands now (issue 572).
    fn show_activity(&mut self, activity: &Activity);

    /// Shows what is asked of the user now, `asked` - as waiting only, where it is put aside
    /// (issue 571).
    fn ask(&mut self, asked: Option<&Asked>);

    /// Takes what the user can type now, `typeable`, to complete what they type from (issue 579).
    fn offer(&mut self, typeable: Arc<Typeable>);

    /// What the user typed next. Nothing is lost if it is dropped before it completes.
    async fn next_input(&mut self) -> Typed;
}

/// What the user typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Typed {
    /// A line, as typed.
    Line(String),
    /// The choices of what is asked, by their indices.
    Chosen(Vec<usize>),
    /// What is asked put aside, to type on.
    PutAside,
    /// What was put aside taken up again.
    TakenUp,
    /// Ctrl-C.
    Interrupted,
    /// The input cannot be read any longer, for the reason given.
    Failed(String),
    /// The input ended.
    Ended,
}

#[cfg(test)]
pub(crate) mod testing {
    //! What the tests of the terminal compare what it prints by: the lines the way of drawing
    //! `line` shows of it.

    pub(crate) use super::line::testing::lines;
}
