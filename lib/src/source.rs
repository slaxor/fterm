//! Forth source preprocessing for uploads: comment stripping, `#include` and `#require`.

use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// One line ready to send, with its origin for error reports.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub file: PathBuf,
    pub no: usize,
    pub text: String,
}

const DELIMITED: &[(&str, char)] = &[
    (".\"", '"'),
    ("s\"", '"'),
    ("c\"", '"'),
    ("abort\"", '"'),
    (".(", ')'),
    ("(", ')'),
];

/// Removes a `\` comment and surrounding whitespace, leaving string literals and `( )` intact.
pub fn strip_comment(line: &str) -> &str {
    let b = line.as_bytes();
    let mut i = 0;
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        if start == i {
            return line.trim();
        }
        let tok = &line[start..i];
        if tok == "\\" {
            return line[..start].trim();
        }
        if let Some(&(_, close)) = DELIMITED.iter().find(|(w, _)| tok.eq_ignore_ascii_case(w)) {
            match line[i..].find(close) {
                Some(p) => i += p + 1,
                None => return line.trim(),
            }
        }
    }
}

enum Directive<'a> {
    Include(&'a str),
    Require(&'a str),
}

fn directive(line: &str) -> Option<Directive<'_>> {
    let t = line.trim();
    let (word, arg) = t.split_once(char::is_whitespace)?;
    match word {
        "#include" => Some(Directive::Include(arg.trim())),
        "#require" => Some(Directive::Require(arg.trim())),
        _ => None,
    }
}

/// Expands Forth sources into sendable lines.
///
/// `required` starts as the set of files already loaded via `#require` in this
/// session; the caller takes the updated set with [`Loader::into_required`] once
/// the upload succeeded.
pub struct Loader<'a> {
    search: &'a [PathBuf],
    required: HashSet<PathBuf>,
    stack: Vec<PathBuf>,
    out: Vec<Line>,
}

impl<'a> Loader<'a> {
    /// Creates a loader that resolves directives against the including file's
    /// directory first, then `search`.
    pub fn new(search: &'a [PathBuf], required: HashSet<PathBuf>) -> Self {
        Self {
            search,
            required,
            stack: Vec::new(),
            out: Vec::new(),
        }
    }

    /// Expands the file at `path`.
    pub fn file(mut self, path: &Path) -> Result<(Vec<Line>, HashSet<PathBuf>)> {
        self.expand_file(path)?;
        Ok((self.out, self.required))
    }

    /// Expands in-memory text (a paste or a macro); directives resolve against `base`.
    pub fn text(
        mut self,
        name: &str,
        base: &Path,
        src: &str,
    ) -> Result<(Vec<Line>, HashSet<PathBuf>)> {
        self.expand(&base.join(name), base, src)?;
        Ok((self.out, self.required))
    }

    fn expand_file(&mut self, path: &Path) -> Result<()> {
        let canon = path
            .canonicalize()
            .with_context(|| path.display().to_string())?;
        if self.stack.contains(&canon) {
            bail!("include cycle at {}", canon.display());
        }
        let src = fs::read_to_string(&canon).with_context(|| canon.display().to_string())?;
        let dir = canon.parent().unwrap_or(Path::new("/")).to_path_buf();
        self.stack.push(canon.clone());
        self.expand(&canon, &dir, &src)?;
        self.stack.pop();
        Ok(())
    }

    fn expand(&mut self, name: &Path, dir: &Path, src: &str) -> Result<()> {
        for (i, raw) in src.lines().enumerate() {
            let at = || format!("{}:{}", name.display(), i + 1);
            match directive(raw) {
                Some(Directive::Include(arg)) => {
                    let f = self.resolve(dir, arg).with_context(at)?;
                    self.expand_file(&f).with_context(at)?;
                }
                Some(Directive::Require(arg)) => {
                    let f = self.resolve(dir, arg).with_context(at)?;
                    if self.required.insert(f.clone()) {
                        self.expand_file(&f).with_context(at)?;
                    }
                }
                None => {
                    let text = strip_comment(raw);
                    if !text.is_empty() {
                        self.out.push(Line {
                            file: name.to_path_buf(),
                            no: i + 1,
                            text: text.into(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn resolve(&self, dir: &Path, arg: &str) -> Result<PathBuf> {
        std::iter::once(dir)
            .chain(self.search.iter().map(PathBuf::as_path))
            .map(|d| d.join(arg))
            .find(|p| p.is_file())
            .map(|p| p.canonicalize())
            .transpose()?
            .with_context(|| format!("{arg}: not found in {} or search path", dir.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_comment_cases() {
        let cases = [
            ("", ""),
            ("   ", ""),
            ("\\ whole line", ""),
            ("  dup + \\ add", "dup +"),
            (": x .\" a \\ b\" ; \\ c", ": x .\" a \\ b\" ;"),
            ("( a \\ b ) drop", "( a \\ b ) drop"),
            ("s\" unterminated \\ x", "s\" unterminated \\ x"),
            ("foo\\bar", "foo\\bar"),
            ("  $D0000004 constant gpio-in", "$D0000004 constant gpio-in"),
        ];
        for (input, want) in cases {
            assert_eq!(strip_comment(input), want, "input {input:?}");
        }
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn include_and_require() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "lib.fs", "1 constant one\n");
        write(d.path(), "inc.fs", "\\ comment\n2 constant two\n");
        let main = write(
            d.path(),
            "main.fs",
            "#require lib.fs\n#include inc.fs\n#require lib.fs\none two +\n",
        );
        let (lines, req) = Loader::new(&[], HashSet::new()).file(&main).unwrap();
        let texts: Vec<_> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["1 constant one", "2 constant two", "one two +"]);
        assert_eq!(lines[1].no, 2);
        assert!(lines[1].file.ends_with("inc.fs"));
        assert_eq!(req.len(), 1);

        let (lines, _) = Loader::new(&[], req).file(&main).unwrap();
        assert_eq!(lines.len(), 2, "lib.fs must not be sent twice");
    }

    #[test]
    fn search_path_and_errors() {
        let d = tempfile::tempdir().unwrap();
        let lib = d.path().join("lib");
        fs::create_dir(&lib).unwrap();
        write(&lib, "x.fs", "x\n");
        let main = write(d.path(), "main.fs", "#include x.fs\n");
        let (lines, _) = Loader::new(&[lib], HashSet::new()).file(&main).unwrap();
        assert_eq!(lines[0].text, "x");

        let err = Loader::new(&[], HashSet::new()).file(&main).unwrap_err();
        assert!(format!("{err:#}").contains("main.fs:1"), "{err:#}");

        let a = write(d.path(), "a.fs", "#include b.fs\n");
        write(d.path(), "b.fs", "#include a.fs\n");
        let err = Loader::new(&[], HashSet::new()).file(&a).unwrap_err();
        assert!(format!("{err:#}").contains("include cycle"), "{err:#}");
    }

    #[test]
    fn text_source() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "lib.fs", "lib\n");
        let (lines, _) = Loader::new(&[], HashSet::new())
            .text("<paste>", d.path(), "a\n#include lib.fs\n\nb \\ c\n")
            .unwrap();
        let texts: Vec<_> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["a", "lib", "b"]);
    }
}
