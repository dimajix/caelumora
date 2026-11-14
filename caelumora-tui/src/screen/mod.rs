//! How the terminal shows what it prints and reads what the user types: one way of drawing per
//! implementation of [`Screen`] (issue 569).

mod full;
mod line;
mod words;

pub(crate) use full::FullScreen;
pub(crate) use line::LineScreen;

use crate::asking::Asked;
use crate::printed::Printed;
use crate::under_way::UnderWay;
use async_trait::async_trait;

/// A way of drawing the terminal: it shows what is printed and the answers under way, and reads
/// what the user types.
#[async_trait]
pub(crate) trait Screen: Send {
    /// Shows `printed`, after what was shown before.
    fn print(&mut self, printed: Vec<Printed>);

    /// Shows the answers under way in the channel talked in, `under_way`, as they stand now.
    fn show_under_way(&mut self, under_way: &[UnderWay]);

    /// Shows what is asked of the user now, `asked` - as waiting only, where it is put aside
    /// (issue 571).
    fn ask(&mut self, asked: Option<&Asked>);

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
