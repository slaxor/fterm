# fterm

A serial terminal for Forth targets (built against Mecrisp-Stellaris). It works like tio, plus:

- uploads that send line by line and wait for the target's `ok.` before the next line, stopping at the first error with `file:line`
- `#include` / `#require` resolved on the host
- macros and config written in Forth, running on the host
- a control socket so neovim or another tmux pane can push files into the open session

## Run

```sh
nix run .            # or: just run
fterm -p /dev/ttyACM0 -b 115200
fterm send dcf77.fs  # through the running session, or directly if none runs
fterm eval 'flash'   # run host Forth in the running session
```

`fterm send` exits non-zero on error and prints `file:line: message` to stderr, so it works as a neovim `makeprg`:

```vim
set makeprg=fterm\ send\ %
set errorformat=%f:%l:\ %m
```

## Keys

| Key | Action |
|-----|--------|
| `ctrl-t q` | quit |
| `ctrl-t :` | host Forth prompt |
| `ctrl-t u` | upload a file (prompt) |
| `ctrl-t r` | repeat last upload |
| `ctrl-t e` | edit the last error location (or last upload) |
| `ctrl-t c` | forget the `#require` list (after a target reset) |
| `ctrl-t h` | help |
| `ctrl-t ctrl-t` | send a literal ctrl-t |
| `ctrl-t <c>`, `F1`..`F12` | macros bound with `bind` |
| `ctrl-c` | abort a running upload or macro |

Pasting with bracketed paste goes through the same upload path, with flow control.

## Line directives

If you type `#` at the start of a line, fterm edits the line locally and handles it itself when you press Enter:

| Directive | Action |
|-----------|--------|
| `#i file`, `#include file` | upload |
| `#r file`, `#require file` | upload once per session |
| `#e [file]` | `$VISUAL`/`$EDITOR` (fallback `nvim`) at `+LINE`; with no argument it opens the last error |
| `#f forth` | run host Forth |

Any other `#` line (`#s`, `# 42`) goes to the target unchanged. If you backspace over the `#`, you're back in passthrough.

The same `#include` / `#require` lines work inside uploaded files. They are resolved relative to the including file, then the `path` directories.

## Config: `.fterm.fs`

fterm loads the nearest `.fterm.fs` found upwards from the working directory, or the file given with `-c`. Relative paths in it resolve against its directory. See `example.fterm.fs`.

Host words on top of a small core Forth (stack, arithmetic, `if else then`, `begin until/again/while repeat`, `do loop i j`, `variable constant create , @ !`, `s" ." ' execute recurse`):

| Word | Stack | |
|------|-------|-|
| `port` | `( a u -- )` | serial device |
| `baud` | `( n -- )` | |
| `timeout` | `( ms -- )` | per-line prompt timeout, default 3000 |
| `eol` | `( c -- )` | 13 CR (default), 10 LF, 0 CRLF |
| `prompt` | `( a u -- )` | success suffix, default `ok.` |
| `error` / `no-errors` | `( a u -- )` / `( -- )` | add an error pattern / clear the defaults |
| `path` | `( a u -- )` | add an include search directory |
| `send` | `( a u -- )` | send a line, wait for `ok.`, throw on error |
| `run` | `( a u -- )` | send a line, don't wait (for words that loop) |
| `raw` | `( a u -- )` | send bytes as they are |
| `upload` | `( a u -- )` | upload a file |
| `expect` | `( a u ms -- f )` | wait for text in the output (includes the reply to the last `send`) |
| `delay` | `( ms -- )` | wait, showing output |
| `bind` | `( xt "key" -- )` | `' w bind F5`, `' w bind p` (ctrl-t p) |
| `included` | `( a u -- )` | load more host Forth |
| `forget-required` | `( -- )` | clear the `#require` list |
| `bye` | `( -- )` | quit |

If you define `on-connect`, it runs after every (re)connect.

Default error patterns (Mecrisp-Stellaris): `not found.`, `Stack underflow`, `Stack not balanced`, `Structures don't match`, `Jump too far`.

## Layout

- `lib/`: core library and public API (source preprocessing, line protocol, host Forth, session, control socket)
- `cli/`: thin binary that only uses `lib`'s public API

## Dev shell

```sh
nix develop
```

## Build & test

```sh
just build
just test
just lint
just doc   # cargo doc --open
```

## License

GPLv3, see [LICENSE](./LICENSE).
