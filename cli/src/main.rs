//! fterm command line: argument parsing only, everything else lives in the `fterm` crate.

use anyhow::Result;
use clap::{Parser, Subcommand};
use fterm::ctl::{self, Cmd};
use fterm::{config, direct, session};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    version,
    about = "Serial terminal for Forth targets with flow-controlled uploads and Forth macros"
)]
struct Cli {
    /// Host Forth config; default: nearest .fterm.fs upwards from the working directory
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Serial device, overrides the config
    #[arg(short, long)]
    port: Option<String>,
    /// Baud rate, overrides the config
    #[arg(short, long)]
    baud: Option<u32>,
    #[command(subcommand)]
    cmd: Option<Sub>,
}

#[derive(Subcommand)]
enum Sub {
    /// Upload a file through the running session, or directly if none runs
    Send { file: PathBuf },
    /// Run host Forth in the running session, or directly if none runs
    Eval { forth: String },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let mut vm = config::load(cli.config.as_deref())?;
    if let Some(p) = cli.port {
        vm.settings.port = p;
    }
    if let Some(b) = cli.baud {
        vm.settings.baud = b;
    }
    let sock = ctl::socket_path(&vm.settings.port);
    match cli.cmd {
        None => session::run(vm, &sock),
        Some(Sub::Send { file }) => direct::run(vm, &sock, Cmd::Upload(std::path::absolute(file)?)),
        Some(Sub::Eval { forth }) => direct::run(vm, &sock, Cmd::Eval(forth)),
    }
}
