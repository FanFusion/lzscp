# lzscp

**Lazy SCP** — a local terminal UI that makes it trivial to push files from your
laptop to a remote server and get the absolute remote path back on your
clipboard.

Built for the SSH + tmux + Claude Code / Codex workflow: drop a screenshot in,
paste the returned path into your remote LLM, done.

## Why

When you live in an SSH session, sharing a local file with the remote side
usually means:

1. Open a second tool (Cursor Remote SSH, Finder + `scp`, rsync by hand)
2. Drag or copy the file over
3. Copy the resulting path
4. Paste it back into your remote editor / agent

`lzscp` collapses those four steps into one: **drop a file into the TUI →
path is on your clipboard.**

## Features

- Drop files or paste paths — bracketed paste captures both
- Handles quotes, escaped spaces, `file://` URLs, Chinese + emoji filenames
- rsync backend with live progress (`--info=progress2`)
- Multi-host fan-out with named groups
- Auto / manual sync modes
- Configurable clipboard format (`remote_path`, `scp_style`, `ssh_path`, `custom`)
- Project-local (`.lzscp/config.toml`) or global (`~/.config/lzscp/config.toml`)
  configuration
- Update check from a single `VERSION` file

## Install

### From release binaries

Download from [Releases](https://github.com/FanFusion/lzscp/releases):

```bash
# macOS (Apple Silicon)
curl -L https://github.com/FanFusion/lzscp/releases/latest/download/lzscp-macos-aarch64 \
  -o lzscp && chmod +x lzscp && mv lzscp ~/.local/bin/

# macOS (Intel)
curl -L https://github.com/FanFusion/lzscp/releases/latest/download/lzscp-macos-x86_64 \
  -o lzscp && chmod +x lzscp && mv lzscp ~/.local/bin/

# Linux (x86_64)
curl -L https://github.com/FanFusion/lzscp/releases/latest/download/lzscp-linux-x86_64 \
  -o lzscp && chmod +x lzscp && mv lzscp ~/.local/bin/
```

### From source

```bash
cargo install --path .
# or
cargo build --release && install -m 755 target/release/lzscp ~/.local/bin/lzscp
```

## Configure

Copy [`examples/config.toml`](examples/config.toml) to either:

- `~/.config/lzscp/config.toml` — global default
- `$PWD/.lzscp/config.toml` — per-project override (takes priority)

Minimal config:

```toml
default_target = "dev"

[[target]]
name       = "dev"
host       = "dev.example.com"
user       = "ubuntu"
remote_dir = "~/uploads"
```

## Use

```bash
lzscp
```

Then in the TUI:

| Key              | Action                                    |
| ---------------- | ----------------------------------------- |
| `Tab` / `S-Tab`  | Cycle focus between panels                |
| `Up` / `Down`    | Move cursor                               |
| `Space`          | Toggle selected target                    |
| `1`–`9`          | Quick-toggle Nth target                   |
| `Enter`          | Sync (manual mode)                        |
| `a` / `m`        | Auto / manual mode                        |
| `c`              | Cycle clipboard format                    |
| `Backspace`      | Remove queued file under cursor           |
| `x`              | Clear queue                               |
| `?`              | Help overlay                              |
| `q` / `Ctrl+C`   | Quit                                      |

Drag a file from your OS file manager into the terminal, or `Cmd+V` / `Ctrl+Shift+V`
a path from the clipboard. Works with spaces, Chinese characters, and emoji
filenames across iTerm2, Terminal.app, Ghostty, kitty, WezTerm, Alacritty,
GNOME Terminal, and Windows Terminal.

## Requirements

- Local: `rsync` (almost always preinstalled)
- Remote: `rsync` + `ssh` daemon; SSH key auth recommended (no password prompts)

## Troubleshooting

**Pasted paths look garbled or come in character-by-character**
Your terminal may not have bracketed paste enabled. On tmux, ensure you are
running a recent version (3.0+) — passthrough of bracketed paste is default.
On iTerm2: *Preferences → General → Selection → Applications in terminal may
access clipboard* must be allowed.

**`ssh unreachable` on startup**
lzscp uses `BatchMode=yes` for the preflight, so it fails if your key requires
a passphrase not held by ssh-agent. Start `ssh-agent` and `ssh-add` your key,
or configure a passphrase-less key.

**Remote `~` not expanded**
First transfer to a host runs `ssh host echo $HOME` to resolve it. If that
fails, set `remote_dir` to an absolute path instead.

## License

MIT © FanFusion
