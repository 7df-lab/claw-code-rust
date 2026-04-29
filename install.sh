#!/bin/sh
# install.sh — Download and install the latest devo binary for Linux / macOS.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --version v0.1.2

set -eu

APP="devo"
REPO="7df-lab/devo"
INSTALL_DIR_DEFAULT="${HOME}/.devo/bin"

MUTED="$(printf '\033[0;2m')"
RED="$(printf '\033[0;31m')"
ORANGE="$(printf '\033[38;5;214m')"
NC="$(printf '\033[0m')"

requested_version="${VERSION:-}"
binary_path=""
no_modify_path="false"
install_dir="${DEVO_INSTALL_DIR:-$INSTALL_DIR_DEFAULT}"

usage() {
    cat <<EOF
devo Installer

Usage: install.sh [options]

Options:
    -h, --help              Display this help message
    -v, --version <version> Install a specific version (for example: v0.1.2)
    -b, --binary <path>     Install from a local binary instead of downloading
        --install-dir <dir> Install into a custom directory
        --no-modify-path    Don't modify shell config files

Environment:
    VERSION                 Same as --version
    DEVO_INSTALL_DIR        Same as --install-dir

Examples:
    curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
    curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --version v0.1.2
    ./install.sh --binary ./target/release/devo
EOF
}

print_message() {
    level="$1"
    message="$2"

    case "$level" in
        info) color="$NC" ;;
        warning) color="$ORANGE" ;;
        error) color="$RED" ;;
        *) color="$NC" ;;
    esac

    printf '%b%s%b\n' "$color" "$message" "$NC"
}

die() {
    print_message error "$1" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        -v|--version)
            if [ -n "${2:-}" ]; then
                requested_version="$2"
                shift 2
            else
                die "Error: --version requires a version argument"
            fi
            ;;
        -b|--binary)
            if [ -n "${2:-}" ]; then
                binary_path="$2"
                shift 2
            else
                die "Error: --binary requires a path argument"
            fi
            ;;
        --install-dir)
            if [ -n "${2:-}" ]; then
                install_dir="$2"
                shift 2
            else
                die "Error: --install-dir requires a directory argument"
            fi
            ;;
        --no-modify-path)
            no_modify_path="true"
            shift
            ;;
        *)
            print_message warning "Warning: Unknown option '$1'" >&2
            shift
            ;;
    esac
done

require_command() {
    command_name="$1"
    hint="$2"

    if ! command -v "$command_name" >/dev/null 2>&1; then
        die "$hint"
    fi
}

normalize_version() {
    version="$1"
    version="${version#v}"
    printf 'v%s\n' "$version"
}

detect_target() {
    raw_os="$(uname -s)"
    raw_arch="$(uname -m)"

    case "$raw_os" in
        Linux) os="unknown-linux-gnu" ;;
        Darwin) os="apple-darwin" ;;
        *)
            die "Unsupported OS: $raw_os. This installer supports Linux and macOS. For Windows, use install.ps1."
            ;;
    esac

    case "$raw_arch" in
        x86_64|amd64) arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *)
            die "Unsupported architecture: $raw_arch"
            ;;
    esac

    printf '%s-%s\n' "$arch" "$os"
}

resolve_latest_version() {
    require_command curl "Error: 'curl' is required but not installed."
    require_command sed "Error: 'sed' is required but not installed."

    latest="$(
        curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' \
            | sed -n '1p'
    )"

    if [ -z "$latest" ]; then
        die "Failed to resolve the latest release version"
    fi

    printf '%s\n' "$latest"
}

release_exists() {
    version_tag="$1"

    http_status="$(curl -sSL -o /dev/null -w '%{http_code}' "https://github.com/${REPO}/releases/tag/${version_tag}" || true)"
    [ "$http_status" = "200" ]
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

choose_shell_profile() {
    shell_name="${SHELL##*/}"
    xdg_config_home="${XDG_CONFIG_HOME:-$HOME/.config}"

    case "$shell_name" in
        zsh)
            for candidate in "${ZDOTDIR:-$HOME}/.zshrc" "${ZDOTDIR:-$HOME}/.zshenv" "$xdg_config_home/zsh/.zshrc" "$xdg_config_home/zsh/.zshenv"; do
                if [ -f "$candidate" ]; then
                    printf '%s\n' "$candidate"
                    return
                fi
            done
            printf '%s\n' "${ZDOTDIR:-$HOME}/.zshrc"
            ;;
        bash)
            for candidate in "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.profile" "$xdg_config_home/bash/.bashrc" "$xdg_config_home/bash/.bash_profile"; do
                if [ -f "$candidate" ]; then
                    printf '%s\n' "$candidate"
                    return
                fi
            done
            printf '%s\n' "$HOME/.bashrc"
            ;;
        sh|dash|ksh|ash)
            if [ -f "$HOME/.profile" ]; then
                printf '%s\n' "$HOME/.profile"
            else
                printf '%s\n' "$HOME/.profile"
            fi
            ;;
        fish)
            if [ -f "$HOME/.config/fish/config.fish" ]; then
                printf '%s\n' "$HOME/.config/fish/config.fish"
            else
                printf '%s\n' "$HOME/.config/fish/config.fish"
            fi
            ;;
        *)
            printf '\n'
            ;;
    esac
}

ensure_path_in_profile() {
    target_install_dir="$1"
    profile="$2"
    shell_name="${SHELL##*/}"

    if [ -z "$profile" ]; then
        return 1
    fi

    if [ -e "$profile" ] && [ ! -w "$profile" ]; then
        return 1
    fi

    mkdir -p "$(dirname "$profile")"

    case "$shell_name" in
        fish)
            path_line="fish_add_path $target_install_dir"
            ;;
        *)
            path_line="export PATH=\"$target_install_dir:\$PATH\""
            ;;
    esac

    if [ -f "$profile" ] && grep -F "$path_line" "$profile" >/dev/null 2>&1; then
        return 0
    fi

    {
        printf '\n'
        printf '# added by devo installer\n'
        printf '%s\n' "$path_line"
    } >> "$profile"
}

print_path_hint() {
    target_install_dir="$1"

    if path_contains "$target_install_dir"; then
        return
    fi

    if [ "$no_modify_path" = "true" ]; then
        print_message warning "Add ${target_install_dir} to your PATH to run devo from any terminal:"
        if [ "${SHELL##*/}" = "fish" ]; then
            print_message info "  fish_add_path ${target_install_dir}"
        else
            print_message info "  export PATH=\"${target_install_dir}:\$PATH\""
        fi
        return
    fi

    profile="$(choose_shell_profile)"
    if ensure_path_in_profile "$target_install_dir" "$profile"; then
        print_message info "${MUTED}Updated PATH in ${NC}${profile}"
        print_message info "${MUTED}Open a new terminal or run:${NC}"
        print_message info "  . \"$profile\""
        return
    fi

    print_message warning "Couldn't update your shell profile automatically."
    if [ "${SHELL##*/}" = "fish" ]; then
        print_message info "Add this line to your shell config:"
        print_message info "  fish_add_path ${target_install_dir}"
    else
        print_message info "Add this line to your shell config:"
        print_message info "  export PATH=\"${target_install_dir}:\$PATH\""
    fi
}

check_version() {
    expected_version="$1"

    if ! command -v "$APP" >/dev/null 2>&1; then
        return
    fi

    installed_path="$(command -v "$APP")"
    installed_version="$("$APP" --version 2>/dev/null || printf '')"
    normalized_expected="${expected_version#v}"

    if printf '%s' "$installed_version" | grep -F "$normalized_expected" >/dev/null 2>&1; then
        print_message info "${MUTED}${APP} ${NC}${expected_version}${MUTED} is already installed at ${NC}${installed_path}"
        exit 0
    fi

    if [ -n "$installed_version" ]; then
        print_message info "${MUTED}Found existing ${APP} at ${NC}${installed_path}${MUTED} (${NC}${installed_version}${MUTED})${NC}"
    fi
}

find_extracted_binary() {
    search_dir="$1"
    found_binary="$(find "$search_dir" -name "$APP" -type f | sed -n '1p')"

    if [ -z "$found_binary" ]; then
        die "Failed to locate the ${APP} binary inside the downloaded archive"
    fi

    printf '%s\n' "$found_binary"
}

install_from_binary() {
    source_binary="$1"

    [ -f "$source_binary" ] || die "Binary not found at ${source_binary}"
    mkdir -p "$install_dir"
    cp "$source_binary" "${install_dir}/${APP}"
    chmod 755 "${install_dir}/${APP}"
}

download_and_install() {
    target="$1"
    version_tag="$2"

    require_command curl "Error: 'curl' is required but not installed."
    require_command tar "Error: 'tar' is required but not installed."
    require_command find "Error: 'find' is required but not installed."

    archive_name="${APP}-${version_tag}-${target}.tar.gz"
    archive_url="https://github.com/${REPO}/releases/download/${version_tag}/${archive_name}"

    print_message info ""
    print_message info "${MUTED}Installing ${NC}${APP} ${MUTED}version: ${NC}${version_tag}"
    print_message info "${MUTED}Target: ${NC}${target}"

    tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/${APP}-install.XXXXXX")"
    trap 'rm -rf "$tmp_dir"' EXIT INT TERM

    curl -fL --progress-bar "$archive_url" -o "$tmp_dir/$archive_name"
    tar -xzf "$tmp_dir/$archive_name" -C "$tmp_dir"

    extracted_binary="$(find_extracted_binary "$tmp_dir")"

    mkdir -p "$install_dir"
    install -m 755 "$extracted_binary" "${install_dir}/${APP}"

    rm -rf "$tmp_dir"
    trap - EXIT INT TERM
}


print_banner() {
    printf '\n'
    printf '%b%s%b\n' "$MUTED" "░▒▓███████▓▒░░▒▓████████▓▒░▒▓█▓▒░░▒▓█▓▒░░▒▓██████▓▒░" "$NC"
    printf '%b%s%b\n' "$MUTED" "░▒▓█▓▒░░▒▓█▓▒░▒▓█▓▒░      ░▒▓█▓▒░░▒▓█▓▒░▒▓█▓▒░░▒▓█▓▒░" "$NC"
    printf '%b%s%b\n' "$MUTED" "░▒▓█▓▒░░▒▓█▓▒░▒▓█▓▒░       ░▒▓█▓▒▒▓█▓▒░░▒▓█▓▒░░▒▓█▓▒░" "$NC"
    printf '%b%s%b\n' "$MUTED" "░▒▓█▓▒░░▒▓█▓▒░▒▓██████▓▒░  ░▒▓█▓▒▒▓█▓▒░░▒▓█▓▒░░▒▓█▓▒░" "$NC"
    printf '%b%s%b\n' "$MUTED" "░▒▓█▓▒░░▒▓█▓▒░▒▓█▓▒░        ░▒▓█▓▓█▓▒░ ░▒▓█▓▒░░▒▓█▓▒░" "$NC"
    printf '%b%s%b\n' "$MUTED" "░▒▓█▓▒░░▒▓█▓▒░▒▓█▓▒░        ░▒▓█▓▓█▓▒░ ░▒▓█▓▒░░▒▓█▓▒░" "$NC"
    printf '%b%s%b\n' "$MUTED" "░▒▓███████▓▒░░▒▓████████▓▒░  ░▒▓██▓▒░   ░▒▓██████▓▒░" "$NC"
    printf '\n'
}

main() {
    print_banner
    
    if [ -n "$binary_path" ]; then
        print_message info ""
        print_message info "${MUTED}Installing ${NC}${APP} ${MUTED}from local binary: ${NC}${binary_path}"
        install_from_binary "$binary_path"
    else
        target="$(detect_target)"

        if [ -z "$requested_version" ]; then
            version_tag="$(resolve_latest_version)"
        else
            version_tag="$(normalize_version "$requested_version")"
            if ! release_exists "$version_tag"; then
                die "Release ${version_tag} not found. See https://github.com/${REPO}/releases"
            fi
        fi

        check_version "$version_tag"
        download_and_install "$target" "$version_tag"
    fi

    print_path_hint "$install_dir"

    if [ -n "${GITHUB_ACTIONS:-}" ] && [ "$GITHUB_ACTIONS" = "true" ] && [ -n "${GITHUB_PATH:-}" ]; then
        printf '%s\n' "$install_dir" >> "$GITHUB_PATH"
        print_message info "${MUTED}Added ${NC}${install_dir}${MUTED} to \$GITHUB_PATH${NC}"
    fi

    print_message info "${MUTED}${APP} is ready.${NC}"
    print_message info ""
    print_message info "  cd <project>    ${MUTED}# open your workspace${NC}"
    print_message info "  devo onboard    ${MUTED}# first-run setup${NC}"
    print_message info ""
    print_message info "${MUTED}Docs: ${NC}https://github.com/${REPO}#readme"
}

if ! can_install_to_dir "$install_dir"; then
    die "Install directory is not writable or cannot be created: ${install_dir}"
fi

main
