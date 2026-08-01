#!/bin/sh
# install.sh — Download and install the latest devo binary for Linux / macOS.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --version v0.1.2

set -eu

APP="devo"
CODE_SEARCH_MCP_APP="devo-code-search-mcp"
REPO="7df-lab/devo"
RG_APP="rg"
RG_REPO="BurntSushi/ripgrep"
CODE_SEARCH_MODEL_REPO="minishlab/potion-code-16M"
CODE_SEARCH_MODEL_DIR_NAME="minishlab--potion-code-16M"
CODE_SEARCH_MODEL_FILES="tokenizer.json model.safetensors config.json"
CODE_SEARCH_LOCAL_MODELS_DIR="local-models"
INSTALL_DIR_DEFAULT="${HOME}/.devo/bin"

MUTED="$(printf '\033[0;2m')"
RED="$(printf '\033[0;31m')"
ORANGE="$(printf '\033[38;5;214m')"
NC="$(printf '\033[0m')"

requested_version="${VERSION:-}"
binary_path=""
no_modify_path="false"
offline_mode="false"
install_code_search_model="${DEVO_INSTALL_CODE_SEARCH_MODEL:-}"
install_dir="${DEVO_INSTALL_DIR:-$INSTALL_DIR_DEFAULT}"
skip_app_install="false"

usage() {
    cat <<EOF
devo Installer

Usage: install.sh [options]

Options:
    -h, --help              Display this help message
    -v, --version <version> Install a specific version (for example: v0.1.2)
    -b, --binary <path>     Install from a local binary instead of downloading
        --install-dir <dir> Install into a custom directory
        --install-code-search-model
                            Download the local Hugging Face model used by code_search
        --offline           Install from assets placed next to install.sh without network access
        --no-modify-path    Don't modify shell config files

Environment:
    VERSION                 Same as --version
    DEVO_INSTALL_DIR        Same as --install-dir
    DEVO_SKIP_RG_INSTALL=1 Skip installing the ripgrep sidecar
    DEVO_INSTALL_CODE_SEARCH_MODEL=1
                            Download the local Hugging Face model used by code_search

Examples:
    curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
    curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --version v0.1.2
    curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --install-code-search-model
    sh ./install.sh --offline
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
        --install-code-search-model)
            install_code_search_model="1"
            shift
            ;;
        --offline)
            offline_mode="true"
            shift
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

is_truthy() {
    case "${1:-}" in
        1|true|TRUE|yes|YES|on|ON) return 0 ;;
        *) return 1 ;;
    esac
}

should_install_code_search_model() {
    is_truthy "$install_code_search_model"
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
        Linux) os="unknown-linux-musl" ;;
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

detect_rg_target() {
    raw_os="$(uname -s)"
    raw_arch="$(uname -m)"

    case "$raw_os" in
        Linux)
            case "$raw_arch" in
                x86_64|amd64) printf '%s\n' "x86_64-unknown-linux-musl" ;;
                aarch64|arm64) printf '%s\n' "aarch64-unknown-linux-gnu" ;;
                *) die "Unsupported architecture for ripgrep: $raw_arch" ;;
            esac
            ;;
        Darwin)
            case "$raw_arch" in
                x86_64|amd64) printf '%s\n' "x86_64-apple-darwin" ;;
                aarch64|arm64) printf '%s\n' "aarch64-apple-darwin" ;;
                *) die "Unsupported architecture for ripgrep: $raw_arch" ;;
            esac
            ;;
        *)
            die "Unsupported OS for ripgrep: $raw_os"
            ;;
    esac
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

resolve_latest_ripgrep_version() {
    require_command curl "Error: 'curl' is required but not installed."
    require_command sed "Error: 'sed' is required but not installed."

    latest="$(
        curl -fsSL "https://api.github.com/repos/${RG_REPO}/releases/latest" \
            | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' \
            | sed -n '1p'
    )"

    if [ -z "$latest" ]; then
        die "Failed to resolve the latest ripgrep release version"
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

existing_devo_path() {
    if [ -x "${install_dir}/${APP}" ]; then
        printf '%s\n' "${install_dir}/${APP}"
        return 0
    fi

    command -v "$APP" 2>/dev/null || return 1
}

normalize_devo_version_output() {
    raw_version="$1"

    for part in $raw_version; do
        case "$part" in
            v[0-9]*.[0-9]*.[0-9]*)
                printf '%s\n' "$part"
                return
                ;;
            [0-9]*.[0-9]*.[0-9]*)
                printf 'v%s\n' "$part"
                return
                ;;
        esac
    done

    if [ -n "$raw_version" ]; then
        printf '%s\n' "$raw_version"
    else
        printf 'unknown\n'
    fi
}

installed_devo_version() {
    installed_path="$1"
    raw_version="$("$installed_path" --version 2>/dev/null || printf '')"
    normalize_devo_version_output "$raw_version"
}

print_version_transition() {
    target_version="$1"
    installed_path="$(existing_devo_path || true)"

    if [ -n "$installed_path" ]; then
        current_version="$(installed_devo_version "$installed_path")"
    else
        current_version="not installed"
    fi

    print_message info "${MUTED}Version: ${NC}${current_version}${MUTED} -> ${NC}${target_version}"
}

check_version() {
    expected_version="$1"
    installed_path="$(existing_devo_path || true)"

    if [ -z "$installed_path" ]; then
        return
    fi

    installed_version="$(installed_devo_version "$installed_path")"

    if [ "$installed_version" = "$expected_version" ]; then
        print_message info "${MUTED}${APP} ${NC}${expected_version}${MUTED} is already installed at ${NC}${installed_path}"
        skip_app_install="true"
        if [ "${DEVO_SKIP_RG_INSTALL:-}" = "1" ] || [ -x "${install_dir}/${RG_APP}" ]; then
            if ! should_install_code_search_model; then
                exit 0
            fi
        else
            print_message info "${MUTED}ripgrep sidecar is missing; continuing sidecar installation.${NC}"
        fi

        if should_install_code_search_model; then
            print_message info "${MUTED}code_search model install requested; continuing optional installation.${NC}"
        fi
        return
    fi

    if [ -n "$installed_version" ]; then
        print_message info "${MUTED}Found existing ${APP} at ${NC}${installed_path}${MUTED} (${NC}${installed_version}${MUTED})${NC}"
    fi
}

find_extracted_binary() {
    search_dir="$1"
    binary_name="${2:-$APP}"
    found_binary="$(find "$search_dir" -name "$binary_name" -type f | sed -n '1p')"

    if [ -z "$found_binary" ]; then
        die "Failed to locate the ${binary_name} binary inside the downloaded archive"
    fi

    printf '%s\n' "$found_binary"
}

find_extracted_optional_binary() {
    search_dir="$1"
    binary_name="$2"
    find "$search_dir" -name "$binary_name" -type f | sed -n '1p'
}

find_extracted_rg_binary() {
    search_dir="$1"
    found_binary="$(find "$search_dir" -name "$RG_APP" -type f | sed -n '1p')"

    if [ -z "$found_binary" ]; then
        die "Failed to locate the ${RG_APP} binary inside the downloaded ripgrep archive"
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
    extracted_mcp_binary="$(find_extracted_optional_binary "$tmp_dir" "$CODE_SEARCH_MCP_APP")"

    mkdir -p "$install_dir"
    install -m 755 "$extracted_binary" "${install_dir}/${APP}"
    if [ -n "$extracted_mcp_binary" ]; then
        install -m 755 "$extracted_mcp_binary" "${install_dir}/${CODE_SEARCH_MCP_APP}"
        print_message info "${MUTED}Installed ${NC}${CODE_SEARCH_MCP_APP}${MUTED} sidecar${NC}"
    else
        print_message warning "Optional ${CODE_SEARCH_MCP_APP} binary was not found in the archive."
    fi

    rm -rf "$tmp_dir"
    trap - EXIT INT TERM
}

install_ripgrep_sidecar() {
    if [ "${DEVO_SKIP_RG_INSTALL:-}" = "1" ]; then
        print_message warning "Skipping ripgrep sidecar install because DEVO_SKIP_RG_INSTALL=1."
        return
    fi

    if [ -x "${install_dir}/${RG_APP}" ]; then
        print_message info "${MUTED}ripgrep sidecar is already installed at ${NC}${install_dir}/${RG_APP}"
        return
    fi

    require_command curl "Error: 'curl' is required but not installed."
    require_command tar "Error: 'tar' is required but not installed."
    require_command find "Error: 'find' is required but not installed."

    rg_target="$(detect_rg_target)"
    rg_version="$(resolve_latest_ripgrep_version)"
    archive_name="ripgrep-${rg_version}-${rg_target}.tar.gz"
    archive_url="https://github.com/${RG_REPO}/releases/download/${rg_version}/${archive_name}"

    print_message info ""
    print_message info "${MUTED}Installing ripgrep sidecar ${NC}${rg_version}${MUTED} target: ${NC}${rg_target}"

    tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/${APP}-rg-install.XXXXXX")"
    trap 'rm -rf "$tmp_dir"' EXIT INT TERM

    curl -fL --progress-bar "$archive_url" -o "$tmp_dir/$archive_name"
    tar -xzf "$tmp_dir/$archive_name" -C "$tmp_dir"

    extracted_binary="$(find_extracted_rg_binary "$tmp_dir")"

    mkdir -p "$install_dir"
    install -m 755 "$extracted_binary" "${install_dir}/${RG_APP}"

    rm -rf "$tmp_dir"
    trap - EXIT INT TERM
}

code_search_model_dir() {
    devo_home="${DEVO_HOME:-$HOME/.devo}"
    printf '%s\n' "${devo_home}/${CODE_SEARCH_LOCAL_MODELS_DIR}/${CODE_SEARCH_MODEL_DIR_NAME}"
}

code_search_model_files_present() {
    model_dir="$1"

    for file in $CODE_SEARCH_MODEL_FILES; do
        if [ ! -f "${model_dir}/${file}" ]; then
            return 1
        fi
    done

    return 0
}

install_code_search_model_files() {
    if ! should_install_code_search_model; then
        return
    fi

    require_command curl "Error: 'curl' is required but not installed."

    model_dir="$(code_search_model_dir)"
    mkdir -p "$model_dir"

    if code_search_model_files_present "$model_dir"; then
        print_message info "${MUTED}code_search model is already installed at ${NC}${model_dir}"
        return
    fi

    print_message info ""
    print_message info "${MUTED}Installing code_search model ${NC}${CODE_SEARCH_MODEL_REPO}${MUTED} into ${NC}${model_dir}"

    for file in $CODE_SEARCH_MODEL_FILES; do
        target_file="${model_dir}/${file}"
        if [ -f "$target_file" ]; then
            print_message info "${MUTED}Found existing ${NC}${target_file}"
            continue
        fi

        url="https://huggingface.co/${CODE_SEARCH_MODEL_REPO}/resolve/main/${file}"
        tmp_file="${target_file}.tmp.$$"
        print_message info "${MUTED}Downloading ${NC}${file}"
        if curl -fL --progress-bar "$url" -o "$tmp_file"; then
            mv "$tmp_file" "$target_file"
        else
            rm -f "$tmp_file"
            die "Failed to download code_search model file: ${file}"
        fi
    done

    if ! code_search_model_files_present "$model_dir"; then
        die "code_search model files were not fully installed at ${model_dir}"
    fi
}

installer_asset_dir() {
    dir_name="$(dirname "$0")"
    if ! asset_dir="$(cd "$dir_name" && pwd -P)"; then
        die "Failed to resolve installer asset directory from ${dir_name}"
    fi
    printf '%s\n' "$asset_dir"
}

find_offline_file() {
    asset_dir="$1"
    pattern="$2"

    for candidate in "$asset_dir"/$pattern; do
        if [ -f "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done

    return 1
}

install_offline_devo() {
    asset_dir="$1"
    target="$2"

    if [ -n "$binary_path" ]; then
        print_message info "${MUTED}Installing ${NC}${APP} ${MUTED}from local binary: ${NC}${binary_path}"
        install_from_binary "$binary_path"
        return
    fi

    if [ -f "${asset_dir}/${APP}" ]; then
        print_message info "${MUTED}Installing ${NC}${APP} ${MUTED}from local binary: ${NC}${asset_dir}/${APP}"
        install_from_binary "${asset_dir}/${APP}"
        return
    fi

    archive_path="$(find_offline_file "$asset_dir" "${APP}-*-${target}.tar.gz" || true)"
    if [ -z "$archive_path" ]; then
        die "Offline devo asset not found. Place ${APP}-*-${target}.tar.gz or ${APP} next to install.sh."
    fi

    require_command tar "Error: 'tar' is required but not installed."
    require_command find "Error: 'find' is required but not installed."

    print_message info "${MUTED}Installing ${NC}${APP} ${MUTED}from offline archive: ${NC}${archive_path}"

    tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/${APP}-offline-install.XXXXXX")"
    trap 'rm -rf "$tmp_dir"' EXIT INT TERM

    tar -xzf "$archive_path" -C "$tmp_dir"
    extracted_binary="$(find_extracted_binary "$tmp_dir")"
    extracted_mcp_binary="$(find_extracted_optional_binary "$tmp_dir" "$CODE_SEARCH_MCP_APP")"

    mkdir -p "$install_dir"
    install -m 755 "$extracted_binary" "${install_dir}/${APP}"
    if [ -n "$extracted_mcp_binary" ]; then
        install -m 755 "$extracted_mcp_binary" "${install_dir}/${CODE_SEARCH_MCP_APP}"
        print_message info "${MUTED}Installed ${NC}${CODE_SEARCH_MCP_APP}${MUTED} sidecar${NC}"
    fi

    rm -rf "$tmp_dir"
    trap - EXIT INT TERM
}

install_offline_ripgrep_sidecar() {
    asset_dir="$1"

    if [ "${DEVO_SKIP_RG_INSTALL:-}" = "1" ]; then
        print_message warning "Skipping ripgrep sidecar install because DEVO_SKIP_RG_INSTALL=1."
        return
    fi

    if [ -x "${install_dir}/${RG_APP}" ]; then
        print_message info "${MUTED}ripgrep sidecar is already installed at ${NC}${install_dir}/${RG_APP}"
        return
    fi

    if [ -f "${asset_dir}/${RG_APP}" ]; then
        print_message info "${MUTED}Installing ripgrep sidecar from ${NC}${asset_dir}/${RG_APP}"
        mkdir -p "$install_dir"
        install -m 755 "${asset_dir}/${RG_APP}" "${install_dir}/${RG_APP}"
        return
    fi

    rg_target="$(detect_rg_target)"
    archive_path="$(find_offline_file "$asset_dir" "ripgrep-*-${rg_target}.tar.gz" || true)"
    if [ -z "$archive_path" ]; then
        die "Offline ripgrep asset not found. Place ripgrep-*-${rg_target}.tar.gz or ${RG_APP} next to install.sh."
    fi

    require_command tar "Error: 'tar' is required but not installed."
    require_command find "Error: 'find' is required but not installed."

    print_message info "${MUTED}Installing ripgrep sidecar from offline archive: ${NC}${archive_path}"

    tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/${APP}-offline-rg-install.XXXXXX")"
    trap 'rm -rf "$tmp_dir"' EXIT INT TERM

    tar -xzf "$archive_path" -C "$tmp_dir"
    extracted_binary="$(find_extracted_rg_binary "$tmp_dir")"

    mkdir -p "$install_dir"
    install -m 755 "$extracted_binary" "${install_dir}/${RG_APP}"

    rm -rf "$tmp_dir"
    trap - EXIT INT TERM
}

install_offline_code_search_model_files() {
    asset_dir="$1"
    model_dir="$(code_search_model_dir)"
    nested_model_dir="${asset_dir}/${CODE_SEARCH_MODEL_DIR_NAME}"

    if code_search_model_files_present "$nested_model_dir"; then
        source_dir="$nested_model_dir"
    elif code_search_model_files_present "$asset_dir"; then
        source_dir="$asset_dir"
    else
        die "Offline code_search model files not found. Place ${CODE_SEARCH_MODEL_FILES} next to install.sh or under ${CODE_SEARCH_MODEL_DIR_NAME}/."
    fi

    mkdir -p "$model_dir"
    print_message info "${MUTED}Installing code_search model from ${NC}${source_dir}${MUTED} into ${NC}${model_dir}"

    for file in $CODE_SEARCH_MODEL_FILES; do
        cp "${source_dir}/${file}" "${model_dir}/${file}"
    done

    if ! code_search_model_files_present "$model_dir"; then
        die "code_search model files were not fully installed at ${model_dir}"
    fi
}


print_banner() {
    printf '\n'
    printf '%b%s%b\n' "$MUTED" "██████╗  ███████╗██╗   ██╗ ██████╗" "$NC"
    printf '%b%s%b\n' "$MUTED" "██╔══██╗ ██╔════╝██║   ██║██╔═══██╗" "$NC"
    printf '%b%s%b\n' "$MUTED" "██║  ██║ █████╗  ██║   ██║██║   ██║" "$NC"
    printf '%b%s%b\n' "$MUTED" "██║  ██║ ██╔══╝  ╚██╗ ██╔╝██║   ██║" "$NC"
    printf '%b%s%b\n' "$MUTED" "██████╔╝ ███████╗ ╚████╔╝ ╚██████╔╝" "$NC"
    printf '%b%s%b\n' "$MUTED" "╚═════╝  ╚══════╝  ╚═══╝   ╚═════╝" "$NC"
    printf '\n'
}

main() {
    print_banner

    if [ "$offline_mode" = "true" ]; then
        asset_dir="$(installer_asset_dir)"
        print_message info "${MUTED}Offline asset directory: ${NC}${asset_dir}"
        target="$(detect_target)"
        install_offline_devo "$asset_dir" "$target"
        install_offline_ripgrep_sidecar "$asset_dir"
        install_offline_code_search_model_files "$asset_dir"
    elif [ -n "$binary_path" ]; then
        print_message info ""
        print_message info "${MUTED}Installing ${NC}${APP} ${MUTED}from local binary: ${NC}${binary_path}"
        install_from_binary "$binary_path"
        install_ripgrep_sidecar
        install_code_search_model_files
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

        print_version_transition "$version_tag"
        check_version "$version_tag"
        if [ "$skip_app_install" != "true" ]; then
            download_and_install "$target" "$version_tag"
        fi

        install_ripgrep_sidecar
        install_code_search_model_files
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
