#!/usr/bin/env bash
# Build and run the Ceiling Tauri desktop shell on Linux / WSL.
#
# Usage:
#   ./scripts/dev.sh                 # debug build + run
#   ./scripts/dev.sh --release       # optimised build
#   ./scripts/dev.sh --skip-build    # run last build
#   ./scripts/dev.sh --verbose       # enable debug logging when launching desktop shell
#   ./scripts/dev.sh --cli           # run backend CLI instead of the desktop shell

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUST_DIR="$REPO_ROOT/rust"
TAURI_APP_DIR="$REPO_ROOT/apps/desktop-tauri"

# ── Parse arguments ──────────────────────────────────────────────────────────

RELEASE=0
SKIP_BUILD=0
VERBOSE=0
CLI_MODE=0

for arg in "$@"; do
    case "$arg" in
        --release)   RELEASE=1 ;;
        --skip-build) SKIP_BUILD=1 ;;
        --verbose)   VERBOSE=1 ;;
        --cli)       CLI_MODE=1 ;;
        -h|--help)
            echo "Usage: $0 [--release] [--skip-build] [--verbose] [--cli]"
            echo ""
            echo "Options:"
            echo "  --release      Optimised build"
            echo "  --skip-build   Run last build without rebuilding"
            echo "  --verbose      Enable debug logging"
            echo "  --cli          Run backend CLI usage command instead of the desktop shell"
            exit 0
            ;;
        *)
            echo "Unknown option: $arg"
            echo "Run '$0 --help' for usage."
            exit 1
            ;;
    esac
done

# ── Check prerequisites ──────────────────────────────────────────────────────

if ! command -v cargo &>/dev/null; then
    echo "ERROR: cargo (Rust) not found."
    echo "Install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi

# ── Detect native Linux target ────────────────────────────────────────────────

NATIVE_TARGET="$(rustc -vV | awk '/^host:/ { print $2 }')"
TARGET_FLAG=(--target "$NATIVE_TARGET")

# ── WSL detection ─────────────────────────────────────────────────────────────

IS_WSL=0
if grep -qi microsoft /proc/version 2>/dev/null || [ -n "${WSL_DISTRO_NAME:-}" ]; then
    IS_WSL=1
    echo "Detected WSL environment (${WSL_DISTRO_NAME:-unknown})"

    if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ]; then
        echo ""
        echo "No display server detected."
        echo "Desktop mode requires WSLg (Windows 11) or an X server."
        echo "Use --cli to run CLI commands instead."
        echo ""
        if [ "$CLI_MODE" -eq 0 ]; then
            echo "Tip: 'codexbar usage -p claude' works without a display."
            echo ""
            CLI_MODE=1
            echo "Auto-switching to CLI mode."
        fi
    fi
fi

if [ "$CLI_MODE" -eq 0 ] && ! command -v pnpm &>/dev/null; then
    echo "ERROR: pnpm not found."
    echo "Install pnpm to build apps/desktop-tauri before running desktop mode."
    exit 1
fi

find_binary() {
    local binary_name="$1"
    local profile="$2"
    shift 2 || true

    local candidates=(
        "$REPO_ROOT/target/$profile/$binary_name"
    )

    if [ -n "${CARGO_BUILD_TARGET:-}" ]; then
        candidates+=("$REPO_ROOT/target/$CARGO_BUILD_TARGET/$profile/$binary_name")
    fi

    if [ -n "$NATIVE_TARGET" ]; then
        candidates+=("$REPO_ROOT/target/$NATIVE_TARGET/$profile/$binary_name")
    fi

    local candidate
    for candidate in "${candidates[@]}"; do
        if [ -f "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done

    return 1
}

# ── Build ─────────────────────────────────────────────────────────────────────

if [ "$SKIP_BUILD" -eq 0 ]; then
    if [ "$CLI_MODE" -eq 1 ]; then
        if [ "$RELEASE" -eq 1 ]; then
            echo "Building Ceiling CLI (release, target=$NATIVE_TARGET)..."
            cargo build --manifest-path "$RUST_DIR/Cargo.toml" --bin codexbar --release "${TARGET_FLAG[@]}"
        else
            echo "Building Ceiling CLI (debug, target=$NATIVE_TARGET)..."
            cargo build --manifest-path "$RUST_DIR/Cargo.toml" --bin codexbar "${TARGET_FLAG[@]}"
        fi
    else
        cd "$TAURI_APP_DIR"
        if [ "$RELEASE" -eq 1 ]; then
            echo "Building Ceiling (release, no bundle)..."
            pnpm run tauri:build
        else
            echo "Building Ceiling (debug, no bundle)..."
            pnpm run tauri:build:debug
        fi
        cd "$REPO_ROOT"
    fi
fi

# ── Locate binary ─────────────────────────────────────────────────────────────

PROFILE="debug"
[ "$RELEASE" -eq 1 ] && PROFILE="release"

if [ "$CLI_MODE" -eq 1 ]; then
    BINARY_NAME="codexbar"
else
    BINARY_NAME="codexbar-desktop-tauri"
fi

if ! BINARY="$(find_binary "$BINARY_NAME" "$PROFILE")"; then
    echo "ERROR: Binary not found for $BINARY_NAME ($PROFILE)"
    echo "Run without --skip-build to build first."
    exit 1
fi

# ── Run ───────────────────────────────────────────────────────────────────────

echo ""
if [ "$CLI_MODE" -eq 1 ]; then
    echo "Running: codexbar usage -p all"
    RUN_ARGS=(usage -p all)
else
    echo "Running: Ceiling"
fi

if [ "$VERBOSE" -eq 1 ]; then
    if [ "$CLI_MODE" -eq 1 ]; then
        RUN_ARGS=(-v "${RUN_ARGS[@]}")
    else
        export RUST_LOG="${RUST_LOG:-debug}"
        echo "Verbose logging enabled via RUST_LOG=$RUST_LOG"
    fi
fi
if [ "$CLI_MODE" -eq 0 ] && [ -z "${TAURI_DEV:-}" ]; then
    export TAURI_DEV=0
fi

if [ "$CLI_MODE" -eq 1 ]; then
    "$BINARY" "${RUN_ARGS[@]}"
else
    "$BINARY"
fi
