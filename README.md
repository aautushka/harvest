# harvest

Pick file paths from your tmux scrollback and insert them at the cursor. Like fzf's `**<Tab>` completion, but for anything that appeared in your terminal output.

## How it works

When triggered, harvest:

1. Captures the last 10 000 lines of the current tmux pane's scrollback
2. Extracts candidate paths — absolute (`/foo/bar`), relative (`./src/main.rs`, `src/lib.rs`), and bare filenames with extensions (`main.rs`, `config.yaml`)
3. Filters to paths that actually exist on disk (resolving relative paths against the current working directory)
4. Pipes the results into fzf for interactive selection
5. Inserts the selected path at the cursor position

### Path extraction details

- **Absolute paths** — any token starting with `/`. Backslash-escaped spaces are handled (`Screen\ Recording.mov` → `Screen Recording.mov`). Also tries to greedily join adjacent words for unescaped `ls`-style output.
- **Relative paths** — any token containing `/` but not starting with it (`./foo`, `src/main.rs`, `../config`).
- **Dot-words** — bare filenames like `main.rs` or `config.yaml`. Only extracted when prompt tracking is active (see below).
- **Trailing junk stripped** — `,`, `;`, and `:N` line-number suffixes (e.g. `file.rs:42` → `file.rs`).

### CWD tracking

When `--prompt` is passed, harvest parses your ZSH prompt format string to detect prompt lines in the scrollback. It then walks backward through the last 20 commands, undoing any `cd` commands it finds, so relative paths from older commands are resolved against the directory they were run in — not your current one.

This means if you ran `find . | grep main` in `~/proj/harvest` and then `cd src`, harvest will still show `./src/main.rs` correctly.

Supported prompt codes: `%{...%}` color groups, `%(cond:A:B)` conditionals (keeps the true branch), `%1{text%}` literal blocks, `$(...)` command substitutions (stripped — their output is dynamic). Any prompt that contains a stable literal like `➜` will work.

## Requirements

- [Rust](https://rustup.rs) (to build)
- [tmux](https://github.com/tmux/tmux)
- [fzf](https://github.com/junegunn/fzf)
- ZSH (for the shell widget; the tmux binding works in any shell)

## Installation

```bash
git clone https://github.com/you/harvest
cd harvest
./install.sh        # dev mode (default)
# or
./install.sh system # copies binary to ~/.local/bin
```

**dev mode** — builds in place, tmux and ZSH configs point at `./target/release/harvest`. Rebuilding takes effect immediately with no reinstall.

**system mode** — copies the binary to `~/.local/bin/harvest` and writes config files to `~/.config/harvest/`. Ensure `~/.local/bin` is in your `PATH`.

The installer will:
- Build the release binary with `cargo build --release`
- Write `tmux.conf` and `zsh.zsh` (with the binary path substituted)
- Offer to patch your `~/.tmux.conf` to source the generated tmux config
- Offer to patch your `~/.zshrc` to source the ZSH widget
- Optionally install the `*f<Tab>` trigger (must come after fzf's own shell integration in `.zshrc`)

After patching `.zshrc`, either `source ~/.zshrc` or open a new shell.  
After patching `.tmux.conf`, either run `tmux source-file ~/.tmux.conf` or start a new session.

## Usage

### ZSH widget — `Ctrl-F`

Opens fzf inline (below your prompt) with paths from the scrollback. The selected path is inserted at the cursor. Works with existing text in the buffer — the path is appended at the cursor position.

Requires tmux.

### ZSH widget — `*f<Tab>` (optional)

Type `*f` at the end of any word and press `Tab`. If the line ends in `*f`, harvest runs and the `*f` is replaced with the selected path. Otherwise `Tab` falls through to its previous binding (fzf completion, normal completion, etc.).

Must be sourced *after* fzf's shell integration in `.zshrc`.

### tmux binding — `Prefix + f`

Opens fzf in a popup window. The selected path is sent to the pane with spaces escaped (`foo\ bar`). Works in any shell running inside tmux.

Reads the prompt pattern from `~/.cache/harvest/prompt` (written by the ZSH widget on shell startup). Without this file, prompt tracking and dot-word extraction are disabled.

### Inserted path format

Paths are inserted backslash-escaped (spaces become `\ `), matching what ZSH would produce for tab-completed paths. This is shell-safe and compatible with most commands.

## Debugging

If paths you expect are missing, set `HARVEST_DEBUG=1` before running harvest manually:

```bash
HARVEST_DEBUG=1 tmux capture-pane -p -t "$TMUX_PANE" -S -10000 | \
  harvest --cwd "$PWD" --prompt "$PROMPT" > /dev/null
cat /tmp/harvest_debug.txt
```

The debug log shows:
- `cwd` and `prompt` arguments received
- The prompt literals extracted from the format string
- Which lines were identified as prompt lines
- The per-section CWD map (how `cd` tracking reconstructed past working directories)
- Each relative path candidate and whether it resolved to an existing file

Common issues:

| Symptom | Likely cause |
|---|---|
| Only absolute paths shown | `--prompt` not passed or no literals found in prompt pattern |
| Relative paths missing after `cd` | `undo_cd` couldn't reconstruct — try `cd path/component` rather than `cd -` or absolute paths |
| Dot-words (`main.rs`) not shown | Prompt tracking not active (no `--prompt` or no matching literals) |
| Paths shown that don't exist | Shouldn't happen — existence-checked at runtime |
| `*f<Tab>` inserts single quotes | ZSH widget sourced without `${(q)selected}` — reinstall |

## How the ZSH widget inserts paths

The widget uses `${(q)selected}` (ZSH backslash quoting) so spaces become `\ `. This matches what you'd get from pressing Tab on a filename — safe to paste into any command without extra quoting.
