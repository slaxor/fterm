//! Keyboard input: reader thread and key-to-byte translation.

use crate::session::Ev;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Handle to the key reader thread; pausing hands the terminal to a child process.
pub struct Keys {
    paused: Arc<AtomicBool>,
    idle: Arc<AtomicBool>,
}

impl Keys {
    /// Starts reading terminal events into `tx`.
    pub fn spawn(tx: Sender<Ev>) -> Self {
        let paused = Arc::new(AtomicBool::new(false));
        let idle = Arc::new(AtomicBool::new(false));
        let (p, i) = (paused.clone(), idle.clone());
        thread::spawn(move || loop {
            i.store(false, SeqCst);
            if p.load(SeqCst) {
                i.store(true, SeqCst);
                thread::sleep(Duration::from_millis(20));
                continue;
            }
            if !event::poll(Duration::from_millis(50)).unwrap_or(false) {
                continue;
            }
            let ev = match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => Ev::Key(k),
                Ok(Event::Paste(s)) => Ev::Paste(s),
                Ok(_) => continue,
                Err(e) => Ev::Fatal(format!("terminal: {e}")),
            };
            if tx.send(ev).is_err() {
                return;
            }
        });
        Self { paused, idle }
    }

    /// Stops reading until [`Keys::resume`]; returns once the thread is idle.
    pub fn pause(&self) {
        self.paused.store(true, SeqCst);
        while !self.idle.load(SeqCst) {
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Resumes reading.
    pub fn resume(&self) {
        self.paused.store(false, SeqCst);
    }
}

/// True for `ctrl-<c>`.
pub fn is_ctrl(k: &KeyEvent, c: char) -> bool {
    k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char(c)
}

/// Name of a function key as used by `bind`, e.g. `F5`.
pub fn fkey_name(k: &KeyEvent) -> Option<String> {
    match k.code {
        KeyCode::F(n) => Some(format!("F{n}")),
        _ => None,
    }
}

/// Bytes a terminal would send for `k`.
pub fn key_bytes(k: &KeyEvent) -> Option<Vec<u8>> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let mut out = match k.code {
        KeyCode::Char(c) if ctrl => vec![ctrl_byte(c)?],
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Backspace => vec![8],
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::Esc => vec![27],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        _ => return None,
    };
    if k.modifiers.contains(KeyModifiers::ALT) {
        out.insert(0, 27);
    }
    Some(out)
}

fn ctrl_byte(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | ' ' => Some(0),
        '[' => Some(27),
        '\\' => Some(28),
        ']' => Some(29),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn key_bytes_cases() {
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        let cases: &[(KeyEvent, Option<&[u8]>)] = &[
            (key(KeyCode::Char('a'), none), Some(b"a")),
            (key(KeyCode::Char('ä'), none), Some("ä".as_bytes())),
            (key(KeyCode::Char('c'), ctrl), Some(&[3])),
            (key(KeyCode::Char('T'), ctrl), Some(&[20])),
            (key(KeyCode::Char('1'), ctrl), None),
            (key(KeyCode::Enter, none), Some(b"\r")),
            (key(KeyCode::Backspace, none), Some(&[8])),
            (key(KeyCode::Up, none), Some(b"\x1b[A")),
            (key(KeyCode::Char('x'), KeyModifiers::ALT), Some(b"\x1bx")),
            (key(KeyCode::F(5), none), None),
        ];
        for (k, want) in cases {
            assert_eq!(key_bytes(k).as_deref(), *want, "{k:?}");
        }
    }

    #[test]
    fn names() {
        assert_eq!(
            fkey_name(&key(KeyCode::F(5), KeyModifiers::NONE)).as_deref(),
            Some("F5")
        );
        assert_eq!(
            fkey_name(&key(KeyCode::Char('a'), KeyModifiers::NONE)),
            None
        );
        assert!(is_ctrl(
            &key(KeyCode::Char('t'), KeyModifiers::CONTROL),
            't'
        ));
        assert!(!is_ctrl(&key(KeyCode::Char('t'), KeyModifiers::NONE), 't'));
    }
}
