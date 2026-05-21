#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

MODE="${1:-dev}"
REPO="$(pwd)"

build() {
    echo ">>> Building..."
    cargo build --release
}

render_tmux_conf() {
    local bin="$1"
    sed "s|__BIN__|$bin|g" "$REPO/tmux.conf.template"
}

render_zsh_widget() {
    local bin="$1"
    sed "s|__BIN__|$bin|g" "$REPO/zsh.zsh.template"
}

render_zsh_tab() {
    cat "$REPO/zsh-tab.zsh.template"
}

find_zshrc() {
    local candidates=(
        "${ZDOTDIR:-$HOME}/.zshrc"
        "$HOME/.zshrc"
    )
    for c in "${candidates[@]}"; do
        if [[ -f "$c" ]]; then
            echo "$c"
            return
        fi
    done
}

maybe_patch_zshrc() {
    local zshrc="$1" snippet="$2"
    local line="source $snippet"
    if [[ -z "$zshrc" ]]; then
        echo ">>> No .zshrc found."
        echo "    Add this line yourself:"
        echo "      $line"
        return
    fi
    if grep -qsF "$line" "$zshrc"; then
        echo ">>> $zshrc already sources $snippet — nothing to patch."
        return
    fi
    echo ">>> Found zshrc at: $zshrc"
    read -r -p "    Append \`$line\` to it? [y/N] " ans
    case "$ans" in
        y|Y|yes)
            printf '\n# harvest\n%s\n' "$line" >> "$zshrc"
            echo "    Patched."
            ;;
        *)
            echo "    Skipped. Add it manually when ready."
            ;;
    esac
}

maybe_patch_zshrc_tab() {
    local zshrc="$1" snippet="$2"
    local line="source $snippet"
    if [[ -z "$zshrc" ]]; then
        echo ">>> No .zshrc found; skipping *f<Tab> trigger."
        return
    fi
    if grep -qsF "$line" "$zshrc"; then
        echo ">>> $zshrc already sources $snippet — nothing to patch."
        return
    fi
    echo
    echo "    Optional: *f<Tab> trigger (chains after fzf's own ** completion)."
    echo "    Must be sourced after fzf shell integration in .zshrc."
    read -r -p "    Enable *f<Tab> trigger? [y/N] " ans
    case "$ans" in
        y|Y|yes)
            printf '\n# harvest *f<Tab> trigger (keep after fzf shell integration)\n%s\n' "$line" >> "$zshrc"
            echo "    Patched. Make sure this line stays after fzf's source line in $zshrc."
            ;;
        *)
            echo "    Skipped. To add later: $line"
            ;;
    esac
}

find_tmux_config() {
    local candidates=(
        "$HOME/.tmux.conf"
        "$HOME/.config/tmux/tmux.conf"
        "${XDG_CONFIG_HOME:-$HOME/.config}/tmux/tmux.conf"
    )
    for c in "${candidates[@]}"; do
        if [[ -f "$c" ]]; then
            echo "$c"
            return
        fi
    done
}

maybe_patch_tmux_config() {
    local config="$1" snippet="$2"
    local line="source-file $snippet"
    if [[ -z "$config" ]]; then
        echo ">>> No tmux config found at any of ~/.tmux.conf, ~/.config/tmux/tmux.conf."
        echo "    Add this line yourself once you create one:"
        echo "      $line"
        return
    fi
    if grep -qsF "$line" "$config"; then
        echo ">>> $config already sources $snippet — nothing to patch."
        return
    fi
    echo ">>> Found tmux config at: $config"
    read -r -p "    Append \`$line\` to it? [y/N] " ans
    case "$ans" in
        y|Y|yes)
            printf '\n# harvest\n%s\n' "$line" >> "$config"
            echo "    Patched."
            ;;
        *)
            echo "    Skipped. Add it manually when ready."
            ;;
    esac
}

case "$MODE" in
    dev)
        build
        render_tmux_conf "$REPO/target/release/harvest" > "$REPO/tmux.conf"
        render_zsh_widget "$REPO/target/release/harvest" > "$REPO/zsh.zsh"
        render_zsh_tab > "$REPO/zsh-tab.zsh"
        if [[ -n "${TMUX:-}" ]]; then
            echo ">>> Sourcing tmux config..."
            tmux source-file "$REPO/tmux.conf"
        fi
        ZSHRC="$(find_zshrc)"
        maybe_patch_tmux_config "$(find_tmux_config)" "$REPO/tmux.conf"
        maybe_patch_zshrc "$ZSHRC" "$REPO/zsh.zsh"
        maybe_patch_zshrc_tab "$ZSHRC" "$REPO/zsh-tab.zsh"
        echo
        echo ">>> Dev install done. Rebuilds in this directory take effect immediately."
        ;;
    system)
        BIN_DIR="$HOME/.local/bin"
        CONFIG_DIR="$HOME/.config/harvest"
        mkdir -p "$BIN_DIR" "$CONFIG_DIR"
        build
        cp "$REPO/target/release/harvest" "$BIN_DIR/"
        render_tmux_conf "$BIN_DIR/harvest" > "$CONFIG_DIR/tmux.conf"
        render_zsh_widget "$BIN_DIR/harvest" > "$CONFIG_DIR/zsh.zsh"
        render_zsh_tab > "$CONFIG_DIR/zsh-tab.zsh"
        if [[ -n "${TMUX:-}" ]]; then
            echo ">>> Sourcing tmux config..."
            tmux source-file "$CONFIG_DIR/tmux.conf"
        fi
        ZSHRC="$(find_zshrc)"
        maybe_patch_tmux_config "$(find_tmux_config)" "$CONFIG_DIR/tmux.conf"
        maybe_patch_zshrc "$ZSHRC" "$CONFIG_DIR/zsh.zsh"
        maybe_patch_zshrc_tab "$ZSHRC" "$CONFIG_DIR/zsh-tab.zsh"
        echo
        echo ">>> System install done."
        echo "    Binary:      $BIN_DIR/harvest"
        echo "    Tmux config: $CONFIG_DIR/tmux.conf"
        echo "    ZSH widget:  $CONFIG_DIR/zsh.zsh"
        echo "    ZSH tab:     $CONFIG_DIR/zsh-tab.zsh"
        echo "    Ensure $BIN_DIR is in your PATH."
        ;;
    *)
        cat <<EOF
usage: $0 [dev|system]
  dev     (default) build in place; tmux conf points at $REPO/target/release/harvest
  system  copy binary to ~/.local/bin and write tmux conf to ~/.config/harvest
EOF
        exit 1
        ;;
esac
