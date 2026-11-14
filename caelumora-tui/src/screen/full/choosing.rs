//! What is asked of the user in the full-screen terminal, as a choice above the line typed: the
//! choice the cursor stands on, those marked, and the keys that move, mark and choose (issue 571).

use super::Typed;
use crate::asking::{Asked, Asking};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::Line;
use std::collections::BTreeSet;

/// What is asked, as it is chosen among: arrows move between its choices, a digit or Enter
/// chooses, Space marks where several may be chosen; Esc puts it aside, and takes it up again.
#[derive(Debug)]
pub(super) struct Choosing {
    /// What is asked, shown or put aside.
    asked: Asked,
    /// The index of the choice the cursor stands on.
    selected: usize,
    /// The indices of the choices marked, where several may be chosen.
    marked: BTreeSet<usize>,
}

/// What a key came to as it was chosen with (`Choosing::take_key`).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Taken {
    /// The key is none of the choice's: the line typed takes it.
    Passed,
    /// The key moved or marked.
    Kept,
    /// The key chose, or put what is asked aside, or took it up again.
    Typed(Typed),
}

impl Choosing {
    /// `asked` to be chosen among, the cursor on the choice recommended, or else the first.
    pub(super) fn new(asked: Asked) -> Self {
        let question = asked.asking().question();
        let recommended = question.and_then(|(_, q)| q.choices.iter().position(|c| c.recommended));
        Self { asked, selected: recommended.unwrap_or(0), marked: BTreeSet::new() }
    }

    /// What is asked, shown or put aside.
    pub(super) fn asked(&self) -> &Asked {
        &self.asked
    }

    /// Shows what is asked as `asked` tells - shown or put aside -, the cursor and the marks where
    /// they stand.
    pub(super) fn show_as(&mut self, asked: Asked) {
        self.asked = asked;
    }

    /// Takes in `key`, pressed while nothing is typed: Up and Down move between the choices, the
    /// digit of one chooses it - or marks it, where several may be chosen, as Space does -, Enter
    /// chooses the one the cursor stands on - or those marked -, and Esc puts what is asked aside;
    /// put aside, Esc takes it up again, and the line typed takes every other key.
    pub(super) fn take_key(&mut self, key: &KeyEvent) -> Taken {
        let asking = match &self.asked {
            Asked::Shown(asking) => asking,
            Asked::Aside(_) if key.code == KeyCode::Esc => return Taken::Typed(Typed::TakenUp),
            Asked::Aside(_) => return Taken::Passed,
        };
        let count = asking.choices().len();
        let several = asking.multi_select();
        match key.code {
            KeyCode::Esc => Taken::Typed(Typed::PutAside),
            _ if count == 0 => Taken::Passed,
            KeyCode::Up => {
                self.selected = self.selected.checked_sub(1).unwrap_or(count - 1);
                Taken::Kept
            }
            KeyCode::Down => {
                self.selected = (self.selected + 1) % count;
                Taken::Kept
            }
            KeyCode::Char(' ') if several => {
                self.mark(self.selected);
                Taken::Kept
            }
            KeyCode::Char(digit) => match digit.to_digit(10).map(|d| d as usize) {
                Some(number) if (1..=count).contains(&number) && several => {
                    self.selected = number - 1;
                    self.mark(number - 1);
                    Taken::Kept
                }
                Some(number) if (1..=count).contains(&number) => {
                    Taken::Typed(Typed::Chosen(vec![number - 1]))
                }
                _ => Taken::Passed,
            },
            KeyCode::Enter => match self.marked.is_empty() {
                true => Taken::Typed(Typed::Chosen(vec![self.selected])),
                false => Taken::Typed(Typed::Chosen(self.marked.iter().copied().collect())),
            },
            _ => Taken::Passed,
        }
    }

    /// The rows that show what is asked, where it is shown: the question, its choices - the one
    /// the cursor stands on marked by an arrow, those marked checked -, and the keys that answer;
    /// none where it is put aside.
    pub(super) fn rows(&self) -> Vec<Line<'static>> {
        let Asked::Shown(asking) = &self.asked else { return Vec::new() };
        let several = asking.multi_select();
        let mut rows = vec![Line::styled(asking.what(), Style::new().bold())];
        for (index, choice) in asking.choices().into_iter().enumerate() {
            let cursor = if index == self.selected { "❯ " } else { "  " };
            let check = match (several, self.marked.contains(&index)) {
                (false, _) => "",
                (true, true) => "[x] ",
                (true, false) => "[ ] ",
            };
            let row = format!("{cursor}{check}{}. {choice}", index + 1);
            rows.push(match index == self.selected {
                true => Line::styled(row, Style::new().cyan()),
                false => Line::from(row),
            });
        }
        rows.push(Line::styled(keys(asking), Style::new().dim()));
        rows
    }

    /// What tells, on the rule, that what is asked waits, where it is put aside, and how to take
    /// it up again - and of questions, that a message declines them
    /// (`UserActor::questions_to_answer`); `None` where it is shown.
    pub(super) fn waiting(&self) -> Option<String> {
        match &self.asked {
            Asked::Shown(_) => None,
            Asked::Aside(Asking::Approval(_)) => {
                Some("an approval waits · Esc shows it".to_string())
            }
            Asked::Aside(Asking::Question { .. }) => {
                Some("questions wait · Esc shows them · a message declines them".to_string())
            }
        }
    }

    /// Marks the choice `index`, or takes its mark away.
    fn mark(&mut self, index: usize) {
        if !self.marked.remove(&index) {
            self.marked.insert(index);
        }
    }
}

/// What tells which keys answer `asking`.
fn keys(asking: &Asking) -> String {
    let has_choices = !asking.choices().is_empty();
    let keys = [
        (has_choices, "↑↓ choose"),
        (has_choices && asking.multi_select(), "Space marks"),
        (has_choices, "Enter answers"),
        (asking.takes_words(), "or type an answer"),
        (true, "Esc puts it aside"),
    ];
    let keys: Vec<&str> = keys.into_iter().filter(|(shown, _)| *shown).map(|(_, k)| k).collect();
    keys.join(" · ")
}
