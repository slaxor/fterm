# fterm

## Layout

- `lib/` — core library, public API
- `cli/` — thin binary, consumes `lib`'s public API only

## Dev shell

```sh
nix develop
```

## Build & test

```sh
cargo build
cargo test
just doc   # cargo doc --open
```

## License

GPLv3, see [LICENSE](./LICENSE).
