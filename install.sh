#!/bin/sh
# Installs the latest released Splice, and its Claude Code skill.
#
#   curl -fsSL https://raw.githubusercontent.com/ArchAstro/splice/main/install.sh | sh
#
# Splice runs on macOS. It uses ffmpeg for import, playback and export; if
# ffmpeg is missing this offers to install it with Homebrew.
#
# Settings (environment variables):
#   SPLICE_VERSION      release to install, for example v0.1.0 (default: the latest)
#   SPLICE_INSTALL_DIR  where the splice command goes (default: ~/.local/bin)
#   SPLICE_APP_DIR      where Splice.app goes (default: /Applications, else ~/Applications)
#   SPLICE_APP          set to 0 for the command only, without Splice.app
#   SPLICE_FFMPEG       set to 0 to skip installing ffmpeg
#   SPLICE_SKILL        set to 0 to skip the Claude Code skill
#   CLAUDE_SKILLS_DIR   where skills live (default: ~/.claude/skills)
set -eu

repo="ArchAstro/splice"
version="${SPLICE_VERSION:-latest}"
bin_dir="${SPLICE_INSTALL_DIR:-$HOME/.local/bin}"
skills_dir="${CLAUDE_SKILLS_DIR:-$HOME/.claude/skills}"

say() { printf '%s\n' "$*"; }
fail() { printf 'splice install: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || fail "Splice runs on macOS only."
case "$(uname -m)" in
    arm64) arch=arm64 ;;
    x86_64) arch=x64 ;;
    *) fail "no prebuilt Splice for $(uname -m)" ;;
esac

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT INT TERM

if [ "$version" = "latest" ]; then
    base="https://github.com/$repo/releases/latest/download"
else
    base="https://github.com/$repo/releases/download/$version"
fi

# Anonymous download first; if that fails (rate limits, a proxy), a signed-in
# GitHub CLI is tried instead.
fetch() {
    if command -v curl >/dev/null 2>&1 && curl -fsSL --retry 3 -o "$work/$1" "$base/$1" 2>/dev/null; then
        return 0
    fi
    if command -v gh >/dev/null 2>&1; then
        if [ "$version" = "latest" ]; then
            gh release download --repo "$repo" --pattern "$1" --dir "$work" --clobber >/dev/null 2>&1 && return 0
        else
            gh release download "$version" --repo "$repo" --pattern "$1" --dir "$work" --clobber >/dev/null 2>&1 && return 0
        fi
    fi
    return 1
}

# Downloads $1 and checks it against the published checksums.
verified() {
    fetch "$1" || return 1
    [ -f "$work/SHA256SUMS" ] || fetch SHA256SUMS || fail "could not download SHA256SUMS"
    expected="$(awk -v name="$1" '$2 == name { print $1 }' "$work/SHA256SUMS")"
    [ -n "$expected" ] || fail "$1 is not listed in SHA256SUMS"
    actual="$(shasum -a 256 "$work/$1" | cut -d' ' -f1)"
    [ "$expected" = "$actual" ] || fail "checksum mismatch for $1; nothing was installed"
}

mkdir -p "$bin_dir"
skill=""

# Splice.app goes in Applications, and `splice` on the command line is the
# same program inside it. SPLICE_APP=0 installs only the command.
if [ "${SPLICE_APP:-1}" != "0" ] && verified splice-macos-app.zip; then
    apps="${SPLICE_APP_DIR:-/Applications}"
    if [ ! -w "$apps" ]; then
        apps="$HOME/Applications"
        mkdir -p "$apps"
    fi
    rm -rf "$apps/Splice.app"
    ditto -x -k "$work/splice-macos-app.zip" "$apps"
    ln -sf "$apps/Splice.app/Contents/MacOS/splice" "$bin_dir/splice"
    skill="$apps/Splice.app/Contents/Resources/skill/splice/SKILL.md"
    say "Installed $("$bin_dir/splice" --version) to $apps/Splice.app, and the splice command to $bin_dir/splice"
else
    asset="splice-darwin-$arch.tar.gz"
    say "Downloading $asset ($version)..."
    verified "$asset" || fail "could not download $asset from $base"
    mkdir "$work/unpacked"
    tar -xzf "$work/$asset" -C "$work/unpacked"
    # Replace by rename, so a splice that is running keeps its own copy.
    rm -f "$bin_dir/.splice.new"
    cp "$work/unpacked/splice" "$bin_dir/.splice.new"
    chmod 755 "$bin_dir/.splice.new"
    mv -f "$bin_dir/.splice.new" "$bin_dir/splice"
    skill="$work/unpacked/skill/splice/SKILL.md"
    say "Installed $("$bin_dir/splice" --version) to $bin_dir/splice"
fi

# ffmpeg does the decoding and encoding.
if ! command -v ffmpeg >/dev/null 2>&1 && [ ! -x /opt/homebrew/bin/ffmpeg ] && [ ! -x /usr/local/bin/ffmpeg ]; then
    if [ "${SPLICE_FFMPEG:-1}" != "0" ] && command -v brew >/dev/null 2>&1; then
        say "Installing ffmpeg with Homebrew (Splice uses it to import, play and export)..."
        brew install ffmpeg || say "Homebrew could not install ffmpeg; run: brew install ffmpeg"
    else
        say "Splice needs ffmpeg to import, play and export. Install it with: brew install ffmpeg"
    fi
fi

if [ "${SPLICE_SKILL:-1}" != "0" ] && [ -d "$(dirname "$skills_dir")" ]; then
    if [ -L "$skills_dir/splice" ]; then
        say "The skill at $skills_dir/splice is a link (a development checkout); left as it is."
    else
        mkdir -p "$skills_dir/splice"
        cp "$skill" "$skills_dir/splice/SKILL.md"
        say "Installed the Claude Code skill to $skills_dir/splice"
    fi
fi

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) say "Add $bin_dir to your PATH to run splice from anywhere." ;;
esac
say "Open Splice from Applications, or run: splice"
