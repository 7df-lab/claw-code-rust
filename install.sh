#!/bin/sh
# install.sh — Download and install the latest devo binary for Linux / macOS.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
#
# You can pin a specific version by setting the VERSION env var:
#   VERSION=v0.1.0 curl -fsSL ... | sh

set -eu

REPO="7df-lab/devo"

# ── Platform detection ───────────────────────────────────────────────────
detect_target() {
    arch="$(uname -m)"
    os="$(uname -s)"

    case "$os" in
        Linux)  os="unknown-linux-gnu" ;;
        Darwin) os="apple-darwin" ;;
        *)
            echo "Unsupported OS: $os"
            exit 1
            ;;
    esac

    case "$arch" in
        x86_64|amd64) arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *)
            echo "Unsupported architecture: $arch"
            exit 1
            ;;
    esac

    echo "${arch}-${os}"
}

# ── Resolve version ──────────────────────────────────────────────────────
resolve_version() {
    if [ "${VERSION:-}" != "" ]; then
        echo "$VERSION"
        return
    fi

    # Fetch the latest release tag from GitHub API (unauthenticated, rate-limited).
    latest="$(
        curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' \
            | sed 's/.*: "//;s/",//'
    )"

    if [ -z "$latest" ]; then
        echo "Failed to resolve latest version" >&2
        exit 1
    fi
    echo "$latest"
}

path_contains() {
    case ":${PATH:-}:" in
        *:"$1":*) return 0 ;;
        *) return 1 ;;
    esac
}

can_install_to_dir() {
    dir="$1"

    if [ -d "$dir" ]; then
        [ -w "$dir" ]
        return
    fi

    parent="$(dirname "$dir")"
    while [ ! -d "$parent" ]; do
        next_parent="$(dirname "$parent")"
        if [ "$next_parent" = "$parent" ]; then
            return 1
        fi
        parent="$next_parent"
    done

    [ -w "$parent" ]
}

print_path_hint() {
    install_dir="$1"

    if path_contains "$install_dir"; then
        return
    fi

    echo
    echo "${install_dir} is not currently in your PATH."
    profile="$(choose_shell_profile)"

    if [ -n "$profile" ] && ensure_path_in_profile "$install_dir" "$profile"; then
        echo "Added it to ${profile}."
        echo "Run:"
        echo "  source \"${profile}\""
        echo "Or restart your terminal."
        return
    fi

    echo "Add it to your shell profile with:"
    echo "  export PATH=\"${install_dir}:\$PATH\""
    if [ -n "$profile" ]; then
        echo "Then run:"
        echo "  source \"${profile}\""
    fi
    echo "Or restart your terminal."
}

choose_install_dir() {
    if [ "${DEVO_INSTALL_DIR:-}" != "" ]; then
        if can_install_to_dir "$DEVO_INSTALL_DIR"; then
            echo "$DEVO_INSTALL_DIR"
            return
        fi

        echo "DEVO_INSTALL_DIR is not writable or cannot be created: ${DEVO_INSTALL_DIR}" >&2
        exit 1
    fi

    if can_install_to_dir /usr/local/bin; then
        echo "/usr/local/bin"
        return
    fi

    for dir in "$HOME/.local/bin" "$HOME/bin"; do
        if path_contains "$dir" && can_install_to_dir "$dir"; then
            echo "$dir"
            return
        fi
    done

    for dir in "$HOME/.local/bin" "$HOME/bin"; do
        if can_install_to_dir "$dir"; then
            echo "$dir"
            return
        fi
    done

    echo "Could not find a writable install directory." >&2
    echo "Set DEVO_INSTALL_DIR to a writable directory in your PATH and rerun the installer." >&2
    exit 1
}

choose_shell_profile() {
    shell_name="${SHELL##*/}"

    case "$shell_name" in
        zsh)
            echo "$HOME/.zshrc"
            ;;
        bash)
            if [ -f "$HOME/.bash_profile" ] || [ "$(uname -s)" = "Darwin" ]; then
                echo "$HOME/.bash_profile"
            elif [ -f "$HOME/.bashrc" ]; then
                echo "$HOME/.bashrc"
            else
                echo "$HOME/.profile"
            fi
            ;;
        sh|dash|ksh)
            echo "$HOME/.profile"
            ;;
        *)
            echo ""
            ;;
    esac
}

ensure_path_in_profile() {
    install_dir="$1"
    profile="$2"
    path_line="export PATH=\"${install_dir}:\$PATH\""
    marker="# added by devo installer"

    if [ -e "$profile" ] && [ ! -w "$profile" ]; then
        return 1
    fi

    if [ -f "$profile" ] && grep -F "$path_line" "$profile" >/dev/null 2>&1; then
        return 0
    fi

    mkdir -p "$(dirname "$profile")"
    {
        echo
        echo "$marker"
        echo "$path_line"
    } >> "$profile"
}

# ── Install ──────────────────────────────────────────────────────────────
main() {
    target="$(detect_target)"
    version="$(resolve_version)"
    archive_url="https://github.com/${REPO}/releases/download/${version}/devo-${version}-${target}.tar.gz"

    echo "Downloading devo ${version} for ${target}..."

    tmpdir="$(mktemp -d)"
    # shellcheck disable=SC2064
    trap "rm -rf '$tmpdir'" EXIT

    curl -fsSL "$archive_url" -o "$tmpdir/devo.tar.gz"
    tar xzf "$tmpdir/devo.tar.gz" -C "$tmpdir"

    install_dir="$(choose_install_dir)"
    mkdir -p "$install_dir"

    # The archive contains a top-level directory like devo-v0.1.0-x86_64-unknown-linux-gnu/devo
    bin_src="$(find "$tmpdir" -name 'devo' -type f | head -1)"
    install -m 755 "$bin_src" "$install_dir/devo"

    echo "Installed devo to ${install_dir}/devo"
    print_path_hint "$install_dir"
    echo "Run 'devo onboard' to get started."
}

main
