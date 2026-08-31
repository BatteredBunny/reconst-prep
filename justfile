default:
    @just --list

ci: fmt-check clippy test

setup:
    ./scripts/prepare-gyroflow-checkout.sh
    
fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --locked --workspace --all-targets -- -D warnings

test:
    cargo test --locked --release --workspace

test-golden:
    cargo test --locked --release -p reconst-prep-core --test golden -- --nocapture

build:
    cargo build --locked --release -p reconst-prep

run *ARGS:
    cargo run --locked --release -p reconst-prep -- {{ ARGS }}

gui *ARGS:
    cargo run --locked --release -p reconst-prep -- gui {{ ARGS }}

gui-debug *ARGS:
    RUST_BACKTRACE=full RUST_LOG=debug cargo run --locked -p reconst-prep -- gui {{ ARGS }}

nix-build:
    nix build

nix-check:
    nix flake check

appimage:
    ./packaging/appimage/build.sh

clean:
    cargo clean
