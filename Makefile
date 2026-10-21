.PHONY: all build build-release run test lint fmt check clean install install-skill app install-app release-check setup icon

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

# Build splice into ~/.cargo/bin and link the Claude Code skill
install:
	sh scripts/install.sh

# Build Splice.app into target/bundle
app: build-release
	scripts/bundle-macos.sh target/release/splice target/bundle

# Put Splice.app in /Applications
install-app: app
	rm -rf /Applications/Splice.app
	ditto target/bundle/Splice.app /Applications/Splice.app
	@echo "Installed /Applications/Splice.app"

# Validate a release candidate. Tagging and pushing stay explicit human actions.
release-check:
	cargo fmt --all -- --check
	cargo clippy --locked --all-targets --all-features -- -D warnings
	RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features
	cargo test --locked --all-targets --all-features
	cargo audit

# Redraw the app icon from its scene
icon: build-release
	target/release/splice --still assets/src/icon.json assets/icon.png --size 1024x1024

# Install git hooks
setup:
	cp scripts/pre-commit .git/hooks/pre-commit
	chmod +x .git/hooks/pre-commit
	@echo "Git hooks installed"

# Copy the Claude Code skill (make install links it instead)
install-skill:
	mkdir -p ~/.claude/skills/splice
	cp skill/splice/SKILL.md ~/.claude/skills/splice/SKILL.md
