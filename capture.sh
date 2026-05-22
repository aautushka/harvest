#!/usr/bin/env zsh
# harvest-capture — snapshot inputs for a harvest bug report.
# Usage: harvest-capture [LINES]   (default: 200)
# Writes to ~/.cache/harvest/capture_TIMESTAMP.txt and prints the path.

[[ -z "${TMUX:-}" ]] && { print "harvest-capture: must be run inside tmux" >&2; exit 1; }

LINES=${1:-200}
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/harvest"

# Capture scrollback FIRST — printing anything before this would corrupt it.
scrollback=$(tmux capture-pane -p -t "$TMUX_PANE" -S -$LINES)

print -r -- "=== HARVEST CAPTURE ==="
print -r -- "date:   $(date)"
print -r -- "lines:  $LINES"
print -r -- "pane:   $TMUX_PANE"
print -r -- ""
print -r -- "=== CWD ==="
print -r -- "$PWD"
print -r -- ""
print -r -- "=== PROMPT ==="
cat "$CACHE/prompt" 2>/dev/null || print -r -- "(no prompt file)"
print -r -- ""
print -r -- "=== CWD_LOG ==="
cat "$CACHE/cwd_${TMUX_PANE}" 2>/dev/null || print -r -- "(no cwd log)"
print -r -- ""
print -r -- "=== SCROLLBACK ==="
print -r -- "$scrollback"
