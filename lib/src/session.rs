//! Interactive session: terminal passthrough, escape commands, `#` directives, reconnect.

use crate::ctl::{self, Cmd, Req};
use crate::edit::{Action, Editor, History};
use crate::keys::{self, Keys};
use crate::link::Target;
use crate::vm::Vm;
use anyhow::{bail, Context, Result};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, KeyCode, KeyEvent};
use crossterm::{execute, terminal};
use serialport::SerialPort;
use std::collections::VecDeque;
use std::io::{stdout, Read, Stdout, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// Everything the main loop reacts to.
pub enum Ev {
    Serial(Vec<u8>),
    Lost(String),
    Key(KeyEvent),
    Paste(String),
    Ctl(Req),
    Note(String),
    Fatal(String),
}

const HELP: &str = "\
ctrl-t q  quit              ctrl-t :  host Forth prompt
ctrl-t u  upload file       ctrl-t r  repeat last upload
ctrl-t e  edit last error   ctrl-t c  forget #require'd files
ctrl-t l  raw passthrough   ctrl-t ctrl-t  send ctrl-t
ctrl-t h  this help
ctrl-t <c> / F1..F12  bound macros (bind in .fterm.fs)
line editing: up/down ctrl-r ctrl-a/e/k/u/w/y alt-b/f
after Enter keys go raw to the target until it prompts
ctrl-c during an upload aborts it
#i file  #r file  #e [file]  #f forth";

/// A `#` line typed at the start of a line.
#[derive(Debug, PartialEq)]
pub enum Hash {
    Include(String),
    Require(String),
    Edit(Option<String>),
    Forth(String),
    Pass,
}

/// Classifies a `#` line; anything unknown goes to the target unchanged.
pub fn parse_hash(line: &str) -> Hash {
    let (word, arg) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let arg = arg.trim();
    match (word, arg.is_empty()) {
        ("#i" | "#include", false) => Hash::Include(arg.into()),
        ("#r" | "#require", false) => Hash::Require(arg.into()),
        ("#e" | "#edit", true) => Hash::Edit(None),
        ("#e" | "#edit", false) => Hash::Edit(Some(arg.into())),
        ("#f", false) => Hash::Forth(arg.into()),
        _ => Hash::Pass,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum LineKind {
    Target,
    Forth,
    Upload,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    /// Local line editing.
    Edit,
    /// Line sent; keys go raw to the target until it prompts.
    Busy,
    /// Raw passthrough until toggled off.
    Raw,
}

struct RawMode;

impl RawMode {
    fn on() -> Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(stdout(), EnableBracketedPaste)?;
        Ok(RawMode)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        if let Err(e) = execute!(stdout(), DisableBracketedPaste) {
            eprintln!("fterm: {e}");
        }
        if let Err(e) = terminal::disable_raw_mode() {
            eprintln!("fterm: {e}");
        }
    }
}

fn show(out: &mut Stdout, bytes: &[u8]) {
    let mut v = Vec::with_capacity(bytes.len() + 8);
    for &b in bytes {
        if b == b'\n' {
            v.push(b'\r');
        }
        v.push(b);
    }
    if let Err(e) = out.write_all(&v).and_then(|_| out.flush()) {
        eprintln!("fterm: stdout: {e}");
    }
}

struct Live<'a> {
    port: &'a mut Option<Box<dyn SerialPort>>,
    rx: &'a Receiver<Ev>,
    out: &'a mut Stdout,
    later: &'a mut VecDeque<Ev>,
}

impl Target for Live<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let p = self.port.as_mut().context("not connected")?;
        p.write_all(bytes)?;
        Ok(p.flush()?)
    }

    fn read(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(Vec::new());
            }
            match self.rx.recv_timeout(left) {
                Ok(Ev::Serial(b)) => {
                    show(self.out, &b);
                    return Ok(b);
                }
                Ok(Ev::Key(k)) if keys::is_ctrl(&k, 'c') => bail!("aborted"),
                Ok(Ev::Lost(e)) => {
                    *self.port = None;
                    bail!("disconnected: {e}");
                }
                Ok(Ev::Fatal(e)) => bail!("{e}"),
                Ok(ev) => self.later.push_back(ev),
                Err(RecvTimeoutError::Timeout) => return Ok(Vec::new()),
                Err(RecvTimeoutError::Disconnected) => bail!("event channel closed"),
            }
        }
    }

    fn emit(&mut self, text: &str) {
        show(self.out, text.as_bytes());
    }
}

/// The interactive terminal.
pub struct Session {
    vm: Vm,
    port: Option<Box<dyn SerialPort>>,
    tx: Sender<Ev>,
    rx: Receiver<Ev>,
    out: Stdout,
    keys: Keys,
    mode: Mode,
    esc: bool,
    kind: LineKind,
    ed: Editor,
    hist: History,
    watch: Vec<u8>,
    later: VecDeque<Ev>,
    next_try: Instant,
    last_open_err: Option<String>,
}

/// Runs the interactive session until `ctrl-t q` or `bye`.
pub fn run(vm: Vm, sock: &Path) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let _srv = ctl::serve(sock, tx.clone())?;
    let keys = Keys::spawn(tx.clone());
    let (hist, hist_err) = match History::load(vm.settings.history.as_deref()) {
        Ok(h) => (h, None),
        Err(e) => (History::default(), Some(format!("history: {e}"))),
    };
    let raw = RawMode::on()?;
    let mut s = Session {
        vm,
        port: None,
        tx,
        rx,
        out: stdout(),
        keys,
        mode: Mode::Edit,
        esc: false,
        kind: LineKind::Target,
        ed: Editor::default(),
        hist,
        watch: Vec::new(),
        later: VecDeque::new(),
        next_try: Instant::now(),
        last_open_err: None,
    };
    let st = &s.vm.settings;
    s.note(&format!(
        "{} @ {} baud, ctrl-t h for help",
        st.port, st.baud
    ));
    if let Some(e) = hist_err {
        s.note(&e);
    }
    let r = s.main_loop();
    drop(raw);
    r
}

impl Session {
    fn main_loop(&mut self) -> Result<()> {
        while !self.vm.quit {
            self.reconnect();
            if let Some(ev) = self.later.pop_front() {
                self.handle(ev);
                continue;
            }
            match self.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(Ev::Fatal(e)) => bail!(e),
                Ok(ev) => self.handle(ev),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    }

    fn handle(&mut self, ev: Ev) {
        match ev {
            Ev::Serial(b) => self.serial(&b),
            Ev::Lost(e) => {
                self.port = None;
                if self.mode == Mode::Busy {
                    self.mode = Mode::Edit;
                }
                self.note(&format!("disconnected: {e}"));
            }
            Ev::Key(k) => self.key(k),
            Ev::Paste(s) => self.paste(&s),
            Ev::Ctl(r) => self.ctl(r),
            Ev::Note(n) => self.note(&n),
            Ev::Fatal(_) => {}
        }
    }

    fn line_visible(&self) -> bool {
        self.mode == Mode::Edit && (self.kind != LineKind::Target || !self.ed.is_empty())
    }

    fn hide_line(&mut self) {
        if self.line_visible() {
            self.echo("\r\x1b[K");
        }
    }

    fn show_line(&mut self) {
        if self.line_visible() {
            self.draw_line();
        }
    }

    fn draw_line(&mut self) {
        let label = match self.kind {
            LineKind::Target => "",
            LineKind::Forth => "forth> ",
            LineKind::Upload => "upload> ",
        };
        let s = self.ed.render(label);
        self.echo(&s);
    }

    fn serial(&mut self, b: &[u8]) {
        let visible = self.line_visible();
        self.hide_line();
        show(&mut self.out, b);
        if self.mode == Mode::Busy {
            self.watch_prompt(b);
        }
        if visible && !b.ends_with(b"\n") {
            self.echo("\n");
        }
        self.show_line();
    }

    fn watch_prompt(&mut self, b: &[u8]) {
        self.watch.extend_from_slice(b);
        while let Some(pos) = self.watch.iter().position(|&c| c == b'\n') {
            let line: Vec<u8> = self.watch.drain(..=pos).collect();
            if self
                .vm
                .settings
                .proto
                .classify(&String::from_utf8_lossy(&line))
                .is_some()
            {
                self.mode = Mode::Edit;
                self.watch.clear();
                return;
            }
        }
        let excess = self.watch.len().saturating_sub(4096);
        self.watch.drain(..excess);
    }

    fn note(&mut self, msg: &str) {
        self.hide_line();
        show(
            &mut self.out,
            format!("\n\x1b[33m[fterm] {msg}\x1b[0m\n").as_bytes(),
        );
        self.show_line();
    }

    fn reconnect(&mut self) {
        if self.port.is_some() || Instant::now() < self.next_try {
            return;
        }
        self.next_try = Instant::now() + Duration::from_secs(1);
        match self.open() {
            Ok(()) => {
                self.last_open_err = None;
                self.note("connected");
                if let Some(xt) = self.vm.find("on-connect") {
                    let _ = self.script(|vm, t| vm.call(xt, t).map(|_| String::new()));
                }
            }
            Err(e) => {
                let msg = format!("waiting for {}: {e:#}", self.vm.settings.port);
                if self.last_open_err.as_deref() != Some(&msg) {
                    self.note(&msg);
                    self.last_open_err = Some(msg);
                }
            }
        }
    }

    fn open(&mut self) -> Result<()> {
        let st = &self.vm.settings;
        let port = serialport::new(&st.port, st.baud)
            .timeout(Duration::from_millis(100))
            .open()?;
        let reader = port.try_clone()?;
        spawn_reader(reader, self.tx.clone());
        self.port = Some(port);
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> bool {
        let r = match self.port.as_mut() {
            Some(p) => p.write_all(bytes).and_then(|_| p.flush()),
            None => {
                self.note("not connected");
                return false;
            }
        };
        if let Err(e) = r {
            self.note(&format!("write: {e}"));
            return false;
        }
        true
    }

    fn echo(&mut self, s: &str) {
        show(&mut self.out, s.as_bytes());
    }

    fn script(
        &mut self,
        f: impl FnOnce(&mut Vm, &mut dyn Target) -> Result<String>,
    ) -> Result<String, String> {
        self.hide_line();
        let mut t = Live {
            port: &mut self.port,
            rx: &self.rx,
            out: &mut self.out,
            later: &mut self.later,
        };
        let r = f(&mut self.vm, &mut t).map_err(|e| format!("{e:#}"));
        match &r {
            Ok(m) if !m.is_empty() => self.note(m),
            Ok(_) => self.show_line(),
            Err(e) => self.note(e),
        }
        r
    }

    fn key(&mut self, k: KeyEvent) {
        if std::mem::take(&mut self.esc) {
            return self.escape_key(k);
        }
        if keys::is_ctrl(&k, 't') {
            self.esc = true;
            return;
        }
        if let Some(xt) = keys::fkey_name(&k).and_then(|n| self.vm.binds.get(&n).copied()) {
            return self.run_xt(xt);
        }
        match self.mode {
            Mode::Edit => self.edit_key(k),
            Mode::Busy | Mode::Raw => {
                if let Some(b) = keys::key_bytes(&k) {
                    self.write(&b);
                }
            }
        }
    }

    fn edit_key(&mut self, k: KeyEvent) {
        match self.ed.key(&k, &self.hist) {
            Action::Redraw => self.draw_line(),
            Action::Submit(line) => self.submit(line),
            Action::Cancel => {
                self.echo("\r\x1b[K");
                self.kind = LineKind::Target;
            }
            Action::Ignored => {}
        }
    }

    fn escape_key(&mut self, k: KeyEvent) {
        if keys::is_ctrl(&k, 't') {
            self.write(&[0x14]);
            return;
        }
        let KeyCode::Char(c) = k.code else { return };
        match c {
            'q' => self.vm.quit = true,
            'h' => self.note(HELP),
            ':' => self.prompt(LineKind::Forth),
            'u' => self.prompt(LineKind::Upload),
            'l' => {
                self.hide_line();
                self.mode = if self.mode == Mode::Raw {
                    Mode::Edit
                } else {
                    Mode::Raw
                };
                let on = if self.mode == Mode::Raw { "on" } else { "off" };
                self.note(&format!("raw passthrough {on}"));
            }
            'r' => match self.vm.last_upload.clone() {
                Some(p) => self.upload(&p),
                None => self.note("nothing uploaded yet"),
            },
            'e' => self.edit(None),
            'c' => {
                self.vm.required.clear();
                self.note("#require list cleared");
            }
            _ => match self.vm.binds.get(&c.to_string()).copied() {
                Some(xt) => self.run_xt(xt),
                None => self.note(&format!("ctrl-t {c}: unbound, ctrl-t h for help")),
            },
        }
    }

    fn prompt(&mut self, kind: LineKind) {
        self.hide_line();
        self.mode = Mode::Edit;
        self.kind = kind;
        self.ed.clear();
        self.draw_line();
    }

    fn submit(&mut self, line: String) {
        let kind = std::mem::replace(&mut self.kind, LineKind::Target);
        if let Err(e) = self.hist.push(&line) {
            self.note(&format!("history: {e}"));
        }
        let hash = match kind {
            LineKind::Target if line.starts_with('#') => parse_hash(&line),
            _ => Hash::Pass,
        };
        if kind == LineKind::Target && hash == Hash::Pass {
            self.echo("\r\x1b[K");
            return self.send_line(&line);
        }
        self.echo("\n");
        match (kind, hash) {
            (LineKind::Forth, _) => self.eval(&line),
            (LineKind::Upload, _) if line.trim().is_empty() => {}
            (LineKind::Upload, _) => self.upload(Path::new(line.trim())),
            (_, Hash::Include(p)) => self.upload(Path::new(&p)),
            (_, Hash::Require(p)) => self.require(Path::new(&p)),
            (_, Hash::Edit(p)) => self.edit(p.as_deref().map(Path::new)),
            (_, Hash::Forth(s)) => self.eval(&s),
            (_, Hash::Pass) => unreachable!("handled above"),
        }
    }

    fn send_line(&mut self, line: &str) {
        let mut b = line.as_bytes().to_vec();
        b.extend_from_slice(&self.vm.settings.proto.eol.clone());
        if self.write(&b) {
            self.mode = Mode::Busy;
            self.watch.clear();
        }
    }

    fn eval(&mut self, src: &str) {
        let _ = self.script(|vm, t| {
            vm.eval(src, t)?;
            Ok(format!("host stack: {:?}", vm.stack()))
        });
    }

    fn upload(&mut self, p: &Path) {
        let _ = self.script(|vm, t| {
            vm.upload_file(p, t)
                .map(|n| format!("{}: {n} lines ok", p.display()))
        });
    }

    fn require(&mut self, p: &Path) {
        let _ = self.script(|vm, t| {
            Ok(match vm.require_file(p, t)? {
                0 => format!("{}: already loaded", p.display()),
                n => format!("{}: {n} lines ok", p.display()),
            })
        });
    }

    fn run_xt(&mut self, xt: usize) {
        let _ = self.script(|vm, t| vm.call(xt, t).map(|_| String::new()));
    }

    fn paste(&mut self, text: &str) {
        let one_line = text.trim_end_matches(['\r', '\n']);
        match self.mode {
            Mode::Edit if !one_line.contains(['\r', '\n']) => {
                self.ed.insert(one_line);
                self.draw_line();
            }
            Mode::Edit => {
                let _ = self.script(|vm, t| {
                    vm.upload_text("<paste>", text, t)
                        .map(|n| format!("paste: {n} lines ok"))
                });
            }
            Mode::Busy | Mode::Raw => {
                self.write(text.as_bytes());
            }
        }
    }

    fn ctl(&mut self, r: Req) {
        let res = match &r.cmd {
            Cmd::Upload(p) => self.script(|vm, t| {
                vm.upload_file(p, t)
                    .map(|n| format!("{}: {n} lines ok", p.display()))
            }),
            Cmd::Eval(s) => self.script(|vm, t| vm.eval(s, t).map(|_| String::new())),
        };
        if r.reply.send(res).is_err() {
            self.note("control client went away");
        }
    }

    fn edit(&mut self, file: Option<&Path>) {
        let (file, line) = match (file, &self.vm.last_error, &self.vm.last_upload) {
            (Some(f), _, _) => (self.vm.resolve(f), 1),
            (None, Some(e), _) => (e.file.clone(), e.no),
            (None, None, Some(f)) => (f.clone(), 1),
            (None, None, None) => return self.note("nothing to edit"),
        };
        if let Err(e) = self.spawn_editor(&file, line) {
            self.note(&format!("editor: {e:#}"));
        }
        self.show_line();
    }

    fn spawn_editor(&mut self, file: &PathBuf, line: usize) -> Result<()> {
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| "nvim".into());
        let mut parts = editor.split_whitespace();
        let prog = parts.next().context("empty $EDITOR")?;
        self.keys.pause();
        terminal::disable_raw_mode()?;
        execute!(self.out, DisableBracketedPaste)?;
        let status = Command::new(prog)
            .args(parts)
            .arg(format!("+{line}"))
            .arg(file)
            .status();
        terminal::enable_raw_mode()?;
        execute!(self.out, EnableBracketedPaste)?;
        self.keys.resume();
        let status = status.with_context(|| prog.to_string())?;
        if !status.success() {
            bail!("{prog} exited with {status}");
        }
        Ok(())
    }
}

fn spawn_reader(mut port: Box<dyn SerialPort>, tx: Sender<Ev>) {
    thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            let ev = match port.read(&mut buf) {
                Ok(0) => Ev::Lost("end of stream".into()),
                Ok(n) => Ev::Serial(buf[..n].to_vec()),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(e) => Ev::Lost(e.to_string()),
            };
            let stop = matches!(ev, Ev::Lost(_));
            if tx.send(ev).is_err() || stop {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hash_cases() {
        let cases = [
            ("#r dcf77.fs", Hash::Require("dcf77.fs".into())),
            ("#require  a b.fs ", Hash::Require("a b.fs".into())),
            ("#i x.fs", Hash::Include("x.fs".into())),
            ("#include x.fs", Hash::Include("x.fs".into())),
            ("#e", Hash::Edit(None)),
            ("#e pins.fs", Hash::Edit(Some("pins.fs".into()))),
            ("#f 1 2 + .", Hash::Forth("1 2 + .".into())),
            ("#r", Hash::Pass),
            ("#s", Hash::Pass),
            ("# 42", Hash::Pass),
        ];
        for (line, want) in cases {
            assert_eq!(parse_hash(line), want, "{line:?}");
        }
    }
}
