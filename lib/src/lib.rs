//! fterm core library.

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
