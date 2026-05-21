# harvest ZSH widget — insert scrollback paths at cursor via Ctrl-F
[[ -z "${TMUX:-}" ]] && return

_harvest_pick() {
  setopt localoptions noglobsubst noposixbuiltins pipefail no_aliases 2>/dev/null
  local selected
  selected=$(tmux capture-pane -p -t "$TMUX_PANE" -S -10000 | /Users/anton/proj/scrollback/target/release/harvest --cwd "$PWD" --prompt "$PROMPT" | fzf --reverse --height 40%)
  [[ -n "$selected" ]] && LBUFFER="${1}${selected}"
  zle reset-prompt
}

_harvest_widget() { _harvest_pick "$LBUFFER"; }
zle -N _harvest_widget
bindkey '^F' _harvest_widget

mkdir -p "${XDG_CACHE_HOME:-$HOME/.cache}/harvest"
print -r -- "$PROMPT" > "${XDG_CACHE_HOME:-$HOME/.cache}/harvest/prompt"
