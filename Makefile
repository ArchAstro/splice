.PHONY: all build build-release run test lint fmt check clean

all: build

build:
	cargo build

build-release:
	cargo build --release

# Open the editor (last project, or a new one)
run:
	cargo run --release -p splice-app

test:
	cargo test --workspace

lint:
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all

check:
	cargo check --workspace

clean:
	cargo clean
