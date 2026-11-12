//! Talking to the target: flow-controlled line sending, uploads, waiting for output.

use crate::source::Line;
use anyhow::{bail, Result};
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Byte channel to the target plus a local output for notes.
pub trait Target {
    /// Writes raw bytes to the target.
    fn write(&mut self, bytes: &[u8]) -> Result<()>;
    /// Returns the next chunk of target output (already shown to the user),
    /// or an empty vector once `deadline` passed.
    fn read(&mut self, deadline: Instant) -> Result<Vec<u8>>;
    /// Prints text locally, never sent to the target.
    fn emit(&mut self, text: &str);
    /// Sets a modem control line (DTR or RTS), e.g. to reset the target.
    fn control(&mut self, line: Control, on: bool) -> Result<()> {
        let _ = (line, on);
        bail!("no serial port to set modem lines on")
    }
}

/// Modem control lines settable with [`Target::control`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Dtr,
    Rts,
}

/// Sets `line` on a serial port.
pub fn set_control(port: &mut dyn serialport::SerialPort, line: Control, on: bool) -> Result<()> {
    match line {
        Control::Dtr => port.write_data_terminal_ready(on)?,
        Control::Rts => port.write_request_to_send(on)?,
    }
    Ok(())
}

/// Prompt and error patterns plus timing for line-by-line sending.
#[derive(Debug, Clone)]
pub struct Protocol {
    pub eol: Vec<u8>,
    pub ok: String,
    pub errors: Vec<String>,
    pub timeout: Duration,
}

impl Default for Protocol {
    fn default() -> Self {
        Self {
            eol: b"\r".to_vec(),
            ok: "ok.".into(),
            errors: [
                "not found.",
                "Stack underflow",
                "Stack not balanced",
                "Structures don't match",
                "Jump too far",
            ]
            .map(String::from)
            .to_vec(),
            timeout: Duration::from_secs(3),
        }
    }
}

/// How the target answered one line.
#[derive(Debug, PartialEq)]
pub enum Reply {
    Ok,
    Error(String),
}

/// Collects target output after one sent line and decides when it answered.
pub struct Matcher<'a> {
    proto: &'a Protocol,
    echo: &'a str,
    buf: Vec<u8>,
}

impl<'a> Matcher<'a> {
    /// Creates a matcher for the reply to `echo`.
    pub fn new(proto: &'a Protocol, echo: &'a str) -> Self {
        Self {
            proto,
            echo,
            buf: Vec::new(),
        }
    }

    /// Feeds output; returns the reply once a complete line decides it.
    pub fn feed(&mut self, bytes: &[u8]) -> Option<Reply> {
        self.buf.extend_from_slice(bytes);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&raw);
            if let Some(r) = self.classify(line.trim_end()) {
                return Some(r);
            }
        }
        None
    }

    fn classify(&self, line: &str) -> Option<Reply> {
        self.proto
            .classify(line.strip_prefix(self.echo).unwrap_or(line))
    }
}

impl Protocol {
    /// Decides whether one complete output line is a prompt, an error, or neither.
    pub fn classify(&self, line: &str) -> Option<Reply> {
        let line = line.trim_end();
        if is_ok(line, &self.ok) {
            return Some(Reply::Ok);
        }
        self.errors
            .iter()
            .any(|e| line.contains(e.as_str()))
            .then(|| Reply::Error(line.trim().to_string()))
    }
}

fn is_ok(line: &str, ok: &str) -> bool {
    line.strip_suffix(ok)
        .is_some_and(|head| head.is_empty() || head.ends_with(char::is_whitespace))
}

/// An upload that stopped at a line the target rejected.
#[derive(Debug, Clone, PartialEq)]
pub struct UploadError {
    pub file: PathBuf,
    pub no: usize,
    pub text: String,
    pub msg: String,
}

impl fmt::Display for UploadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: {}\n    {}",
            self.file.display(),
            self.no,
            self.msg,
            self.text
        )
    }
}

impl std::error::Error for UploadError {}

/// Sends one line and waits until the target prompts or reports an error.
/// Also returns everything the target printed in reply.
pub fn send_line(t: &mut dyn Target, p: &Protocol, text: &str) -> Result<(Reply, String)> {
    let mut out = text.as_bytes().to_vec();
    out.extend_from_slice(&p.eol);
    t.write(&out)?;
    let mut m = Matcher::new(p, text);
    let mut seen = Vec::new();
    let deadline = Instant::now() + p.timeout;
    loop {
        let chunk = t.read(deadline)?;
        let out = || String::from_utf8_lossy(&seen).into_owned();
        if chunk.is_empty() {
            let msg = format!("no prompt within {} ms", p.timeout.as_millis());
            return Ok((Reply::Error(msg), out()));
        }
        seen.extend_from_slice(&chunk);
        if let Some(r) = m.feed(&chunk) {
            return Ok((r, String::from_utf8_lossy(&seen).into_owned()));
        }
    }
}

/// Sends `lines` one by one, stopping at the first error.
pub fn upload(t: &mut dyn Target, p: &Protocol, lines: &[Line]) -> Result<()> {
    for l in lines {
        if let (Reply::Error(msg), _) = send_line(t, p, &l.text)? {
            let e = UploadError {
                file: l.file.clone(),
                no: l.no,
                text: l.text.clone(),
                msg,
            };
            return Err(e.into());
        }
    }
    Ok(())
}

/// Waits up to `timeout` for `pat` in the target output.
pub fn expect(t: &mut dyn Target, pat: &str, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    let mut seen = Vec::new();
    loop {
        let chunk = t.read(deadline)?;
        if chunk.is_empty() {
            return Ok(false);
        }
        seen.extend_from_slice(&chunk);
        if String::from_utf8_lossy(&seen).contains(pat) {
            return Ok(true);
        }
        let keep = seen.len().saturating_sub(pat.len().max(1) * 4);
        seen.drain(..keep);
    }
}

/// Passes target output through for `d` without sending anything.
pub fn delay(t: &mut dyn Target, d: Duration) -> Result<()> {
    let deadline = Instant::now() + d;
    while Instant::now() < deadline {
        t.read(deadline)?;
    }
    Ok(())
}

/// Sends `text` as one line, failing on an error reply; returns the reply output.
pub fn send_checked(t: &mut dyn Target, p: &Protocol, text: &str) -> Result<String> {
    match send_line(t, p, text)? {
        (Reply::Ok, out) => Ok(out),
        (Reply::Error(msg), _) => bail!("{text}: {msg}"),
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::collections::VecDeque;

    /// Pretends to be Mecrisp: echoes, answers ` ok.` or `not found.` for lines containing `bad`.
    #[derive(Default)]
    pub struct Mecrisp {
        pub sent: Vec<String>,
        pub notes: String,
        pub pending: VecDeque<Vec<u8>>,
        pub silent: bool,
    }

    impl Target for Mecrisp {
        fn write(&mut self, bytes: &[u8]) -> Result<()> {
            let s = String::from_utf8_lossy(bytes);
            for line in s.split_terminator('\r') {
                self.sent.push(line.to_string());
                if self.silent {
                    continue;
                }
                let reply = if line.contains("bad") {
                    format!("{line} bad not found.\n")
                } else {
                    format!("{line}  ok.\n")
                };
                let (a, b) = reply.as_bytes().split_at(reply.len() / 2);
                self.pending.push_back(a.to_vec());
                self.pending.push_back(b.to_vec());
            }
            Ok(())
        }

        fn read(&mut self, _deadline: Instant) -> Result<Vec<u8>> {
            Ok(self.pending.pop_front().unwrap_or_default())
        }

        fn emit(&mut self, text: &str) {
            self.notes.push_str(text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Mecrisp;
    use super::*;

    #[test]
    fn protocol_classify() {
        let p = Protocol::default();
        assert_eq!(p.classify("log  ok.\r"), Some(Reply::Ok));
        assert_eq!(
            p.classify("x not found."),
            Some(Reply::Error("x not found.".into()))
        );
        assert_eq!(p.classify("gap 1000 pulse 100"), None);
    }

    #[test]
    fn matcher_cases() {
        let p = Protocol::default();
        let cases: &[(&str, &str, Option<Reply>)] = &[
            ("1 2 +", "1 2 +  ok.\n", Some(Reply::Ok)),
            ("1 2 +", "1 2 +  ok.\r\n", Some(Reply::Ok)),
            ("words", "words\nfoo bar\nbaz  ok.\n", Some(Reply::Ok)),
            (
                "foo",
                "foo foo not found.\n",
                Some(Reply::Error("foo not found.".into())),
            ),
            (
                ".\" not found.\"",
                ".\" not found.\" not found. ok.\n",
                Some(Reply::Ok),
            ),
            ("1 2 +", "1 2 +  ok.", None),
            ("x", "xbroke.\n", None),
            ("x", "x took.\n", None),
        ];
        for (echo, out, want) in cases {
            let mut m = Matcher::new(&p, echo);
            assert_eq!(&m.feed(out.as_bytes()), want, "echo {echo:?} out {out:?}");
        }
    }

    fn line(no: usize, text: &str) -> Line {
        Line {
            file: "t.fs".into(),
            no,
            text: text.into(),
        }
    }

    #[test]
    fn upload_stops_at_error() {
        let p = Protocol::default();
        let mut t = Mecrisp::default();
        let lines = [line(1, "a"), line(2, "bad"), line(3, "c")];
        let err = upload(&mut t, &p, &lines).unwrap_err();
        let e = err.downcast_ref::<UploadError>().unwrap();
        assert_eq!((e.no, e.msg.as_str()), (2, "bad not found."));
        assert_eq!(t.sent, ["a", "bad"]);
    }

    #[test]
    fn upload_times_out() {
        let p = Protocol::default();
        let mut t = Mecrisp {
            silent: true,
            ..Default::default()
        };
        let err = upload(&mut t, &p, &[line(1, "a")]).unwrap_err();
        assert!(err.to_string().contains("no prompt"), "{err}");
    }

    #[test]
    fn expect_finds_split_pattern() {
        let mut t = Mecrisp::default();
        t.pending.extend([b"xx C".to_vec(), b"ET yy".to_vec()]);
        assert!(expect(&mut t, "CET", Duration::from_millis(10)).unwrap());
        assert!(!expect(&mut t, "CET", Duration::from_millis(10)).unwrap());
    }
}
