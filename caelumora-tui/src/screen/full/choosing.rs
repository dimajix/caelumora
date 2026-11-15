//! What is asked of the user in the full-screen terminal, as a choice above the line typed: the
//! choice the cursor stands on, those checked, and the keys and clicks that move, check and choose
//! (issues 571, 576).

use super::{wrapped_rows, Typed};
use crate::asking::{Asked, Asking};
use crate::screen::words::{offered_words, what_is_asked};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::Line;
use std::collections::BTreeSet;

/// What is asked, as it is chosen among: arrows move between its choices, a digit or Enter
/// chooses, Space checks where several may be chosen, a click on a choice as its digit does; Esc
/// puts it aside, and takes it up again.
#[derive(Debug)]
pub(super) struct Choosing {
    /// What is asked, shown or put aside.
    asked: Asked,
    /// The index of the choice the cursor stands on.
    cursor: usize,
    /// The indices of the choices checked, where several may be chosen.
    checked: BTreeSet<usize>,
}

/// What a key or a click came to as it was chosen with (`Choosing::take_key`,
/// `Choosing::take_click`).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Taken {
    /// The key or the click is none of the choice's: the line typed takes the key.
    Passed,
    /// The key or the click moved or checked.
    Kept,
    /// The key or the click chose, or the key put what is asked aside, or took it up again.
    Typed(Typed),
}

impl Choosing {
    /// `asked` to be chosen among, the cursor on the choice recommended, or else the first.
    pub(super) fn new(asked: Asked) -> Self {
        let question = asked.asking().question();
        let recommended = question.and_then(|(_, q)| q.choices.iter().position(|c| c.recommended));
        Self { asked, cursor: recommended.unwrap_or(0), checked: BTreeSet::new() }
    }

    /// What is asked, shown or put aside.
    pub(super) fn asked(&self) -> &Asked {
        &self.asked
    }

    /// Shows what is asked as `asked` tells - shown or put aside -, the cursor and the checks where
    /// they stand.
    pub(super) fn show_as(&mut self, asked: Asked) {
        self.asked = asked;
    }

    /// Takes in `key`, pressed while nothing is typed: Up and Down move between the choices, the
    /// digit of one chooses it - or checks it, where several may be chosen, as Space does -, Enter
    /// chooses the one the cursor stands on - or those checked -, and Esc puts what is asked aside;
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
                self.cursor = self.cursor.checked_sub(1).unwrap_or(count - 1);
                Taken::Kept
            }
            KeyCode::Down => {
                self.cursor = (self.cursor + 1) % count;
                Taken::Kept
            }
            KeyCode::Char(' ') if several => {
                self.check(self.cursor);
                Taken::Kept
            }
            KeyCode::Char(digit) => match digit.to_digit(10).map(|d| d as usize) {
                Some(number) if (1..=count).contains(&number) => self.choose(number - 1),
                _ => Taken::Passed,
            },
            KeyCode::Enter => match self.checked.is_empty() {
                true => Taken::Typed(Typed::Chosen(vec![self.cursor])),
                false => Taken::Typed(Typed::Chosen(self.checked.iter().copied().collect())),
            },
            _ => Taken::Passed,
        }
    }

    /// Takes in a click on `row` of its rows as drawn `width` wide: one on a choice chooses it - or
    /// checks it, where several may be chosen -, one elsewhere is passed.
    pub(super) fn take_click(&mut self, row: u16, width: u16) -> Taken {
        let count = self.asked.asking().choices().len();
        let mut row = usize::from(row);
        for (index, line) in self.rows().iter().enumerate() {
            let height = wrapped_rows(line, width);
            if row < height {
                return match index.checked_sub(1) {
                    Some(choice) if choice < count => self.choose(choice),
                    _ => Taken::Passed,
                };
            }
            row -= height;
        }
        Taken::Passed
    }

    /// The rows that show what is asked, where it is shown: the question, its choices - the one
    /// the cursor stands on led by an arrow, those checked with an x -, and the keys that answer;
    /// none where it is put aside.
    pub(super) fn rows(&self) -> Vec<Line<'static>> {
        let Asked::Shown(asking) = &self.asked else { return Vec::new() };
        let several = asking.multi_select();
        let mut rows = vec![Line::styled(what_is_asked(asking), Style::new().bold())];
        for (index, offered) in asking.choices().iter().enumerate() {
            let arrow = if index == self.cursor { "❯ " } else { "  " };
            let check = match (several, self.checked.contains(&index)) {
                (false, _) => "",
                (true, true) => "[x] ",
                (true, false) => "[ ] ",
            };
            let row = format!("{arrow}{check}{}. {}", index + 1, offered_words(offered));
            rows.push(match index == self.cursor {
                true => Line::styled(row, Style::new().cyan()),
                false => Line::from(row),
            });
        }
        rows.push(Line::styled(keys(asking), Style::new().dim()));
        rows
    }

    /// How many rows its rows take, drawn `width` wide and broken at words; none where it is put
    /// aside.
    pub(super) fn height(&self, width: u16) -> usize {
        self.rows().iter().map(|line| wrapped_rows(line, width)).sum()
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

    /// Chooses the choice `index` - or, where several may be chosen, checks it, or takes its check
    /// away, the cursor moved to it.
    fn choose(&mut self, index: usize) -> Taken {
        if !self.asked.asking().multi_select() {
            return Taken::Typed(Typed::Chosen(vec![index]));
        }
        self.cursor = index;
        self.check(index);
        Taken::Kept
    }

    /// Checks the choice `index`, or takes its check away.
    fn check(&mut self, index: usize) {
        if !self.checked.remove(&index) {
            self.checked.insert(index);
        }
    }
}

/// What tells which keys answer `asking`.
fn keys(asking: &Asking) -> String {
    let has_choices = !asking.choices().is_empty();
    let keys = [
        (has_choices, "↑↓ choose"),
        (has_choices && asking.multi_select(), "Space checks"),
        (has_choices, "Enter answers"),
        (asking.takes_words(), "or type an answer"),
        (true, "Esc puts it aside"),
    ];
    let keys: Vec<&str> = keys.into_iter().filter(|(shown, _)| *shown).map(|(_, k)| k).collect();
    keys.join(" · ")
}
