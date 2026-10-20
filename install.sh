#!/bin/sh

set -eu

PROGRAM=nebula
DEFAULT_REPO=ennerperez/nebula
REPO=${NEBULA_REPO:-$DEFAULT_REPO}
VERSION=${NEBULA_VERSION:-latest}
BIN_DIR=${NEBULA_INSTALL_DIR:-${HOME:-}/.local/bin}
AGENT_MODE=${NEBULA_AGENT:-auto}
INSTALL_CLI=1
INSTALL_SKILL=1
SKILL_ROOTS=
TMP_DIR=

usage() {
    cat <<'EOF'
Install the Nebula CLI and its Agent Skill from a verified GitHub release.

Usage:
  ./install.sh [options]

Options:
  --version <version>    Release tag (for example, v0.1.0). Default: latest
  --repo <owner/repo>    GitHub repository. Default: ennerperez/nebula
  --bin-dir <path>       CLI destination. Default: $NEBULA_INSTALL_DIR or ~/.local/bin
  --agent <agent>        Skill target: auto, all, codex, claude, gemini,
                         opencode, generic, or none. May be repeated.
  --skill-dir <path>     Additional skills root; installs <path>/nebula.
                         May be repeated.
  --no-cli               Do not install the CLI.
  --no-skill             Do not install the Agent Skill.
  -h, --help             Show this help.

Environment overrides:
  NEBULA_REPO, NEBULA_VERSION, NEBULA_INSTALL_DIR, NEBULA_AGENT

The script downloads an archive and SHA256SUMS into a temporary directory,
verifies the archive, then installs files. It never executes downloaded code.
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

note() {
    printf '%s\n' "$*"
}

cleanup() {
    if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
        rm -rf "$TMP_DIR"
    fi
}

append_skill_root() {
    root=$1
    [ -n "$root" ] || die 'skill directory cannot be empty'
    case "
$SKILL_ROOTS
" in
        *"
$root
"*) return ;;
    esac
    if [ -n "$SKILL_ROOTS" ]; then
        SKILL_ROOTS="$SKILL_ROOTS
$root"
    else
        SKILL_ROOTS=$root
    fi
}

add_agent() {
    agent=$1
    case "$agent" in
        codex) append_skill_root "${CODEX_HOME:-${HOME:-}/.codex}/skills" ;;
        claude) append_skill_root "${CLAUDE_CONFIG_DIR:-${HOME:-}/.claude}/skills" ;;
        gemini) append_skill_root "${GEMINI_CLI_HOME:-${HOME:-}}/.gemini/skills" ;;
        opencode) append_skill_root "${XDG_CONFIG_HOME:-${HOME:-}/.config}/opencode/skills" ;;
        generic) append_skill_root "${HOME:-}/.agents/skills" ;;
        all)
            add_agent codex
            add_agent claude
            add_agent gemini
            add_agent opencode
            ;;
        auto|none) AGENT_MODE=$agent ;;
        *) die "unsupported agent '$agent' (use --help)" ;;
    esac
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || die '--version requires a value'
            VERSION=$2
            shift 2
            ;;
        --repo)
            [ "$#" -ge 2 ] || die '--repo requires a value'
            REPO=$2
            shift 2
            ;;
        --bin-dir)
            [ "$#" -ge 2 ] || die '--bin-dir requires a value'
            BIN_DIR=$2
            shift 2
            ;;
        --agent)
            [ "$#" -ge 2 ] || die '--agent requires a value'
            add_agent "$2"
            shift 2
            ;;
        --skill-dir)
            [ "$#" -ge 2 ] || die '--skill-dir requires a value'
            append_skill_root "$2"
            shift 2
            ;;
        --no-cli)
            INSTALL_CLI=0
            shift
            ;;
        --no-skill)
            INSTALL_SKILL=0
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            [ "$#" -eq 0 ] || die 'positional arguments are not supported'
            ;;
        *) die "unknown option '$1' (use --help)" ;;
    esac
done

if [ "$AGENT_MODE" = none ] && [ -z "$SKILL_ROOTS" ]; then
    INSTALL_SKILL=0
fi

[ "$INSTALL_CLI" -eq 1 ] || [ "$INSTALL_SKILL" -eq 1 ] || die 'nothing to install'
[ -n "${HOME:-}" ] || die 'HOME is not set'
[ -n "$BIN_DIR" ] || die 'binary directory cannot be empty'

if [ "$INSTALL_SKILL" -eq 1 ] && [ -z "$SKILL_ROOTS" ]; then
    case "$AGENT_MODE" in
        auto|none) ;;
        *) add_agent "$AGENT_MODE" ;;
    esac
fi

case "$REPO" in
    */*) ;;
    *) die "invalid repository '$REPO'; expected owner/repo" ;;
esac
case "$REPO" in
    *[!A-Za-z0-9._/-]*|*//*|/*|*/) die "invalid repository '$REPO'" ;;
esac
case "$VERSION" in
    latest) ;;
    ''|*[!A-Za-z0-9._-]*) die "invalid version '$VERSION'" ;;
esac

OS=$(uname -s 2>/dev/null || true)
case "$OS" in
    Linux) OS=linux ;;
    Darwin) OS=macos ;;
    MINGW*|MSYS*|CYGWIN*) OS=windows ;;
    *) die "unsupported operating system: ${OS:-unknown}" ;;
esac

ARCH=$(uname -m 2>/dev/null || true)
case "$ARCH" in
    x86_64|amd64) ARCH=x86_64 ;;
    arm64|aarch64) ARCH=aarch64 ;;
    *) die "unsupported architecture: ${ARCH:-unknown}" ;;
esac

if [ "$OS" = windows ]; then
    ARCHIVE="$PROGRAM-$OS-$ARCH.zip"
    BINARY="$PROGRAM.exe"
else
    ARCHIVE="$PROGRAM-$OS-$ARCH.tar.gz"
    BINARY=$PROGRAM
fi

if command -v curl >/dev/null 2>&1; then
    download() {
        curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
            --output "$2" "$1"
    }
elif command -v wget >/dev/null 2>&1; then
    download() {
        wget --https-only --quiet --output-document="$2" "$1"
    }
else
    die 'curl or wget is required'
fi

TMP_DIR=$(mktemp -d 2>/dev/null || mktemp -d -t nebula-install)
trap cleanup EXIT HUP INT TERM

if [ "$VERSION" = latest ]; then
    RELEASE_BASE="https://github.com/$REPO/releases/latest/download"
else
    RELEASE_BASE="https://github.com/$REPO/releases/download/$VERSION"
fi

note "Downloading $ARCHIVE..."
download "$RELEASE_BASE/$ARCHIVE" "$TMP_DIR/$ARCHIVE"
download "$RELEASE_BASE/SHA256SUMS" "$TMP_DIR/SHA256SUMS"

EXPECTED=$(awk -v file="$ARCHIVE" '
    {
        name = $2
        sub(/^\*/, "", name)
        if (name == file) {
            print $1
            exit
        }
    }
' "$TMP_DIR/SHA256SUMS")
[ -n "$EXPECTED" ] || die "$ARCHIVE is missing from SHA256SUMS"
case "$EXPECTED" in
    *[!A-Fa-f0-9]*|'') die "invalid checksum for $ARCHIVE" ;;
esac
[ "${#EXPECTED}" -eq 64 ] || die "invalid SHA-256 length for $ARCHIVE"

if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL=$(sha256sum "$TMP_DIR/$ARCHIVE" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
    ACTUAL=$(shasum -a 256 "$TMP_DIR/$ARCHIVE" | awk '{print $1}')
elif command -v openssl >/dev/null 2>&1; then
    ACTUAL=$(openssl dgst -sha256 "$TMP_DIR/$ARCHIVE" | awk '{print $NF}')
else
    die 'sha256sum, shasum, or openssl is required to verify the download'
fi
[ "$ACTUAL" = "$EXPECTED" ] || die "checksum mismatch for $ARCHIVE"
note "Verified $ARCHIVE"

mkdir "$TMP_DIR/release"
if [ "$OS" = windows ]; then
    command -v unzip >/dev/null 2>&1 || die 'unzip is required for Windows releases'
    unzip -Z1 "$TMP_DIR/$ARCHIVE" > "$TMP_DIR/archive-files" || \
        die "cannot list archive contents: $ARCHIVE"
else
    command -v tar >/dev/null 2>&1 || die 'tar is required'
    tar -tzf "$TMP_DIR/$ARCHIVE" > "$TMP_DIR/archive-files" || \
        die "cannot list archive contents: $ARCHIVE"
fi

if awk '
    {
        entry = $0
        gsub(/\\/, "/", entry)
        if (entry ~ /^\// || entry ~ /^[A-Za-z]:\//) {
            unsafe = 1
            exit
        }
        count = split(entry, parts, "/")
        for (i = 1; i <= count; i++) {
            if (parts[i] == "..") {
                unsafe = 1
                exit
            }
        }
    }
    END { exit unsafe ? 0 : 1 }
' "$TMP_DIR/archive-files"; then
    die "archive contains an unsafe path: $ARCHIVE"
fi

if [ "$OS" = windows ]; then
    unzip -q "$TMP_DIR/$ARCHIVE" -d "$TMP_DIR/release"
else
    tar -xzf "$TMP_DIR/$ARCHIVE" -C "$TMP_DIR/release"
fi

if [ "$INSTALL_CLI" -eq 1 ]; then
    [ -f "$TMP_DIR/release/$BINARY" ] || die "release does not contain $BINARY"
fi
if [ "$INSTALL_SKILL" -eq 1 ]; then
    [ -f "$TMP_DIR/release/skills/nebula/SKILL.md" ] || \
        die 'release does not contain skills/nebula/SKILL.md'
fi

if [ "$INSTALL_CLI" -eq 1 ]; then
    mkdir -p "$BIN_DIR"
    cp "$TMP_DIR/release/$BINARY" "$BIN_DIR/$BINARY"
    chmod 755 "$BIN_DIR/$BINARY"
    note "Installed CLI: $BIN_DIR/$BINARY"
    INSTALLED_VERSION=$("$BIN_DIR/$BINARY" --version) || \
        die "installed CLI failed verification: $BIN_DIR/$BINARY"
    note "Verified CLI: $INSTALLED_VERSION"
    case ":${PATH:-}:" in
        *":$BIN_DIR:"*) ;;
        *) note "Add $BIN_DIR to PATH to run '$PROGRAM' from any directory." ;;
    esac
fi

if [ "$INSTALL_SKILL" -eq 1 ]; then
    if [ "$AGENT_MODE" = auto ] && [ -z "$SKILL_ROOTS" ]; then
        detected=0
        if command -v codex >/dev/null 2>&1 || [ -d "${CODEX_HOME:-$HOME/.codex}" ]; then
            add_agent codex
            detected=1
        fi
        if command -v claude >/dev/null 2>&1 || [ -d "${CLAUDE_CONFIG_DIR:-$HOME/.claude}" ]; then
            add_agent claude
            detected=1
        fi
        if command -v gemini >/dev/null 2>&1 || [ -d "${GEMINI_CLI_HOME:-$HOME}/.gemini" ]; then
            add_agent gemini
            detected=1
        fi
        if command -v opencode >/dev/null 2>&1 || [ -d "${XDG_CONFIG_HOME:-$HOME/.config}/opencode" ]; then
            add_agent opencode
            detected=1
        fi
        if [ "$detected" -eq 0 ]; then
            add_agent generic
            note 'No supported agent detected; using the generic Agent Skills directory.'
        fi
    fi

    old_ifs=$IFS
    IFS='
'
    for root in $SKILL_ROOTS; do
        destination=$root/nebula
        mkdir -p "$destination"
        cp -R "$TMP_DIR/release/skills/nebula/." "$destination/"
        note "Installed Agent Skill: $destination"
    done
    IFS=$old_ifs
fi
