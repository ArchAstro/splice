.PHONY: all build build-release run test lint fmt check clean install-skill

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

# Teach Claude Code to drive Splice (`splice ctl`)
install-skill:
	mkdir -p ~/.claude/skills/splice
	cp skill/splice/SKILL.md ~/.claude/skills/splice/SKILL.md
