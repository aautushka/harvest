# harvest *f<Tab> trigger — must be sourced after fzf shell integration
# Chains: *f<Tab> → harvest picker; anything else → previous Tab widget
[[ -z "${TMUX:-}" ]] && return

_harvest_prev_tab="${${(z)$(bindkey '^I')}[-1]}"

_harvest_tab_widget() {
  if [[ "$LBUFFER" == *'*f' ]]; then
    _harvest_pick "${LBUFFER%\*f}"
  else
    zle "$_harvest_prev_tab"
  fi
}
zle -N _harvest_tab_widget
bindkey '^I' _harvest_tab_widget
