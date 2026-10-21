#!/bin/sh
# Builds and installs Splice from a checkout, and links its Claude Code skill.
#
#   sh scripts/install.sh            from a checkout
#   gh repo clone ArchAstro/splice && sh splice/scripts/install.sh
#
# Puts the `splice` binary in ~/.cargo/bin and links the skill into
# ~/.claude/skills/splice, so it follows the checkout.
set -eu

repo="$(cd "$(dirname "$0")/.." && pwd)"

[ "$(uname -s)" = "Darwin" ] || { echo "Splice runs on macOS only." >&2; exit 1; }
if ! command -v cargo >/dev/null 2>&1; then
    echo "Splice needs Rust to build. Install it from https://rustup.rs and run this again." >&2
    exit 1
fi

echo "Building Splice (a few minutes the first time)..."
cargo install --path "$repo/crates/splice-app" --locked

skills="${CLAUDE_SKILLS_DIR:-$HOME/.claude/skills}"
mkdir -p "$skills"
if [ -e "$skills/splice" ] || [ -L "$skills/splice" ]; then
    echo "Skill already present at $skills/splice; leaving it as it is."
else
    # A link, so the skill follows the checkout when it is updated.
    ln -s "$repo/skill/splice" "$skills/splice"
    echo "Linked the skill into $skills/splice"
fi

if ! command -v ffmpeg >/dev/null 2>&1 && [ ! -x /opt/homebrew/bin/ffmpeg ] && [ ! -x /usr/local/bin/ffmpeg ]; then
    echo "Splice needs ffmpeg to import, play and export: brew install ffmpeg"
fi

case ":$PATH:" in
    *":$HOME/.cargo/bin:"*) ;;
    *) echo "Add ~/.cargo/bin to your PATH to run splice from anywhere." ;;
esac
echo "Done. Open Splice with: splice"
