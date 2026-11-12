//! Host-side Forth: configuration language and macro engine.

use crate::link::{self, Protocol, Target, UploadError};
use crate::source::{Line, Loader};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

/// Connection and upload settings, set by the config script.
#[derive(Debug, Clone)]
pub struct Settings {
    pub port: String,
    pub baud: u32,
    pub proto: Protocol,
    pub path: Vec<PathBuf>,
    pub history: Option<PathBuf>,
    pub base: PathBuf,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            port: "/dev/ttyACM0".into(),
            baud: 115200,
            proto: Protocol::default(),
            path: Vec::new(),
            history: crate::edit::default_history_path(),
            base: PathBuf::from("."),
        }
    }
}

#[rustfmt::skip]
#[derive(Clone, Copy, Debug, PartialEq)]
enum Prim {
    Dup, Drop, Swap, Over, Rot, Nip, Tuck, TwoDup, TwoDrop, Depth,
    Add, Sub, Mul, Div, Mod, Negate, Abs, Min, Max, OnePlus, OneMinus,
    And, Or, Xor, Invert, Lshift, Rshift,
    Eq, Ne, Lt, Gt, ZeroEq, ZeroLt,
    Fetch, Store, CFetch, CStore, PlusStore, Here, Allot, Comma, Cells,
    ToR, RFrom, RFetch, I, J, Execute,
    Dot, Emit, Cr, Space, Type, Words,
    Send, Run, Raw, Upload, Expect, Delay, Included,
    Port, Baud, Eol, Prompt, Error, NoErrors, Timeout, AddPath, HistFile, Forget, Bye,
}

#[rustfmt::skip]
const PRIMS: &[(&str, Prim)] = {
    use Prim::*;
    &[
        ("dup", Dup), ("drop", Drop), ("swap", Swap), ("over", Over), ("rot", Rot),
        ("nip", Nip), ("tuck", Tuck), ("2dup", TwoDup), ("2drop", TwoDrop), ("depth", Depth),
        ("+", Add), ("-", Sub), ("*", Mul), ("/", Div), ("mod", Mod), ("negate", Negate),
        ("abs", Abs), ("min", Min), ("max", Max), ("1+", OnePlus), ("1-", OneMinus),
        ("and", And), ("or", Or), ("xor", Xor), ("invert", Invert),
        ("lshift", Lshift), ("rshift", Rshift),
        ("=", Eq), ("<>", Ne), ("<", Lt), (">", Gt), ("0=", ZeroEq), ("0<", ZeroLt),
        ("@", Fetch), ("!", Store), ("c@", CFetch), ("c!", CStore), ("+!", PlusStore),
        ("here", Here), ("allot", Allot), (",", Comma), ("cells", Cells),
        (">r", ToR), ("r>", RFrom), ("r@", RFetch), ("i", I), ("j", J), ("execute", Execute),
        (".", Dot), ("emit", Emit), ("cr", Cr), ("space", Space), ("type", Type), ("words", Words),
        ("send", Send), ("run", Run), ("raw", Raw), ("upload", Upload), ("expect", Expect), ("delay", Delay),
        ("included", Included),
        ("port", Port), ("baud", Baud), ("eol", Eol), ("prompt", Prompt), ("error", Error),
        ("no-errors", NoErrors), ("timeout", Timeout), ("path", AddPath), ("history", HistFile),
        ("forget-required", Forget), ("bye", Bye),
    ]
};

#[derive(Clone, Debug)]
enum Op {
    Call(usize),
    Lit(i64),
    Str(usize, usize),
    Print(usize, usize),
    Branch(usize),
    BranchZ(usize),
    Do,
    Loop(usize),
    Exit,
}

#[derive(Clone, Debug)]
enum Kind {
    Prim(Prim),
    Colon(Rc<Vec<Op>>),
    Const(i64),
    Var(usize),
}

#[derive(Clone, Debug)]
struct Word {
    name: String,
    kind: Kind,
}

#[derive(Debug, Clone, Copy)]
enum Mark {
    If(usize),
    Else(usize),
    Begin(usize),
    While(usize, usize),
    Do(usize),
}

#[derive(Clone, Debug)]
struct Source {
    origin: String,
    line: usize,
    text: String,
}

struct Compiling {
    name: String,
    ops: Vec<Op>,
    marks: Vec<Mark>,
    start: usize,
}

struct Input {
    src: Vec<char>,
    pos: usize,
}

impl Input {
    fn word(&mut self) -> Option<String> {
        while self.pos < self.src.len() && self.src[self.pos].is_whitespace() {
            self.pos += 1;
        }
        let start = self.pos;
        while self.pos < self.src.len() && !self.src[self.pos].is_whitespace() {
            self.pos += 1;
        }
        (start < self.pos).then(|| self.src[start..self.pos].iter().collect())
    }

    fn until(&mut self, close: char) -> Result<String> {
        if self.pos < self.src.len() {
            self.pos += 1;
        }
        let start = self.pos;
        while self.pos < self.src.len() && self.src[self.pos] != close {
            self.pos += 1;
        }
        if self.pos >= self.src.len() && close != '\n' {
            bail!("missing {close:?}");
        }
        let s = self.src[start..self.pos].iter().collect();
        self.pos += 1;
        Ok(s)
    }
}

/// The host Forth machine. Holds settings, key bindings and session state.
pub struct Vm {
    words: Vec<Word>,
    ds: Vec<i64>,
    rs: Vec<i64>,
    mem: Vec<u8>,
    comp: Option<Compiling>,
    depth: usize,
    pub settings: Settings,
    pub binds: BTreeMap<String, usize>,
    pub required: HashSet<PathBuf>,
    pub last_upload: Option<PathBuf>,
    pub last_error: Option<UploadError>,
    pub quit: bool,
    tail: String,
    sources: HashMap<usize, Source>,
    origin: String,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    /// Creates a machine with all primitives and default settings.
    pub fn new() -> Self {
        let words = PRIMS
            .iter()
            .map(|&(n, p)| Word {
                name: n.into(),
                kind: Kind::Prim(p),
            })
            .collect();
        Self {
            words,
            ds: Vec::new(),
            rs: Vec::new(),
            mem: Vec::new(),
            comp: None,
            depth: 0,
            settings: Settings::default(),
            binds: BTreeMap::new(),
            required: HashSet::new(),
            last_upload: None,
            last_error: None,
            quit: false,
            tail: String::new(),
            sources: HashMap::new(),
            origin: "<input>".into(),
        }
    }

    /// Interprets `src`. On error the stacks and any open definition are reset.
    pub fn eval(&mut self, src: &str, t: &mut dyn Target) -> Result<()> {
        let mut input = Input {
            src: src.chars().collect(),
            pos: 0,
        };
        let r = self.interpret(&mut input, t);
        if r.is_err() {
            self.ds.clear();
            self.rs.clear();
            self.comp = None;
            self.depth = 0;
        }
        r
    }

    /// Runs the host Forth file at `path`; it becomes the base for relative paths.
    pub fn eval_file(&mut self, path: &Path, t: &mut dyn Target) -> Result<()> {
        if let Some(dir) = path.parent() {
            self.settings.base = dir.to_path_buf();
        }
        self.eval_named(path, t)
    }

    fn eval_named(&mut self, path: &Path, t: &mut dyn Target) -> Result<()> {
        let src = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
        let outer = std::mem::replace(&mut self.origin, path.display().to_string());
        let r = self
            .eval(&src, t)
            .with_context(|| path.display().to_string());
        self.origin = outer;
        r
    }

    /// Decompiled form of a word, as printed by `see`.
    pub fn see(&self, xt: usize) -> Result<String> {
        let w = self.word(xt)?;
        Ok(match &w.kind {
            Kind::Prim(_) => format!("{} is a primitive\n", w.name),
            Kind::Const(v) => format!("{v} constant {}\n", w.name),
            Kind::Var(a) => format!("variable {} \\ at {a}\n", w.name),
            Kind::Colon(ops) => {
                let mut out = format!(": {}\n", w.name);
                for (i, op) in ops.iter().enumerate() {
                    out.push_str(&format!("  {i:3}  {}\n", self.op_text(op)));
                }
                out.push_str(";\n");
                out
            }
        })
    }

    /// Source text of a colon definition with its origin, as printed by `list`.
    pub fn list(&self, xt: usize) -> Result<String> {
        let w = self.word(xt)?;
        let s = self
            .sources
            .get(&xt)
            .ok_or_else(|| anyhow!("{}: no source (not a colon definition)", w.name))?;
        Ok(format!("\\ {}:{}\n{}\n", s.origin, s.line, s.text))
    }

    fn op_text(&self, op: &Op) -> String {
        let name = |xt: &usize| {
            self.words
                .get(*xt)
                .map_or("<recurse>", |w| w.name.as_str())
                .to_string()
        };
        let text = |a: &usize, u: &usize| self.string(*a as i64, *u as i64).unwrap_or_default();
        match op {
            Op::Call(xt) => name(xt),
            Op::Lit(n) => n.to_string(),
            Op::Str(a, u) => format!("s\" {}\"", text(a, u)),
            Op::Print(a, u) => format!(".\" {}\"", text(a, u)),
            Op::Branch(to) => format!("branch -> {to}"),
            Op::BranchZ(to) => format!("0branch -> {to}"),
            Op::Do => "do".into(),
            Op::Loop(to) => format!("loop -> {to}"),
            Op::Exit => "exit".into(),
        }
    }

    /// Executes the word with execution token `xt`.
    pub fn call(&mut self, xt: usize, t: &mut dyn Target) -> Result<()> {
        let r = self.exec(xt, t);
        if r.is_err() {
            self.ds.clear();
            self.rs.clear();
            self.depth = 0;
        }
        r
    }

    /// Finds the execution token of `name`.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.words
            .iter()
            .rposition(|w| w.name.eq_ignore_ascii_case(name))
    }

    /// The data stack, bottom first.
    pub fn stack(&self) -> &[i64] {
        &self.ds
    }

    /// Uploads a target Forth file, honouring `#include`/`#require`.
    pub fn upload_file(&mut self, path: &Path, t: &mut dyn Target) -> Result<usize> {
        let path = self.resolve(path);
        self.last_upload = Some(path.clone());
        let (lines, req) = Loader::new(&self.settings.path, self.required.clone()).file(&path)?;
        self.run_upload(&lines, req, t)
    }

    /// Uploads in-memory target Forth text, e.g. a paste.
    pub fn upload_text(&mut self, name: &str, text: &str, t: &mut dyn Target) -> Result<usize> {
        let cwd = std::env::current_dir()?;
        let (lines, req) =
            Loader::new(&self.settings.path, self.required.clone()).text(name, &cwd, text)?;
        self.run_upload(&lines, req, t)
    }

    /// Uploads `path` only if it was not yet required in this session.
    pub fn require_file(&mut self, path: &Path, t: &mut dyn Target) -> Result<usize> {
        let path = self.resolve(path);
        let canon = path
            .canonicalize()
            .with_context(|| path.display().to_string())?;
        if self.required.contains(&canon) {
            return Ok(0);
        }
        let n = self.upload_file(&canon, t)?;
        self.required.insert(canon);
        Ok(n)
    }

    fn run_upload(
        &mut self,
        lines: &[Line],
        req: HashSet<PathBuf>,
        t: &mut dyn Target,
    ) -> Result<usize> {
        match link::upload(t, &self.settings.proto, lines) {
            Ok(()) => {
                self.required = req;
                self.last_error = None;
                Ok(lines.len())
            }
            Err(e) => {
                self.last_error = e.downcast_ref::<UploadError>().cloned();
                Err(e)
            }
        }
    }

    /// Resolves a relative path against the working directory, then the config directory.
    pub fn resolve(&self, p: &Path) -> PathBuf {
        if p.is_absolute() || p.exists() {
            return p.to_path_buf();
        }
        let alt = self.settings.base.join(p);
        if alt.exists() {
            alt
        } else {
            p.to_path_buf()
        }
    }

    fn interpret(&mut self, inp: &mut Input, t: &mut dyn Target) -> Result<()> {
        while let Some(tok) = inp.word() {
            self.token(&tok, inp, t)
                .with_context(|| format!("at `{tok}`"))?;
            if self.quit {
                break;
            }
        }
        Ok(())
    }

    fn token(&mut self, tok: &str, inp: &mut Input, t: &mut dyn Target) -> Result<()> {
        let lower = tok.to_ascii_lowercase();
        match lower.as_str() {
            "\\" => return inp.until('\n').map(drop),
            "(" => return inp.until(')').map(drop),
            _ => {}
        }
        if self.comp.is_some() {
            return self.compile_token(tok, &lower, inp);
        }
        match lower.as_str() {
            ":" => {
                let start = inp.pos - 1;
                let name = inp.word().context("missing name")?;
                self.comp = Some(Compiling {
                    name,
                    ops: Vec::new(),
                    marks: Vec::new(),
                    start,
                });
            }
            "variable" => {
                let name = inp.word().context("missing name")?;
                let a = self.alloc(8);
                self.words.push(Word {
                    name,
                    kind: Kind::Var(a),
                });
            }
            "constant" => {
                let name = inp.word().context("missing name")?;
                let v = self.pop()?;
                self.words.push(Word {
                    name,
                    kind: Kind::Const(v),
                });
            }
            "create" => {
                let name = inp.word().context("missing name")?;
                let a = self.mem.len();
                self.words.push(Word {
                    name,
                    kind: Kind::Var(a),
                });
            }
            "s\"" => {
                let (a, u) = self.store_str(&inp.until('"')?);
                self.push(a as i64);
                self.push(u as i64);
            }
            ".\"" => t.emit(&inp.until('"')?),
            ".(" => t.emit(&inp.until(')')?),
            "'" => {
                let xt = self.tick(inp)?;
                self.push(xt as i64);
            }
            "char" | "[char]" => {
                let c = inp.word().context("missing char")?;
                self.push(c.chars().next().unwrap_or(' ') as i64);
            }
            "see" | "list" => {
                let xt = self.tick(inp)?;
                let text = if lower == "see" {
                    self.see(xt)?
                } else {
                    self.list(xt)?
                };
                t.emit(&text);
            }
            "bind" => {
                let key = normalize_key(&inp.word().context("missing key name")?);
                let xt = self.pop()? as usize;
                self.word(xt)?;
                self.binds.insert(key, xt);
            }
            _ => {
                if let Some(xt) = self.find(tok) {
                    self.exec(xt, t)?;
                } else {
                    let n = parse_number(tok).ok_or_else(|| anyhow!("{tok} not found"))?;
                    self.push(n);
                }
            }
        }
        Ok(())
    }

    fn compile_token(&mut self, tok: &str, lower: &str, inp: &mut Input) -> Result<()> {
        let op = match lower {
            ";" => return self.finish(inp),
            "if" => return self.mark(Mark::If, Op::BranchZ(0)),
            "else" => {
                let n = self.here_op();
                let m = self.pop_mark()?;
                let Mark::If(at) = m else {
                    bail!("else without if")
                };
                self.emit_op(Op::Branch(0));
                self.patch(at, n + 1);
                self.comp_mut().marks.push(Mark::Else(n));
                return Ok(());
            }
            "then" => {
                let n = self.here_op();
                match self.pop_mark()? {
                    Mark::If(at) | Mark::Else(at) => self.patch(at, n),
                    _ => bail!("then without if"),
                }
                return Ok(());
            }
            "begin" => {
                let n = self.here_op();
                self.comp_mut().marks.push(Mark::Begin(n));
                return Ok(());
            }
            "until" | "again" => {
                let Mark::Begin(b) = self.pop_mark()? else {
                    bail!("{lower} without begin")
                };
                if lower == "until" {
                    Op::BranchZ(b)
                } else {
                    Op::Branch(b)
                }
            }
            "while" => {
                let Mark::Begin(b) = self.pop_mark()? else {
                    bail!("while without begin")
                };
                let n = self.here_op();
                self.emit_op(Op::BranchZ(0));
                self.comp_mut().marks.push(Mark::While(b, n));
                return Ok(());
            }
            "repeat" => {
                let Mark::While(b, w) = self.pop_mark()? else {
                    bail!("repeat without while")
                };
                self.emit_op(Op::Branch(b));
                let n = self.here_op();
                self.patch(w, n);
                return Ok(());
            }
            "do" => {
                self.emit_op(Op::Do);
                let n = self.here_op();
                self.comp_mut().marks.push(Mark::Do(n));
                return Ok(());
            }
            "loop" => {
                let Mark::Do(b) = self.pop_mark()? else {
                    bail!("loop without do")
                };
                Op::Loop(b)
            }
            "exit" => Op::Exit,
            "recurse" => Op::Call(self.words.len()),
            "s\"" => {
                let (a, u) = self.store_str(&inp.until('"')?);
                Op::Str(a, u)
            }
            ".\"" => {
                let (a, u) = self.store_str(&inp.until('"')?);
                Op::Print(a, u)
            }
            "[']" => Op::Lit(self.tick(inp)? as i64),
            "[char]" => Op::Lit(
                inp.word()
                    .context("missing char")?
                    .chars()
                    .next()
                    .unwrap_or(' ') as i64,
            ),
            _ => match self.find(tok) {
                Some(xt) => Op::Call(xt),
                None => Op::Lit(parse_number(tok).ok_or_else(|| anyhow!("{tok} not found"))?),
            },
        };
        self.emit_op(op);
        Ok(())
    }

    fn finish(&mut self, inp: &Input) -> Result<()> {
        let c = self.comp.take().context("not compiling")?;
        if !c.marks.is_empty() {
            bail!("unbalanced control structure in {}", c.name);
        }
        let text: String = inp.src[c.start..inp.pos.min(inp.src.len())]
            .iter()
            .collect();
        let line = inp.src[..c.start].iter().filter(|&&ch| ch == '\n').count() + 1;
        let src = Source {
            origin: self.origin.clone(),
            line,
            text,
        };
        self.sources.insert(self.words.len(), src);
        self.words.push(Word {
            name: c.name,
            kind: Kind::Colon(Rc::new(c.ops)),
        });
        Ok(())
    }

    fn comp_mut(&mut self) -> &mut Compiling {
        self.comp.as_mut().expect("compiling")
    }

    fn here_op(&mut self) -> usize {
        self.comp_mut().ops.len()
    }

    fn emit_op(&mut self, op: Op) {
        self.comp_mut().ops.push(op);
    }

    fn mark(&mut self, m: fn(usize) -> Mark, op: Op) -> Result<()> {
        let n = self.here_op();
        self.emit_op(op);
        self.comp_mut().marks.push(m(n));
        Ok(())
    }

    fn pop_mark(&mut self) -> Result<Mark> {
        self.comp_mut()
            .marks
            .pop()
            .context("control structure mismatch")
    }

    fn patch(&mut self, at: usize, to: usize) {
        match &mut self.comp_mut().ops[at] {
            Op::Branch(x) | Op::BranchZ(x) => *x = to,
            other => unreachable!("patching {other:?}"),
        }
    }

    fn tick(&mut self, inp: &mut Input) -> Result<usize> {
        let name = inp.word().context("missing name")?;
        self.find(&name).ok_or_else(|| anyhow!("{name} not found"))
    }

    fn word(&self, xt: usize) -> Result<&Word> {
        self.words.get(xt).ok_or_else(|| anyhow!("invalid xt {xt}"))
    }

    fn exec(&mut self, xt: usize, t: &mut dyn Target) -> Result<()> {
        match self.word(xt)?.kind.clone() {
            Kind::Prim(p) => self.prim(p, t),
            Kind::Colon(body) => self.run(&body, t),
            Kind::Const(v) => {
                self.push(v);
                Ok(())
            }
            Kind::Var(a) => {
                self.push(a as i64);
                Ok(())
            }
        }
    }

    fn run(&mut self, body: &[Op], t: &mut dyn Target) -> Result<()> {
        self.depth += 1;
        if self.depth > 512 {
            bail!("return stack overflow");
        }
        let mut pc = 0;
        while pc < body.len() {
            pc += 1;
            match &body[pc - 1] {
                Op::Call(xt) => self.exec(*xt, t)?,
                Op::Lit(n) => self.push(*n),
                Op::Str(a, u) => {
                    self.push(*a as i64);
                    self.push(*u as i64);
                }
                Op::Print(a, u) => t.emit(&self.string(*a as i64, *u as i64)?),
                Op::Branch(to) => pc = *to,
                Op::BranchZ(to) => {
                    if self.pop()? == 0 {
                        pc = *to;
                    }
                }
                Op::Do => {
                    let (limit, start) = self.pop2()?;
                    self.rs.push(limit);
                    self.rs.push(start);
                }
                Op::Loop(to) => {
                    let n = self.rs.len();
                    if n < 2 {
                        bail!("loop without do");
                    }
                    self.rs[n - 1] += 1;
                    if self.rs[n - 1] < self.rs[n - 2] {
                        pc = *to;
                    } else {
                        self.rs.truncate(n - 2);
                    }
                }
                Op::Exit => break,
            }
        }
        self.depth -= 1;
        Ok(())
    }

    #[rustfmt::skip]
    fn prim(&mut self, p: Prim, t: &mut dyn Target) -> Result<()> {
        use Prim::*;
        match p {
            Dup => { let a = self.peek(0)?; self.push(a) }
            Drop => { self.pop()?; }
            Swap => { let (a, b) = self.pop2()?; self.push(b); self.push(a) }
            Over => { let a = self.peek(1)?; self.push(a) }
            Rot => { let c = self.pop()?; let (a, b) = self.pop2()?; self.push(b); self.push(c); self.push(a) }
            Nip => { let (_, b) = self.pop2()?; self.push(b) }
            Tuck => { let (a, b) = self.pop2()?; self.push(b); self.push(a); self.push(b) }
            TwoDup => { let (a, b) = (self.peek(1)?, self.peek(0)?); self.push(a); self.push(b) }
            TwoDrop => { self.pop2()?; }
            Depth => { let d = self.ds.len() as i64; self.push(d) }
            Add => self.bin(|a, b| Ok(a.wrapping_add(b)))?,
            Sub => self.bin(|a, b| Ok(a.wrapping_sub(b)))?,
            Mul => self.bin(|a, b| Ok(a.wrapping_mul(b)))?,
            Div => self.bin(|a, b| a.checked_div(b).context("division by zero"))?,
            Mod => self.bin(|a, b| a.checked_rem(b).context("division by zero"))?,
            Min => self.bin(|a, b| Ok(a.min(b)))?,
            Max => self.bin(|a, b| Ok(a.max(b)))?,
            And => self.bin(|a, b| Ok(a & b))?,
            Or => self.bin(|a, b| Ok(a | b))?,
            Xor => self.bin(|a, b| Ok(a ^ b))?,
            Lshift => self.bin(|a, b| Ok(a.wrapping_shl(b as u32)))?,
            Rshift => self.bin(|a, b| Ok(((a as u64).wrapping_shr(b as u32)) as i64))?,
            Eq => self.bin(|a, b| Ok(flag(a == b)))?,
            Ne => self.bin(|a, b| Ok(flag(a != b)))?,
            Lt => self.bin(|a, b| Ok(flag(a < b)))?,
            Gt => self.bin(|a, b| Ok(flag(a > b)))?,
            Negate => self.un(|a| a.wrapping_neg())?,
            Abs => self.un(|a| a.wrapping_abs())?,
            OnePlus => self.un(|a| a.wrapping_add(1))?,
            OneMinus => self.un(|a| a.wrapping_sub(1))?,
            Invert => self.un(|a| !a)?,
            ZeroEq => self.un(|a| flag(a == 0))?,
            ZeroLt => self.un(|a| flag(a < 0))?,
            Fetch => { let a = self.pop()?; let a = self.addr(a, 8)?; let v = i64::from_le_bytes(self.mem[a..a + 8].try_into()?); self.push(v) }
            Store => { let a = self.pop()?; let v = self.pop()?; let a = self.addr(a, 8)?; self.mem[a..a + 8].copy_from_slice(&v.to_le_bytes()) }
            CFetch => { let a = self.pop()?; let a = self.addr(a, 1)?; let v = self.mem[a] as i64; self.push(v) }
            CStore => { let a = self.pop()?; let v = self.pop()?; let a = self.addr(a, 1)?; self.mem[a] = v as u8 }
            PlusStore => { let a = self.pop()?; let v = self.pop()?; let a = self.addr(a, 8)?; let old = i64::from_le_bytes(self.mem[a..a + 8].try_into()?); self.mem[a..a + 8].copy_from_slice(&old.wrapping_add(v).to_le_bytes()) }
            Here => { let h = self.mem.len() as i64; self.push(h) }
            Allot => { let n = self.pop()?; if !(0..=1 << 20).contains(&n) { bail!("bad allot {n}") } self.alloc(n as usize); }
            Comma => { let v = self.pop()?; let a = self.alloc(8); self.mem[a..a + 8].copy_from_slice(&v.to_le_bytes()) }
            Cells => self.un(|a| a.wrapping_mul(8))?,
            ToR => { let v = self.pop()?; self.rs.push(v) }
            RFrom => { let v = self.rs.pop().context("return stack underflow")?; self.push(v) }
            RFetch | I => { let v = *self.rs.last().context("return stack underflow")?; self.push(v) }
            J => { let n = self.rs.len(); if n < 3 { bail!("j outside nested loop") } let v = self.rs[n - 3]; self.push(v) }
            Execute => { let xt = self.pop()?; self.exec(xt as usize, t)? }
            Dot => { let v = self.pop()?; t.emit(&format!("{v} ")) }
            Emit => { let v = self.pop()?; t.emit(&char::from_u32(v as u32).unwrap_or('?').to_string()) }
            Cr => t.emit("\n"),
            Space => t.emit(" "),
            Type => { let s = self.pop_str()?; t.emit(&s) }
            Words => { let names: Vec<_> = self.words.iter().rev().map(|w| w.name.as_str()).collect(); t.emit(&format!("{}\n", names.join(" "))) }
            Send => { let s = self.pop_str()?; self.tail = link::send_checked(t, &self.settings.proto, &s)? }
            Run => { let mut b = self.pop_str()?.into_bytes(); b.extend_from_slice(&self.settings.proto.eol); self.tail.clear(); t.write(&b)? }
            Raw => { let s = self.pop_str()?; self.tail.clear(); t.write(s.as_bytes())? }
            Upload => { let s = self.pop_str()?; self.upload_file(Path::new(&s), t)?; }
            Expect => { let ms = self.pop()?; let s = self.pop_str()?; let tail = std::mem::take(&mut self.tail); let ok = tail.contains(&s) || link::expect(t, &s, millis(ms)?)?; self.push(flag(ok)) }
            Delay => { let ms = self.pop()?; link::delay(t, millis(ms)?)? }
            Included => { let s = self.pop_str()?; let p = self.resolve(Path::new(&s)); self.eval_named(&p, t)? }
            Port => self.settings.port = self.pop_str()?,
            Baud => self.settings.baud = u32::try_from(self.pop()?).context("bad baud rate")?,
            Eol => { let c = self.pop()?; self.settings.proto.eol = if c == 0 { b"\r\n".to_vec() } else { vec![c as u8] } }
            Prompt => self.settings.proto.ok = self.pop_str()?,
            Error => { let s = self.pop_str()?; self.settings.proto.errors.push(s) }
            NoErrors => self.settings.proto.errors.clear(),
            Timeout => self.settings.proto.timeout = millis(self.pop()?)?,
            AddPath => { let s = self.pop_str()?; let p = self.settings.base.join(s); self.settings.path.push(p) }
            HistFile => { let s = self.pop_str()?; self.settings.history = (!s.is_empty()).then(|| self.settings.base.join(s)) }
            Forget => self.required.clear(),
            Bye => self.quit = true,
        }
        Ok(())
    }

    fn push(&mut self, v: i64) {
        self.ds.push(v);
    }

    fn pop(&mut self) -> Result<i64> {
        self.ds.pop().context("stack underflow")
    }

    fn pop2(&mut self) -> Result<(i64, i64)> {
        let b = self.pop()?;
        let a = self.pop()?;
        Ok((a, b))
    }

    fn peek(&self, n: usize) -> Result<i64> {
        self.ds
            .iter()
            .rev()
            .nth(n)
            .copied()
            .context("stack underflow")
    }

    fn bin(&mut self, f: impl Fn(i64, i64) -> Result<i64>) -> Result<()> {
        let (a, b) = self.pop2()?;
        let v = f(a, b)?;
        self.push(v);
        Ok(())
    }

    fn un(&mut self, f: impl Fn(i64) -> i64) -> Result<()> {
        let a = self.pop()?;
        self.push(f(a));
        Ok(())
    }

    fn alloc(&mut self, n: usize) -> usize {
        let a = self.mem.len();
        self.mem.resize(a + n, 0);
        a
    }

    fn addr(&self, a: i64, n: usize) -> Result<usize> {
        usize::try_from(a)
            .ok()
            .filter(|&a| a + n <= self.mem.len())
            .ok_or_else(|| anyhow!("invalid address {a}"))
    }

    fn store_str(&mut self, s: &str) -> (usize, usize) {
        let a = self.alloc(s.len());
        self.mem[a..].copy_from_slice(s.as_bytes());
        (a, s.len())
    }

    fn string(&self, a: i64, u: i64) -> Result<String> {
        let n = usize::try_from(u).context("negative length")?;
        let a = self.addr(a, n)?;
        Ok(String::from_utf8_lossy(&self.mem[a..a + n]).into_owned())
    }

    fn pop_str(&mut self) -> Result<String> {
        let (a, u) = self.pop2()?;
        self.string(a, u)
    }
}

fn flag(b: bool) -> i64 {
    if b {
        -1
    } else {
        0
    }
}

fn millis(ms: i64) -> Result<Duration> {
    u64::try_from(ms)
        .map(Duration::from_millis)
        .context("negative time")
}

/// Canonical spelling of a key name: `F1`..`F12` upper case, everything else verbatim.
pub fn normalize_key(k: &str) -> String {
    match k.strip_prefix(['f', 'F']) {
        Some(n) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => format!("F{n}"),
        _ => k.to_string(),
    }
}

fn parse_number(tok: &str) -> Option<i64> {
    let (neg, t) = match tok.strip_prefix('-') {
        Some(r) if !r.is_empty() => (true, r),
        _ => (false, tok),
    };
    let v = if let Some(h) = t.strip_prefix('$') {
        i64::from_str_radix(h, 16).ok()?
    } else if let Some(b) = t.strip_prefix('%') {
        i64::from_str_radix(b, 2).ok()?
    } else if let Some(c) = t.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        let mut it = c.chars();
        let ch = it.next()?;
        if it.next().is_some() {
            return None;
        }
        ch as i64
    } else {
        t.parse().ok()?
    };
    Some(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::fake::Mecrisp;

    fn run(src: &str) -> (Vm, Mecrisp) {
        let mut vm = Vm::new();
        let mut t = Mecrisp::default();
        vm.eval(src, &mut t).unwrap();
        (vm, t)
    }

    #[test]
    fn arithmetic_and_control() {
        let cases: &[(&str, &[i64])] = &[
            ("1 2 +", &[3]),
            ("$10 %101 -3", &[16, 5, -3]),
            ("7 2 / 7 2 mod", &[3, 1]),
            ("1 2 swap over", &[2, 1, 2]),
            (": sq dup * ; 5 sq", &[25]),
            (": t if 1 else 2 then ; -1 t 0 t", &[1, 2]),
            (": c 0 begin 1+ dup 5 = until ; c", &[5]),
            (": s 0 5 0 do i + loop ; s", &[10]),
            (": w 0 begin dup 3 < while 1+ repeat ; w", &[3]),
            (": f dup 1 > if dup 1- recurse * then ; 5 f", &[120]),
            ("variable x 42 x ! 1 x +! x @", &[43]),
            ("10 constant ten ten ten +", &[20]),
            ("s\" abc\" nip char a [char] b drop", &[3, 97]),
            ("1 2 \\ 3 4\n( 5 ) 6", &[1, 2, 6]),
            (": a 1 ; ' a execute", &[1]),
        ];
        for (src, want) in cases {
            let (vm, _) = run(src);
            assert_eq!(vm.stack(), *want, "src {src:?}");
        }
    }

    #[test]
    fn errors_reset_state() {
        let mut vm = Vm::new();
        let mut t = Mecrisp::default();
        let cases = ["nope", "drop", "1 0 /", ": x if ;", "s\" open"];
        for src in cases {
            assert!(vm.eval(src, &mut t).is_err(), "src {src:?}");
            assert!(vm.stack().is_empty());
        }
        vm.eval("1", &mut t).unwrap();
        assert_eq!(vm.stack(), [1]);
    }

    #[test]
    fn output_words() {
        let (_, t) = run(".\" hi\" 1 . cr 65 emit s\" x\" type : g .\" in\" ; g");
        assert_eq!(t.notes, "hi1 \nAxin");
    }

    #[test]
    fn settings_and_binds() {
        let src = "s\" /dev/ttyUSB1\" port 9600 baud 10 eol s\" ok>\" prompt \
                   no-errors s\" Error\" error 500 timeout : m ; ' m bind f5 ' m bind p";
        let (vm, _) = run(src);
        let s = &vm.settings;
        assert_eq!((s.port.as_str(), s.baud), ("/dev/ttyUSB1", 9600));
        assert_eq!(s.proto.eol, b"\n");
        assert_eq!(s.proto.ok, "ok>");
        assert_eq!(s.proto.errors, ["Error"]);
        assert_eq!(s.proto.timeout, Duration::from_millis(500));
        assert_eq!(vm.binds.keys().collect::<Vec<_>>(), ["F5", "p"]);
    }

    #[test]
    fn host_io() {
        let (vm, t) =
            run(": go s\" 1 2 +\" send s\" ok.\" 0 expect s\" ok.\" 0 expect ; go s\" x\" run");
        assert_eq!(t.sent, ["1 2 +", "x"]);
        assert_eq!(
            vm.stack(),
            [-1, 0],
            "first expect sees the send reply, second does not"
        );

        let mut vm = Vm::new();
        let mut t = Mecrisp::default();
        let err = vm.eval("s\" bad\" send", &mut t).unwrap_err();
        assert!(format!("{err:#}").contains("not found"), "{err:#}");
    }

    #[test]
    fn upload_and_require() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.fs");
        std::fs::write(&f, "1\n2\nbad\n").unwrap();
        let g = d.path().join("b.fs");
        std::fs::write(&g, "3\n").unwrap();

        let mut vm = Vm::new();
        let mut t = Mecrisp::default();
        assert!(vm.upload_file(&f, &mut t).is_err());
        assert_eq!(vm.last_error.as_ref().map(|e| e.no), Some(3));

        assert_eq!(vm.require_file(&g, &mut t).unwrap(), 1);
        assert_eq!(vm.require_file(&g, &mut t).unwrap(), 0);
        vm.eval("forget-required", &mut t).unwrap();
        assert_eq!(vm.require_file(&g, &mut t).unwrap(), 1);
        assert_eq!(t.sent, ["1", "2", "bad", "3", "3"]);
    }

    #[test]
    fn see_and_list() {
        let src = "\\ header\n: sq ( n -- n*n ) dup * ;\n: t if .\" yes\" else 0 then ;\n5 constant five variable v";
        let (vm, t) = run(&format!("{src}\nsee sq list sq see t see five see dup"));
        assert!(
            t.notes.contains(": sq\n    0  dup\n    1  *\n;"),
            "{}",
            t.notes
        );
        assert!(
            t.notes
                .contains("\\ <input>:2\n: sq ( n -- n*n ) dup * ;\n"),
            "{}",
            t.notes
        );
        assert!(t.notes.contains("0branch -> 3"), "{}", t.notes);
        assert!(t.notes.contains(".\" yes\""), "{}", t.notes);
        assert!(t.notes.contains("5 constant five"), "{}", t.notes);
        assert!(t.notes.contains("dup is a primitive"), "{}", t.notes);
        let mut vm = vm;
        assert!(vm.eval("list five", &mut Mecrisp::default()).is_err());
        assert!(vm.eval("see nope", &mut Mecrisp::default()).is_err());
    }

    #[test]
    fn keys_and_numbers() {
        assert_eq!(normalize_key("f12"), "F12");
        assert_eq!(normalize_key("f"), "f");
        assert_eq!(normalize_key("fx"), "fx");
        assert_eq!(parse_number("-$ff"), Some(-255));
        assert_eq!(parse_number("'a'"), Some(97));
        assert_eq!(parse_number("-"), None);
        assert_eq!(parse_number("1x"), None);
    }
}
