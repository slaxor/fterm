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

/// User-wide config: `$XDG_CONFIG_HOME/fterm/init.fs` or `~/.config/fterm/init.fs`.
pub fn global_config() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(dir.join("fterm/init.fs")).filter(|p| p.is_file())
}

/// Creates a machine and runs `explicit` or the nearest `.fterm.fs`; only without one the
/// global config. A project config pulls the global one in with `include-global`.
pub fn load(explicit: Option<&Path>) -> Result<Vm> {
    load_from(
        global_config().as_deref(),
        explicit,
        &std::env::current_dir()?,
    )
}

fn load_from(global: Option<&Path>, explicit: Option<&Path>, cwd: &Path) -> Result<Vm> {
    let mut vm = Vm::new();
    vm.settings.base = cwd.to_path_buf();
    vm.settings.global = global.map(Path::to_path_buf);
    let project = explicit.map(Path::to_path_buf).or_else(|| find_config(cwd));
    match (project, global) {
        (Some(p), _) => vm.eval_file(&p, &mut Offline)?,
        (None, Some(g)) => {
            vm.eval_file(g, &mut Offline)?;
            vm.settings.base = cwd.to_path_buf();
        }
        (None, None) => {}
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
    fn project_replaces_global() {
        let d = tempfile::tempdir().unwrap();
        let g = d.path().join("init.fs");
        std::fs::write(&g, "9600 baud : hi ; s\" /dev/a\" port").unwrap();
        let proj = d.path().join("proj");
        std::fs::create_dir(&proj).unwrap();
        let port_baud = |vm: &Vm| (vm.settings.port.clone(), vm.settings.baud);

        let vm = load_from(Some(&g), None, &proj).unwrap();
        assert_eq!(port_baud(&vm), ("/dev/a".into(), 9600));
        assert!(vm.find("hi").is_some());
        assert_eq!(
            vm.settings.base, proj,
            "base must not stay in the global config dir"
        );

        std::fs::write(proj.join(CONFIG), "s\" /dev/b\" port").unwrap();
        let vm = load_from(Some(&g), None, &proj).unwrap();
        assert_eq!(port_baud(&vm), ("/dev/b".into(), 115200));
        assert!(
            vm.find("hi").is_none(),
            "global must not load next to a project config"
        );

        std::fs::write(proj.join(CONFIG), "include-global s\" /dev/b\" port").unwrap();
        let vm = load_from(Some(&g), None, &proj).unwrap();
        assert_eq!(port_baud(&vm), ("/dev/b".into(), 9600));
        assert!(vm.find("hi").is_some());

        let vm = load_from(None, None, &proj).unwrap();
        assert_eq!(
            vm.settings.port, "/dev/b",
            "include-global without init.fs is a no-op"
        );

        std::fs::write(&g, "include-global 1").unwrap();
        let vm = load_from(Some(&g), None, d.path()).unwrap();
        assert_eq!(
            vm.stack(),
            [1],
            "include-global inside init.fs must not recurse"
        );
    }

    #[test]
    fn offline_refuses_io() {
        let mut vm = Vm::new();
        assert!(vm.eval("s\" x\" send", &mut Offline).is_err());
    }
}
