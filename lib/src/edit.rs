//! Local line editing with readline-style keys and a persistent history.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const MAX_HISTORY: usize = 1000;

/// Command history, optionally backed by a file that every new entry is appended to.
#[derive(Debug, Default)]
pub struct History {
    entries: Vec<String>,
    path: Option<PathBuf>,
}

impl History {
    /// Loads the last entries from `path`; a missing file is an empty history.
    pub fn load(path: Option<&Path>) -> io::Result<Self> {
        let mut h = History {
            entries: Vec::new(),
            path: path.map(Path::to_path_buf),
        };
        let Some(p) = path else { return Ok(h) };
        if !p.exists() {
            return Ok(h);
        }
        let text = fs::read_to_string(p)?;
        h.entries = text
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect();
        let excess = h.entries.len().saturating_sub(MAX_HISTORY);
        h.entries.drain(..excess);
        Ok(h)
    }

    /// Adds `line` unless it is blank or repeats the newest entry.
    pub fn push(&mut self, line: &str) -> io::Result<()> {
        if line.trim().is_empty() || self.entries.last().is_some_and(|l| l == line) {
            return Ok(());
        }
        self.entries.push(line.to_string());
        if self.entries.len() > MAX_HISTORY {
            self.entries.remove(0);
        }
        let Some(p) = &self.path else { return Ok(()) };
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut f = OpenOptions::new().create(true).append(true).open(p)?;
        writeln!(f, "{line}")
    }

    /// Entries, oldest first.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }
}

/// Default history file: `$XDG_STATE_HOME/fterm/history` or `~/.local/state/fterm/history`.
pub fn default_history_path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state.join("fterm/history"))
}

/// What the caller should do after a key.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Line changed: redraw it.
    Redraw,
    /// Enter: the finished line.
    Submit(String),
    /// ctrl-c: line discarded.
    Cancel,
    /// Key has no meaning here.
    Ignored,
}

#[derive(Debug, Default)]
struct Search {
    query: String,
    hit: Option<usize>,
    saved: (Vec<char>, usize),
}

/// One line being edited.
#[derive(Debug, Default)]
pub struct Editor {
    buf: Vec<char>,
    cur: usize,
    kill: Vec<char>,
    nav: Option<usize>,
    draft: Vec<char>,
    search: Option<Search>,
}

impl Editor {
    /// The current text.
    pub fn line(&self) -> String {
        self.buf.iter().collect()
    }

    /// True if nothing is typed and no search is open.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty() && self.search.is_none()
    }

    /// Inserts text at the cursor (pastes).
    pub fn insert(&mut self, s: &str) {
        for c in s.chars().filter(|c| !c.is_control()) {
            self.buf.insert(self.cur, c);
            self.cur += 1;
        }
    }

    /// Forgets the line and any history navigation.
    pub fn clear(&mut self) {
        self.buf.clear();
        self.cur = 0;
        self.nav = None;
        self.search = None;
    }

    /// Terminal bytes that redraw the line after `prompt`, cursor placed.
    pub fn render(&self, prompt: &str) -> String {
        if let Some(s) = &self.search {
            let hit: String = self.buf.iter().collect();
            return format!("\r\x1b[K(reverse-i-search)`{}': {hit}", s.query);
        }
        let back = self.buf.len() - self.cur;
        let mut out = format!("\r\x1b[K{prompt}{}", self.line());
        if back > 0 {
            out.push_str(&format!("\x1b[{back}D"));
        }
        out
    }

    /// Applies one key.
    pub fn key(&mut self, k: &KeyEvent, h: &History) -> Action {
        if self.search.is_some() {
            return self.search_key(k, h);
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match (k.code, ctrl, alt) {
            (KeyCode::Enter, _, _) | (KeyCode::Char('m' | 'j'), true, _) => return self.submit(),
            (KeyCode::Char('c'), true, _) => {
                self.clear();
                return Action::Cancel;
            }
            (KeyCode::Char('r'), true, _) => {
                let saved = (self.buf.clone(), self.cur);
                self.search = Some(Search {
                    saved,
                    ..Default::default()
                });
            }
            (KeyCode::Char('a'), true, _) | (KeyCode::Home, _, _) => self.cur = 0,
            (KeyCode::Char('e'), true, _) | (KeyCode::End, _, _) => self.cur = self.buf.len(),
            (KeyCode::Char('b'), true, _) | (KeyCode::Left, false, false) => {
                self.cur = self.cur.saturating_sub(1)
            }
            (KeyCode::Char('f'), true, _) | (KeyCode::Right, false, false) => {
                self.cur = (self.cur + 1).min(self.buf.len())
            }
            (KeyCode::Char('b'), _, true) | (KeyCode::Left, _, _) => self.cur = self.word_left(),
            (KeyCode::Char('f'), _, true) | (KeyCode::Right, _, _) => self.cur = self.word_right(),
            (KeyCode::Char('h'), true, _) | (KeyCode::Backspace, _, _) => {
                if self.cur == 0 {
                    return Action::Ignored;
                }
                self.cur -= 1;
                self.buf.remove(self.cur);
            }
            (KeyCode::Char('d'), true, _) | (KeyCode::Delete, _, _) => {
                if self.cur == self.buf.len() {
                    return Action::Ignored;
                }
                self.buf.remove(self.cur);
            }
            (KeyCode::Char('k'), true, _) => self.kill = self.buf.split_off(self.cur),
            (KeyCode::Char('u'), true, _) => {
                self.kill = self.buf.drain(..self.cur).collect();
                self.cur = 0;
            }
            (KeyCode::Char('w'), true, _) => {
                let from = self.word_left();
                self.kill = self.buf.drain(from..self.cur).collect();
                self.cur = from;
            }
            (KeyCode::Char('y'), true, _) => {
                let k: String = self.kill.iter().collect();
                self.insert(&k);
            }
            (KeyCode::Char('p'), true, _) | (KeyCode::Up, _, _) => return self.history(h, -1),
            (KeyCode::Char('n'), true, _) | (KeyCode::Down, _, _) => return self.history(h, 1),
            (KeyCode::Char(c), false, false) => self.insert(&c.to_string()),
            _ => return Action::Ignored,
        }
        Action::Redraw
    }

    fn submit(&mut self) -> Action {
        let line = self.line();
        self.clear();
        Action::Submit(line)
    }

    fn history(&mut self, h: &History, dir: isize) -> Action {
        let n = h.entries().len();
        let next = match (self.nav, dir < 0) {
            (None, true) if n > 0 => {
                self.draft = self.buf.clone();
                Some(n - 1)
            }
            (None, _) => return Action::Ignored,
            (Some(0), true) => return Action::Ignored,
            (Some(i), true) => Some(i - 1),
            (Some(i), false) if i + 1 < n => Some(i + 1),
            (Some(_), false) => None,
        };
        self.nav = next;
        self.buf = match next {
            Some(i) => h.entries()[i].chars().collect(),
            None => std::mem::take(&mut self.draft),
        };
        self.cur = self.buf.len();
        Action::Redraw
    }

    fn search_key(&mut self, k: &KeyEvent, h: &History) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let s = self.search.as_mut().expect("search open");
        match (k.code, ctrl) {
            (KeyCode::Char('r'), true) => {
                let before = s.hit.unwrap_or(h.entries().len());
                s.hit = find_back(h, &s.query, before).or(s.hit);
            }
            (KeyCode::Char('g' | 'c'), true) | (KeyCode::Esc, _) => {
                let s = self.search.take().expect("search open");
                (self.buf, self.cur) = s.saved;
                return Action::Redraw;
            }
            (KeyCode::Backspace, _) => {
                s.query.pop();
                s.hit = find_back(h, &s.query, h.entries().len());
            }
            (KeyCode::Char(c), false) => {
                s.query.push(c);
                let before = s.hit.map_or(h.entries().len(), |i| i + 1);
                s.hit = find_back(h, &s.query, before);
            }
            (KeyCode::Enter, _) => {
                self.search = None;
                return self.submit();
            }
            _ => {
                self.search = None;
                self.cur = self.buf.len();
                return Action::Redraw;
            }
        }
        let s = self.search.as_ref().expect("search open");
        self.buf = match s.hit {
            Some(i) => h.entries()[i].chars().collect(),
            None => Vec::new(),
        };
        self.cur = self.buf.len();
        Action::Redraw
    }

    fn word_left(&self) -> usize {
        let mut i = self.cur;
        while i > 0 && self.buf[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.buf[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    fn word_right(&self) -> usize {
        let mut i = self.cur;
        while i < self.buf.len() && self.buf[i].is_whitespace() {
            i += 1;
        }
        while i < self.buf.len() && !self.buf[i].is_whitespace() {
            i += 1;
        }
        i
    }
}

fn find_back(h: &History, q: &str, before: usize) -> Option<usize> {
    h.entries()[..before.min(h.entries().len())]
        .iter()
        .rposition(|e| e.contains(q))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn c(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    fn typed(s: &str) -> Vec<KeyEvent> {
        s.chars().map(|ch| k(KeyCode::Char(ch))).collect()
    }

    fn feed(e: &mut Editor, h: &History, keys: &[KeyEvent]) -> Action {
        keys.iter().fold(Action::Ignored, |_, key| e.key(key, h))
    }

    fn hist(lines: &[&str]) -> History {
        History {
            entries: lines.iter().map(|s| s.to_string()).collect(),
            path: None,
        }
    }

    #[test]
    fn editing_keys() {
        let h = History::default();
        let cases: Vec<(Vec<KeyEvent>, &str)> = vec![
            (typed("1 2 +"), "1 2 +"),
            ([typed("abc"), vec![k(KeyCode::Backspace)]].concat(), "ab"),
            ([typed("bc"), vec![c('a')], typed("a")].concat(), "abc"),
            (
                [typed("ac"), vec![k(KeyCode::Left)], typed("b")].concat(),
                "abc",
            ),
            (
                [typed("abc"), vec![k(KeyCode::Home), c('d')]].concat(),
                "bc",
            ),
            ([typed("abc def"), vec![c('w')]].concat(), "abc "),
            (
                [typed("abc def"), vec![c('a'), c('f'), c('k')]].concat(),
                "a",
            ),
            ([typed("abc def"), vec![c('u')]].concat(), ""),
            (
                [typed("ab cd"), vec![c('w'), c('a'), c('y')]].concat(),
                "cdab ",
            ),
            (
                [
                    typed("one two"),
                    vec![KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT)],
                    typed("X"),
                ]
                .concat(),
                "one Xtwo",
            ),
            (vec![k(KeyCode::Backspace), k(KeyCode::Delete)], ""),
        ];
        for (keys, want) in cases {
            let mut e = Editor::default();
            feed(&mut e, &h, &keys);
            assert_eq!(e.line(), want, "{keys:?}");
        }
    }

    #[test]
    fn submit_and_cancel() {
        let h = History::default();
        let mut e = Editor::default();
        assert_eq!(
            feed(&mut e, &h, &[typed("x"), vec![k(KeyCode::Enter)]].concat()),
            Action::Submit("x".into())
        );
        assert!(e.is_empty());
        assert_eq!(
            feed(&mut e, &h, &[typed("y"), vec![c('c')]].concat()),
            Action::Cancel
        );
        assert!(e.is_empty());
    }

    #[test]
    fn history_navigation() {
        let h = hist(&["first", "second"]);
        let mut e = Editor::default();
        e.insert("draft");
        assert_eq!(e.key(&k(KeyCode::Up), &h), Action::Redraw);
        assert_eq!(e.line(), "second");
        e.key(&k(KeyCode::Up), &h);
        assert_eq!(e.line(), "first");
        assert_eq!(e.key(&k(KeyCode::Up), &h), Action::Ignored);
        e.key(&k(KeyCode::Down), &h);
        e.key(&k(KeyCode::Down), &h);
        assert_eq!(e.line(), "draft");
        assert_eq!(e.key(&k(KeyCode::Down), &h), Action::Ignored);
    }

    #[test]
    fn reverse_search() {
        let h = hist(&["decode", "1 2 +", "log", "#r dcf77.fs"]);
        let mut e = Editor::default();
        feed(&mut e, &h, &[vec![c('r')], typed("o")].concat());
        assert_eq!(e.line(), "log");
        e.key(&c('r'), &h);
        assert_eq!(e.line(), "decode");
        e.key(&c('r'), &h);
        assert_eq!(e.line(), "decode", "no older match keeps the hit");
        assert!(e.render("").contains("reverse-i-search"));
        assert_eq!(
            e.key(&k(KeyCode::Enter), &h),
            Action::Submit("decode".into())
        );

        e.insert("keep");
        feed(
            &mut e,
            &h,
            &[vec![c('r')], typed("dcf"), vec![k(KeyCode::Esc)]].concat(),
        );
        assert_eq!(e.line(), "keep");

        feed(
            &mut e,
            &h,
            &[vec![c('r')], typed("dcf"), vec![k(KeyCode::Right)]].concat(),
        );
        assert_eq!(e.line(), "#r dcf77.fs");
    }

    #[test]
    fn render_places_cursor() {
        let mut e = Editor::default();
        e.insert("abc");
        e.key(&k(KeyCode::Left), &History::default());
        assert_eq!(e.render("> "), "\r\x1b[K> abc\x1b[1D");
    }

    #[test]
    fn history_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/history");
        let mut h = History::load(Some(&p)).unwrap();
        for l in ["a", "a", "", "b"] {
            h.push(l).unwrap();
        }
        assert_eq!(h.entries(), ["a", "b"]);
        let h = History::load(Some(&p)).unwrap();
        assert_eq!(h.entries(), ["a", "b"]);
        assert_eq!(History::load(None).unwrap().entries().len(), 0);
    }
}
