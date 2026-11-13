//! Full-screen fuzzy file picker for uploads.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fs;
use std::path::{Path, PathBuf};

const EXTENSIONS: &[&str] = &["fs", "fth", "4th", "f", "forth"];
const SKIP_DIRS: &[&str] = &["target", "node_modules", "result"];
const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 5000;

/// Forth sources below `roots`, shown relative to `cwd` where possible, sorted and unique.
pub fn collect(roots: &[PathBuf], cwd: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for r in roots {
        walk(r, 0, &mut out);
    }
    let mut names: Vec<String> = out
        .iter()
        .map(|p| p.strip_prefix(cwd).unwrap_or(p).display().to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() && depth < MAX_DEPTH && !SKIP_DIRS.contains(&name.as_str()) {
            walk(&p, depth + 1, out);
        } else if ft.is_file() && has_forth_ext(&p) {
            out.push(p);
        }
    }
}

fn has_forth_ext(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e))
}

/// Case-insensitive subsequence score of `query` in `cand`; `None` if it does not match.
/// Consecutive matches and matches at word starts (after `/ _ - .`) score higher.
pub fn score(query: &str, cand: &str) -> Option<i64> {
    let c: Vec<char> = cand.to_lowercase().chars().collect();
    let mut total = 0i64;
    let mut pos = 0usize;
    let mut prev: Option<usize> = None;
    for q in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let i = pos + c[pos..].iter().position(|&x| x == q)?;
        total += 1;
        if prev.is_some_and(|p| p + 1 == i) {
            total += 5;
        }
        if i == 0 || matches!(c[i - 1], '/' | '_' | '-' | '.') {
            total += 8;
        }
        prev = Some(i);
        pos = i + 1;
    }
    Some(total * 10 - c.len() as i64)
}

/// Result of a key in the picker.
#[derive(Debug, PartialEq)]
pub enum Pick {
    /// State changed: redraw.
    Redraw,
    /// Closed: the chosen entry, or `None` if cancelled.
    Done(Option<String>),
}

/// Picker state: candidates, query, selection.
#[derive(Debug)]
pub struct Picker {
    title: String,
    items: Vec<String>,
    query: String,
    shown: Vec<usize>,
    sel: usize,
    top: usize,
}

impl Picker {
    /// Creates a picker over `items`, pre-filtered with `query`.
    pub fn new(title: &str, items: Vec<String>, query: &str) -> Self {
        let mut p = Self {
            title: title.into(),
            items,
            query: query.into(),
            shown: Vec::new(),
            sel: 0,
            top: 0,
        };
        p.refilter();
        p
    }

    /// The highlighted entry.
    pub fn selected(&self) -> Option<&str> {
        self.shown.get(self.sel).map(|&i| self.items[i].as_str())
    }

    /// Applies one key; `rows` is the list height, for paging.
    pub fn key(&mut self, k: &KeyEvent, rows: usize) -> Pick {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match (k.code, ctrl) {
            (KeyCode::Enter, _) => return Pick::Done(self.selected().map(String::from)),
            (KeyCode::Esc, _) | (KeyCode::Char('c' | 'g'), true) => return Pick::Done(None),
            (KeyCode::Up, _) | (KeyCode::BackTab, _) | (KeyCode::Char('p' | 'k'), true) => {
                self.move_by(-1)
            }
            (KeyCode::Down, _) | (KeyCode::Tab, _) | (KeyCode::Char('n' | 'j'), true) => {
                self.move_by(1)
            }
            (KeyCode::PageUp, _) => self.move_by(-(rows.max(1) as isize)),
            (KeyCode::PageDown, _) => self.move_by(rows.max(1) as isize),
            (KeyCode::Char('u'), true) => {
                self.query.clear();
                self.refilter();
            }
            (KeyCode::Backspace, _) => {
                self.query.pop();
                self.refilter();
            }
            (KeyCode::Char(c), false) => {
                self.query.push(c);
                self.refilter();
            }
            _ => {}
        }
        self.scroll(rows);
        Pick::Redraw
    }

    fn move_by(&mut self, d: isize) {
        let n = self.shown.len();
        if n == 0 {
            return;
        }
        self.sel = (self.sel as isize + d).clamp(0, n as isize - 1) as usize;
    }

    fn scroll(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.sel < self.top {
            self.top = self.sel;
        } else if self.sel >= self.top + rows {
            self.top = self.sel + 1 - rows;
        }
    }

    fn refilter(&mut self) {
        let mut scored: Vec<(i64, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, s)| score(&self.query, s).map(|sc| (sc, i)))
            .collect();
        if !self.query.trim().is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        self.shown = scored.into_iter().map(|(_, i)| i).collect();
        self.sel = 0;
        self.top = 0;
    }

    /// Full-screen frame for a `w` x `h` terminal, with an optional preview of the selection.
    pub fn render(&self, w: usize, h: usize, preview: Option<&str>) -> String {
        let rows = h.saturating_sub(2).max(1);
        let split = preview.is_some() && w >= 100;
        let left_w = if split { w / 2 - 1 } else { w };
        let header = format!(
            " {} > {}\x1b[7m \x1b[0m  ({}/{})",
            self.title,
            self.query,
            self.shown.len(),
            self.items.len()
        );
        let mut lines = vec![header];
        let prev: Vec<&str> = preview.map(|p| p.lines().collect()).unwrap_or_default();
        for r in 0..rows {
            let idx = self.top + r;
            let mut left = match self.shown.get(idx) {
                Some(&i) => {
                    let name = fit(&self.items[i], left_w.saturating_sub(2));
                    let pad = left_w.saturating_sub(2 + name.chars().count());
                    if idx == self.sel {
                        format!("\x1b[7m> {name}{}\x1b[0m", " ".repeat(pad))
                    } else {
                        format!("  {name}{}", " ".repeat(pad))
                    }
                }
                None => " ".repeat(left_w),
            };
            if split {
                let p = prev.get(r).map_or(String::new(), |l| {
                    fit(&l.replace('\t', "    "), w - left_w - 3)
                });
                left.push_str(&format!(" \x1b[2m│\x1b[0m {p}"));
            }
            lines.push(left);
        }
        lines.push(fit(
            " ↑↓ select   type to filter   Enter choose   Esc cancel",
            w,
        ));
        format!("\x1b[H\x1b[2J{}", lines.join("\r\n"))
    }
}

fn fit(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        return s.to_string();
    }
    let tail: String = s
        .chars()
        .rev()
        .take(w.saturating_sub(1))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn items() -> Vec<String> {
        [
            "dcf77-forth/dcf77.fs",
            "dcf77-forth/pins.fs",
            "lib/util.fs",
            "autoexec.fs",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn score_cases() {
        assert_eq!(score("", "abc"), Some(-3));
        assert!(score("pins", "dcf77-forth/pins.fs").is_some());
        assert!(score("xyz", "dcf77-forth/pins.fs").is_none());
        assert!(
            score("PINS", "dcf77-forth/pins.fs").is_some(),
            "case-insensitive"
        );
        assert!(
            score("pf", "pins.fs") > score("pf", "xpxxxf"),
            "word starts score higher"
        );
        assert!(
            score("dcf", "xdcfx") > score("dcf", "xdxcxfx"),
            "consecutive scores higher"
        );
    }

    #[test]
    fn filter_and_select() {
        let mut p = Picker::new("upload", items(), "");
        assert_eq!(
            p.selected(),
            Some("dcf77-forth/dcf77.fs"),
            "empty query keeps order"
        );
        for c in "pin".chars() {
            p.key(&k(KeyCode::Char(c)), 10);
        }
        assert_eq!(p.selected(), Some("dcf77-forth/pins.fs"));
        assert_eq!(
            p.key(&k(KeyCode::Enter), 10),
            Pick::Done(Some("dcf77-forth/pins.fs".into()))
        );

        let mut p = Picker::new("upload", items(), "zzz");
        assert_eq!(p.selected(), None);
        assert_eq!(p.key(&k(KeyCode::Enter), 10), Pick::Done(None));
        p.key(&k(KeyCode::Backspace), 10);
        p.key(
            &KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            10,
        );
        assert_eq!(p.shown.len(), 4);
        assert_eq!(p.key(&k(KeyCode::Esc), 10), Pick::Done(None));
    }

    #[test]
    fn navigation_clamps_and_scrolls() {
        let mut p = Picker::new("upload", items(), "");
        p.key(&k(KeyCode::Up), 2);
        assert_eq!(p.sel, 0);
        for _ in 0..10 {
            p.key(&k(KeyCode::Down), 2);
        }
        assert_eq!((p.sel, p.top), (3, 2));
        p.key(&k(KeyCode::PageUp), 2);
        assert_eq!((p.sel, p.top), (1, 1));
    }

    #[test]
    fn render_frame() {
        let p = Picker::new("upload", items(), "");
        let f = p.render(120, 6, Some("\\ DCF77 receiver\n$D0 constant x"));
        assert!(f.starts_with("\x1b[H\x1b[2J"));
        let lines: Vec<&str> = f.split("\r\n").collect();
        assert_eq!(lines.len(), 6);
        assert!(lines[0].contains("(4/4)"));
        assert!(
            lines[1].contains("\x1b[7m> dcf77-forth/dcf77.fs")
                && lines[1].contains("DCF77 receiver")
        );
        assert!(
            !p.render(60, 6, Some("x")).contains('│'),
            "no preview when narrow"
        );
        assert_eq!(fit("abcdef", 4), "…def");
    }

    #[test]
    fn collect_skips_hidden_and_build_dirs() {
        let d = tempfile::tempdir().unwrap();
        for f in ["a.fs", "sub/b.4th", ".hidden/c.fs", "target/d.fs", "e.txt"] {
            let p = d.path().join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "").unwrap();
        }
        let got = collect(&[d.path().to_path_buf(), d.path().join("sub")], d.path());
        assert_eq!(got, ["a.fs", "sub/b.4th"]);
    }
}
