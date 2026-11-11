//! fterm core: serial terminal for Forth targets with flow-controlled uploads and host Forth macros.

pub mod config;
pub mod ctl;
pub mod direct;
pub mod keys;
pub mod link;
pub mod session;
pub mod source;
pub mod vm;

/// Returns a greeting for `name`.
pub fn greet(name: &str) -> String {
    format!("hello, {name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greet_includes_name() {
        assert_eq!(greet("world"), "hello, world");
    }
}
