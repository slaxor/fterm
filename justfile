test:
    cargo test --workspace

build:
    cargo build --workspace --release

doc:
    cargo doc --workspace --no-deps --open

fmt:
    cargo fmt --all

lint:
    cargo clippy --workspace --all-targets -- -D warnings

fix:
    cargo clippy --workspace --all-targets --fix --allow-dirty
    cargo fmt --all

run *args:
    cargo run -p fterm-cli -- {{args}}
