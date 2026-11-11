//! Control socket: lets `fterm send`/`fterm eval` drive a running session.

use crate::session::Ev;
use anyhow::{bail, Context, Result};
use std::io::{ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread;

/// A request from a client.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Upload(PathBuf),
    Eval(String),
}

impl Cmd {
    /// Wire format: verb, newline, payload.
    pub fn encode(&self) -> String {
        match self {
            Cmd::Upload(p) => format!("upload\n{}", p.display()),
            Cmd::Eval(s) => format!("eval\n{s}"),
        }
    }

    /// Parses the wire format.
    pub fn decode(s: &str) -> Result<Self> {
        let (verb, arg) = s.split_once('\n').unwrap_or((s, ""));
        match verb {
            "upload" => Ok(Cmd::Upload(arg.into())),
            "eval" => Ok(Cmd::Eval(arg.into())),
            _ => bail!("unknown request {verb:?}"),
        }
    }
}

/// A request plus the channel its result goes back on.
pub struct Req {
    pub cmd: Cmd,
    pub reply: Sender<Result<String, String>>,
}

/// Listening socket; removed on drop.
pub struct Server {
    path: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.path) {
            eprintln!("fterm: removing {}: {e}", self.path.display());
        }
    }
}

/// Default socket path for a serial port.
pub fn socket_path(port: &str) -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let name: String = port
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    dir.join(format!("fterm{name}.sock"))
}

/// Listens on `path`, forwarding requests into the session as [`Ev::Ctl`].
pub fn serve(path: &Path, tx: Sender<Ev>) -> Result<Server> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            bail!("another fterm already serves {}", path.display());
        }
        std::fs::remove_file(path).with_context(|| format!("removing stale {}", path.display()))?;
    }
    let l = UnixListener::bind(path).with_context(|| path.display().to_string())?;
    thread::spawn(move || {
        for conn in l.incoming().flatten() {
            let Err(e) = handle(conn, &tx) else { continue };
            if tx.send(Ev::Note(format!("control socket: {e:#}"))).is_err() {
                return;
            }
        }
    });
    Ok(Server {
        path: path.to_path_buf(),
    })
}

fn handle(mut conn: UnixStream, tx: &Sender<Ev>) -> Result<()> {
    let mut s = String::new();
    conn.read_to_string(&mut s)?;
    if s.is_empty() {
        return Ok(());
    }
    let cmd = Cmd::decode(&s)?;
    let (rtx, rrx) = mpsc::channel();
    tx.send(Ev::Ctl(Req { cmd, reply: rtx }))?;
    let reply = match rrx.recv()? {
        Ok(m) => format!("ok\n{m}"),
        Err(m) => format!("error\n{m}"),
    };
    conn.write_all(reply.as_bytes())?;
    Ok(())
}

/// Sends `cmd` to a running session. `Ok(None)` means none is listening.
pub fn request(path: &Path, cmd: &Cmd) -> Result<Option<Result<String, String>>> {
    let mut c = match UnixStream::connect(path) {
        Ok(c) => c,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
            return Ok(None)
        }
        Err(e) => return Err(e).with_context(|| path.display().to_string()),
    };
    c.write_all(cmd.encode().as_bytes())?;
    c.shutdown(Shutdown::Write)?;
    let mut s = String::new();
    c.read_to_string(&mut s)?;
    let (status, msg) = s.split_once('\n').unwrap_or((&s, ""));
    Ok(Some(if status == "ok" {
        Ok(msg.into())
    } else {
        Err(msg.into())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let cases = [
            Cmd::Upload("/a/b.fs".into()),
            Cmd::Eval("1 2 +\n\\ c\n.".into()),
            Cmd::Eval(String::new()),
        ];
        for c in cases {
            assert_eq!(Cmd::decode(&c.encode()).unwrap(), c);
        }
        assert!(Cmd::decode("frob\nx").is_err());
    }

    #[test]
    fn socket_name() {
        let p = socket_path("/dev/ttyACM0");
        assert!(p.ends_with("fterm_dev_ttyACM0.sock"), "{}", p.display());
    }

    #[test]
    fn serve_and_request() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.sock");
        let (tx, rx) = mpsc::channel();
        let srv = serve(&path, tx).unwrap();
        assert!(
            serve(&path, mpsc::channel().0).is_err(),
            "second server must refuse"
        );
        let worker = thread::spawn(move || {
            if let Ok(Ev::Ctl(r)) = rx.recv() {
                r.reply.send(Err(format!("{:?}", r.cmd))).unwrap();
            }
        });
        let got = request(&path, &Cmd::Eval("x".into())).unwrap();
        assert_eq!(got, Some(Err("Eval(\"x\")".into())));
        worker.join().unwrap();
        drop(srv);
        assert_eq!(request(&path, &Cmd::Eval("x".into())).unwrap(), None);
    }
}
