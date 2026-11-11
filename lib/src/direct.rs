//! One-shot commands: through a running session's socket, or straight to the port.

use crate::ctl::{self, Cmd};
use crate::link::Target;
use crate::vm::Vm;
use anyhow::{bail, Context, Result};
use serialport::SerialPort;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::{Duration, Instant};

/// Runs `cmd` in the session listening on `sock`, or opens the port itself if none does.
pub fn run(mut vm: Vm, sock: &Path, cmd: Cmd) -> Result<()> {
    if let Some(r) = ctl::request(sock, &cmd)? {
        return match r {
            Ok(m) if m.is_empty() => Ok(()),
            Ok(m) => {
                eprintln!("{m}");
                Ok(())
            }
            Err(m) => bail!(m),
        };
    }
    let st = &vm.settings;
    let port = serialport::new(&st.port, st.baud)
        .timeout(Duration::from_millis(50))
        .open()
        .with_context(|| st.port.clone())?;
    let mut t = Direct { port };
    match cmd {
        Cmd::Upload(p) => {
            let n = vm.upload_file(&p, &mut t)?;
            eprintln!("{}: {n} lines ok", p.display());
        }
        Cmd::Eval(s) => vm.eval(&s, &mut t)?,
    }
    Ok(())
}

/// Target on a directly opened port; output goes to stdout.
pub struct Direct {
    pub port: Box<dyn SerialPort>,
}

impl Target for Direct {
    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.port.write_all(bytes)?;
        Ok(self.port.flush()?)
    }

    fn read(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        let mut buf = [0u8; 1024];
        while Instant::now() < deadline {
            match self.port.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let mut out = std::io::stdout();
                    out.write_all(&buf[..n])?;
                    out.flush()?;
                    return Ok(buf[..n].to_vec());
                }
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::TimedOut => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Vec::new())
    }

    fn emit(&mut self, text: &str) {
        print!("{text}");
    }
}
