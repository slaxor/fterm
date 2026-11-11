//! Locating and loading the host Forth config.

use crate::link::Target;
use crate::vm::Vm;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// File name searched upwards from the working directory.
pub const CONFIG: &str = ".fterm.fs";

/// Nearest `.fterm.fs` in `start` or one of its ancestors.
pub fn find_config(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|d| d.join(CONFIG))
        .find(|p| p.is_file())
}

/// Creates a machine and runs `explicit`, or the nearest `.fterm.fs`, in it.
pub fn load(explicit: Option<&Path>) -> Result<Vm> {
    let mut vm = Vm::new();
    let cwd = std::env::current_dir()?;
    vm.settings.base = cwd.clone();
    let path = explicit
        .map(Path::to_path_buf)
        .or_else(|| find_config(&cwd));
    if let Some(p) = path {
        vm.eval_file(&p, &mut Offline)?;
    }
    Ok(vm)
}

/// Target while loading the config: nothing to talk to yet.
pub struct Offline;

impl Target for Offline {
    fn write(&mut self, _: &[u8]) -> Result<()> {
        anyhow::bail!("not connected while loading the config; use on-connect")
    }

    fn read(&mut self, _: Instant) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn emit(&mut self, text: &str) {
        print!("{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_search_walks_up() {
        let d = tempfile::tempdir().unwrap();
        let sub = d.path().join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_config(&sub), None);
        let cfg = d.path().join(CONFIG);
        std::fs::write(&cfg, "s\" /dev/null\" port").unwrap();
        assert_eq!(find_config(&sub), Some(cfg.clone()));
        let vm = load(Some(&cfg)).unwrap();
        assert_eq!(vm.settings.port, "/dev/null");
        assert_eq!(vm.settings.base, d.path());
    }

    #[test]
    fn offline_refuses_io() {
        let mut vm = Vm::new();
        assert!(vm.eval("s\" x\" send", &mut Offline).is_err());
    }
}
