//! Reading a bash command line for the rules on tool calls (issue 247): its simple commands, each
//! as the words that run - or that it cannot be judged. A small scanner of quoting, escapes and the
//! separators of commands, which calls whatever else it meets unjudgeable: what it answers is what
//! bash runs, or nothing.
//!
//! It does not see a program reached by another path (`/bin/rm`) or through another (`env rm`,
//! `sh -c "rm"`): a rule on commands compares words, and the boundary is where the command runs.

use std::iter::Peekable;
use std::mem::take;
use std::str::Chars;

/// The longest command line that is judged, in characters.
pub(crate) const MAX_COMMAND_LINE_CHARS: usize = 10_000;

/// bash's reserved words: a simple command beginning with one is part of a compound command - a
/// loop, a condition - whose parts its separators do not tell apart (`do rm x` of
/// `while true; do rm x; done`).
const RESERVED_WORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "select", "function", "time", "coproc", "!", "[[", "]]",
];

/// A word of a simple command, its quoting and escapes removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Word {
    text: String,
    /// Whether it holds a glob (`*`, `?`, `[`) or a `~` unquoted: the word that runs may be
    /// another.
    expands: bool,
}

impl Word {
    /// The text it runs as; `None` if it expands.
    pub(crate) fn literal(&self) -> Option<&str> {
        (!self.expands).then_some(self.text.as_str())
    }

    /// Whether it runs as `text`.
    pub(crate) fn is(&self, text: &str) -> bool {
        self.literal() == Some(text)
    }
}

/// A command line that cannot be judged: bash would run other words than those written, or do
/// more than run them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Unjudgeable;

/// The simple commands of `line`, each as its words, in the order written. The separators of
/// commands are `;`, `&&`, `||`, `|` and a line break. Unjudgeable are:
///
/// - substitution and expansion (`$`, a backtick, also within double quotes), redirection (`<`,
///   `>`), a background job (`&` alone, `|&`), subshells and groups (`(`, `)`, `{`, `}`);
/// - a simple command beginning with an assignment (`NAME=value`, `NAME+=value`), with one of
///   bash's reserved words, or with a word that expands;
/// - an unclosed quote, a trailing backslash, a separator with no command before or after it, a
///   line without any command, and one longer than `MAX_COMMAND_LINE_CHARS`.
///
/// A `#` is read as a word, not as the start of a comment: what follows it counts as commands,
/// which judges more strictly than bash runs.
pub(crate) fn simple_commands(line: &str) -> Result<Vec<Vec<Word>>, Unjudgeable> {
    if line.chars().count() > MAX_COMMAND_LINE_CHARS {
        return Err(Unjudgeable);
    }
    let mut scanner = Scanner::default();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => scanner.end_word()?,
            ';' | '\n' => scanner.end_command(false)?,
            '&' if chars.next_if_eq(&'&').is_some() => scanner.end_command(true)?,
            '|' if chars.next_if_eq(&'&').is_some() => return Err(Unjudgeable),
            '|' => {
                chars.next_if_eq(&'|');
                scanner.end_command(true)?;
            }
            '\'' => scanner.single_quoted(&mut chars)?,
            '"' => scanner.double_quoted(&mut chars)?,
            '\\' => match chars.next() {
                // A line continued: the break is removed.
                Some('\n') => {}
                Some(c) => scanner.push_quoted(c),
                None => return Err(Unjudgeable),
            },
            '$' | '`' | '<' | '>' | '&' | '(' | ')' | '{' | '}' => return Err(Unjudgeable),
            '*' | '?' | '[' | '~' => scanner.push_expanding(c),
            '=' => scanner.push_equals()?,
            c => scanner.push_plain(c),
        }
    }
    scanner.finish()
}

/// Whether `words` begin with `prefix`, word by word.
pub(crate) fn begins_with(words: &[Word], prefix: &[String]) -> bool {
    words.len() >= prefix.len() && words.iter().zip(prefix).all(|(word, text)| word.is(text))
}

/// Where `simple_commands` is in a line.
#[derive(Default)]
struct Scanner {
    commands: Vec<Vec<Word>>,
    /// The words of the command being read.
    words: Vec<Word>,
    /// The word being read, once one is begun - an empty one by `''`.
    word: Option<Word>,
    /// Whether the word read so far could be the name of an assignment: unquoted letters, digits
    /// and `_`, not beginning with a digit, and possibly a final `+`.
    name: NameState,
    /// Whether the last command ended in `&&`, `||` or `|`, which another must follow.
    continued: bool,
}

#[derive(Default, PartialEq)]
enum NameState {
    #[default]
    Name,
    NamePlus,
    No,
}

impl Scanner {
    fn word(&mut self) -> &mut Word {
        self.word.get_or_insert_with(|| Word { text: String::new(), expands: false })
    }

    fn push_plain(&mut self, c: char) {
        let empty = self.word.as_ref().is_none_or(|word| word.text.is_empty());
        self.name = match self.name {
            NameState::Name
                if c == '_' || c.is_ascii_alphabetic() || (c.is_ascii_digit() && !empty) =>
            {
                NameState::Name
            }
            NameState::Name if c == '+' && !empty => NameState::NamePlus,
            _ => NameState::No,
        };
        self.word().text.push(c);
    }

    fn push_quoted(&mut self, c: char) {
        self.name = NameState::No;
        self.word().text.push(c);
    }

    fn push_expanding(&mut self, c: char) {
        self.name = NameState::No;
        let word = self.word();
        word.text.push(c);
        word.expands = true;
    }

    /// An `=` unquoted: an assignment if it ends a name at the start of a command.
    fn push_equals(&mut self) -> Result<(), Unjudgeable> {
        let named = self.word.as_ref().is_some_and(|word| !word.text.is_empty());
        if named && self.name != NameState::No && self.words.is_empty() {
            return Err(Unjudgeable);
        }
        self.push_plain('=');
        Ok(())
    }

    /// The rest of a single-quoted part, after its `'`: everything literal up to the next `'`.
    fn single_quoted(&mut self, chars: &mut Peekable<Chars>) -> Result<(), Unjudgeable> {
        self.name = NameState::No;
        self.word();
        loop {
            match chars.next() {
                Some('\'') => return Ok(()),
                Some(c) => self.push_quoted(c),
                None => return Err(Unjudgeable),
            }
        }
    }

    /// The rest of a double-quoted part, after its `"`: literal but for expansions, which are
    /// unjudgeable, and a backslash, which escapes only what it would expand or end.
    fn double_quoted(&mut self, chars: &mut Peekable<Chars>) -> Result<(), Unjudgeable> {
        self.name = NameState::No;
        self.word();
        loop {
            match chars.next() {
                Some('"') => return Ok(()),
                Some('$' | '`') | None => return Err(Unjudgeable),
                Some('\\') => match chars.next() {
                    Some('\n') => {}
                    Some(c @ ('$' | '`' | '"' | '\\')) => self.push_quoted(c),
                    Some(c) => {
                        self.push_quoted('\\');
                        self.push_quoted(c);
                    }
                    None => return Err(Unjudgeable),
                },
                Some(c) => self.push_quoted(c),
            }
        }
    }

    fn end_word(&mut self) -> Result<(), Unjudgeable> {
        self.name = NameState::Name;
        let Some(word) = self.word.take() else { return Ok(()) };
        if self.words.is_empty() && (word.expands || RESERVED_WORDS.contains(&word.text.as_str())) {
            return Err(Unjudgeable);
        }
        self.words.push(word);
        Ok(())
    }

    /// Ends the command being read at a separator: `strong` for `&&`, `||` and `|`, which must
    /// follow a command. An empty command between two `;` or lines is passed over.
    fn end_command(&mut self, strong: bool) -> Result<(), Unjudgeable> {
        self.end_word()?;
        if self.words.is_empty() {
            return if strong { Err(Unjudgeable) } else { Ok(()) };
        }
        self.commands.push(take(&mut self.words));
        self.continued = strong;
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<Vec<Word>>, Unjudgeable> {
        self.end_command(false)?;
        if self.continued || self.commands.is_empty() {
            return Err(Unjudgeable);
        }
        Ok(self.commands)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The words of the simple commands of `line`, `None` if it is unjudgeable.
    fn read(line: &str) -> Option<Vec<Vec<String>>> {
        let commands = simple_commands(line).ok()?;
        Some(
            commands
                .into_iter()
                .map(|words| words.into_iter().map(|word| word.text).collect())
                .collect(),
        )
    }

    fn commands(commands: &[&[&str]]) -> Option<Vec<Vec<String>>> {
        Some(
            commands
                .iter()
                .map(|words| words.iter().map(|word| word.to_string()).collect())
                .collect(),
        )
    }

    #[test]
    fn a_line_is_read_as_its_simple_commands() {
        let cases: &[(&str, &[&[&str]])] = &[
            ("ls", &[&["ls"]]),
            ("  ls   -la\tsrc  ", &[&["ls", "-la", "src"]]),
            ("git status && git diff", &[&["git", "status"], &["git", "diff"]]),
            ("make || echo failed; ls", &[&["make"], &["echo", "failed"], &["ls"]]),
            ("cat a | grep x | wc -l", &[&["cat", "a"], &["grep", "x"], &["wc", "-l"]]),
            ("ls\nrm x\n", &[&["ls"], &["rm", "x"]]),
            ("ls;;; ls", &[&["ls"], &["ls"]]),
            ("ls &&\n  rm x", &[&["ls"], &["rm", "x"]]),
            ("ls \\\n -la", &[&["ls", "-la"]]),
            ("grep 'a b' \"c d\" e\\ f", &[&["grep", "a b", "c d", "e f"]]),
            ("echo '$HOME `x` \\'", &[&["echo", "$HOME `x` \\"]]),
            ("echo \"a\\\"b\\\\c\\d\"", &[&["echo", "a\"b\\c\\d"]]),
            ("echo '' x", &[&["echo", "", "x"]]),
            ("echo \\; \\$x \\&\\&", &[&["echo", ";", "$x", "&&"]]),
            ("git commit -m 'a; rm x'", &[&["git", "commit", "-m", "a; rm x"]]),
            ("ls a=b", &[&["ls", "a=b"]]),
            ("\"FOO\"=1 ls", &[&["FOO=1", "ls"]]),
            ("=x", &[&["=x"]]),
            ("ls *.rs ~/x", &[&["ls", "*.rs", "~/x"]]),
            ("ls # ; rm x", &[&["ls", "#"], &["rm", "x"]]),
        ];
        for (line, expected) in cases {
            assert_eq!(read(line), commands(expected), "{line:?}");
        }
    }

    #[test]
    fn a_line_bash_would_run_otherwise_than_written_is_unjudgeable() {
        let lines = [
            "ls $HOME",
            "ls ${x}",
            "ls $(rm x)",
            "ls `rm x`",
            "echo \"$(rm x)\"",
            "echo \"`rm x`\"",
            "echo $'x'",
            "ls > f",
            "ls 2>&1",
            "cat < f",
            "cat <<EOF",
            "diff <(ls a) <(ls b)",
            "ls &> f",
            "sleep 9 &",
            "ls |& cat",
            "(rm x)",
            "{ rm x; }",
            "f() { rm x; }",
            "FOO=1 ls",
            "FOO+=1 ls",
            "_x1=y ls",
            "FOO='a b' ls",
            "a[1]=x rm y",
            "while true; do rm x; done",
            "if true; then rm x; fi",
            "for f in a; do rm x; done",
            "! rm x",
            "time rm x",
            "[[ -f x ]] && rm x",
            "coproc rm x",
            "function f",
            "case x in esac",
            "*.sh",
            "~/bin/x",
            "ls; rm*",
            "echo 'x",
            "echo \"x",
            "ls \\",
            "&& ls",
            "ls &&",
            "ls ||",
            "ls |",
            "ls && && rm x",
            "ls | | rm x",
            "",
            "   ",
            ";",
            "\n\n",
        ];
        for line in lines {
            assert_eq!(simple_commands(line), Err(Unjudgeable), "{line:?}");
        }
        assert!(simple_commands(&"x".repeat(MAX_COMMAND_LINE_CHARS)).is_ok());
        assert_eq!(
            simple_commands(&"x".repeat(MAX_COMMAND_LINE_CHARS + 1)),
            Err(Unjudgeable),
            "too long"
        );
    }

    #[test]
    fn a_word_that_expands_is_never_the_word_written() {
        let prefix = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        let [command] = simple_commands("ls -l *.rs '*.md' \\~").unwrap().try_into().unwrap();
        assert!(begins_with(&command, &prefix(&["ls", "-l"])));
        assert!(!begins_with(&command, &prefix(&["ls", "-l", "*.rs"])), "a glob unquoted");
        assert!(command[3].is("*.md") && command[4].is("~"), "quoted, escaped");
        assert!(
            begins_with(&command, &prefix(&[]))
                && !begins_with(&command[..1], &prefix(&["ls", "-l"]))
        );
    }
}
