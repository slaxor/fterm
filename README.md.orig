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

## Install with home-manager

```nix
# flake inputs
fterm.url = "https://github.com/slaxor/fterm.git";

# home configuration
imports = [ inputs.fterm.homeManagerModules.default ];
programs.fterm = {
  enable = true;
  init = ''
    2 timestamps
    : reboot  s" reset" run  2000 delay ;
    ' reboot bind F6
  '';
};
```

`init` is written to `~/.config/fterm/init.fs`, which loads before the project's `.fterm.fs`. The package is built against your own `pkgs`, and `overlays.default` provides `pkgs.fterm`. For the serial port your user needs to be in the `dialout` group (`users.users.<you>.extraGroups` on NixOS).

## Input

Lines are edited locally, readline style, and sent with Enter:

| Key | Action |
|-----|--------|
| `up`/`down`, `ctrl-p`/`ctrl-n` | history |
| `ctrl-r` | reverse history search (`ctrl-r` again for older, Enter runs, Esc cancels) |
| `ctrl-a`/`ctrl-e`, `home`/`end` | line start / end |
| `ctrl-b`/`ctrl-f`, `left`/`right` | char back / forward |
| `alt-b`/`alt-f`, `ctrl-left`/`ctrl-right` | word back / forward |
| `ctrl-w`, `ctrl-u`, `ctrl-k`, `ctrl-y` | kill word / to start / to end, yank |
| `ctrl-c` | discard the line |

After Enter, keys go raw to the target until it prompts with `ok.` (or reports an error), so words that loop until a keypress (`log`, `decode`) keep working. History is shared by target lines, `#` directives and host Forth. It is saved to `$XDG_STATE_HOME/fterm/history` (`~/.local/state/fterm/history`), or to the file you give `history` in `.fterm.fs`.

## Keys

| Key | Action |
|-----|--------|
| `ctrl-t q` | quit |
| `ctrl-t :` | host Forth prompt |
| `ctrl-t u` | upload a file (prompt) |
| `ctrl-t r` | repeat last upload |
| `ctrl-t e` | edit the last error location (or last upload) |
| `ctrl-t c` | forget the `#require` list (after a target reset) |
| `ctrl-t l` | toggle raw passthrough (no local editing) |
| `ctrl-t h` | help |
| `ctrl-t ctrl-t` | send a literal ctrl-t |
| `ctrl-t <c>`, `F1`..`F12` | macros bound with `bind` |
| `ctrl-c` | abort a running upload or macro |

A single-line paste goes into the line editor. A multi-line paste goes through the upload path, with flow control.

## Line directives

Lines starting with `#` are handled by fterm itself:

| Directive | Action |
|-----------|--------|
| `#i file`, `#include file` | upload |
| `#r file`, `#require file` | upload once per session |
| `#e [file]` | `$VISUAL`/`$EDITOR` (fallback `nvim`) at `+LINE`; with no argument it opens the last error |
| `#f forth` | run host Forth |

Any other `#` line (`#s`, `# 42`) goes to the target unchanged.

The same `#include` / `#require` lines work inside uploaded files. They are resolved relative to the including file, then the `path` directories.

## Config: `.fterm.fs`

fterm loads `~/.config/fterm/init.fs` (`$XDG_CONFIG_HOME`) if it exists, then the nearest `.fterm.fs` found upwards from the working directory, or the file given with `-c`. Relative paths in it resolve against its directory. See `example.fterm.fs`.

Host words on top of a small core Forth (stack, arithmetic, `if else then`, `begin until/again/while repeat`, `do loop i j`, `variable constant create , @ !`, `s" ." ' execute recurse`, `words see list`):

| Word | Stack | |
|------|-------|-|
| `port` | `( a u -- )` | serial device |
| `baud` | `( n -- )` | |
| `timeout` | `( ms -- )` | per-line prompt timeout, default 3000 |
| `eol` | `( c -- )` | 13 CR (default), 10 LF, 0 CRLF |
| `prompt` | `( a u -- )` | success suffix, default `ok.` |
| `error` / `no-errors` | `( a u -- )` / `( -- )` | add an error pattern / clear the defaults |
| `path` | `( a u -- )` | add an include search directory |
| `history` | `( a u -- )` | history file, relative to the config; empty string: none |
| `send` | `( a u -- )` | send a line, wait for `ok.`, throw on error |
| `run` | `( a u -- )` | send a line, don't wait (for words that loop) |
| `raw` | `( a u -- )` | send bytes as they are |
| `upload` | `( a u -- )` | upload a file |
| `expect` | `( a u ms -- f )` | wait for text in the output (includes the reply to the last `send`) |
| `delay` | `( ms -- )` | wait, showing output |
| `bind` | `( xt "key" -- )` | `' w bind F5`, `' w bind p` (ctrl-t p) |
| `included` | `( a u -- )` | load more host Forth |
| `forget-required` | `( -- )` | clear the `#require` list |
| `see` | `( "name" -- )` | decompile a host word |
| `list` | `( "name" -- )` | show a colon definition's source with `file:line` |
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
