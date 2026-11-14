//! How the terminal shows what it prints and reads what the user types: one way of drawing per
//! implementation of [`Screen`] (issue 569).

mod line;

pub(crate) use line::LineScreen;

use crate::printed::Printed;

/// A way of drawing the terminal: it shows what is printed, and reads what the user types.
pub(crate) trait Screen {
    /// Shows `printed`, after what was shown before.
    fn print(&mut self, printed: Vec<Printed>);

    /// What the user typed next. Nothing is lost if it is dropped before it completes.
    async fn next_input(&mut self) -> Typed;
}

/// What the user typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Typed {
    /// A line, as typed.
    Line(String),
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
