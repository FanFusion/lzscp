# lzscp

> Because `scp` is 3 characters but still too much trouble.

## What is this?

A local TUI that makes pushing a file from your laptop to an SSH server brain-dead simple:

**drop file into TUI → remote absolute path lands on your clipboard → paste into Claude Code on the remote box → done.**

## Origin Story

I SSH into a server all day and run Claude Code / Codex inside tmux. Every time I wanted to show the agent a local screenshot, the flow was:

1. Open Cursor Remote SSH (wait forever)
2. Drag file into a folder
3. Copy the path
4. Paste into Claude Code
5. Curse

So I made `lzscp`. Drag into TUI, get path, paste. One step.

Like lzgit, I don't write Rust. Claude Code wrote all of it. I mostly said "no, spaces and Chinese filenames *also* need to work."

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/FanFusion/lzscp/main/install.sh | bash
```

Auto-detects Linux / macOS, x86_64 / arm64, downloads the matching binary to `~/.local/bin/lzscp`. Works on everything the GH Actions matrix builds.

Overrides:

```bash
# Install to somewhere else
INSTALL_DIR=/usr/local/bin curl -fsSL https://raw.githubusercontent.com/FanFusion/lzscp/main/install.sh | bash

# Pin to a specific version
VERSION=v0.1.0 curl -fsSL https://raw.githubusercontent.com/FanFusion/lzscp/main/install.sh | bash
```

### Manual download

Grab a prebuilt binary from [Releases](https://github.com/FanFusion/lzscp/releases/latest):

```bash
# macOS (Apple Silicon)
curl -fsSL https://github.com/FanFusion/lzscp/releases/latest/download/lzscp-macos-aarch64 -o ~/.local/bin/lzscp && chmod +x ~/.local/bin/lzscp

# macOS (Intel)
curl -fsSL https://github.com/FanFusion/lzscp/releases/latest/download/lzscp-macos-x86_64 -o ~/.local/bin/lzscp && chmod +x ~/.local/bin/lzscp

# Linux (x86_64)
curl -fsSL https://github.com/FanFusion/lzscp/releases/latest/download/lzscp-linux-x86_64 -o ~/.local/bin/lzscp && chmod +x ~/.local/bin/lzscp

# Linux (aarch64)
curl -fsSL https://github.com/FanFusion/lzscp/releases/latest/download/lzscp-linux-aarch64 -o ~/.local/bin/lzscp && chmod +x ~/.local/bin/lzscp
```

### From source

```bash
cargo install --path .
# or
cargo build --release && install -m 755 target/release/lzscp ~/.local/bin/lzscp
```

### AI-era install

```
claude "install lzscp from https://raw.githubusercontent.com/FanFusion/lzscp/main/README.md"
```

## Configure

Drop a `config.toml` at either:

- `$PWD/.lzscp/config.toml` — per-project (takes priority)
- `~/.config/lzscp/config.toml` — global fallback

Minimum:

```toml
default_target = "dev"

[[target]]
name       = "dev"
host       = "dev.example.com"
user       = "ubuntu"
remote_dir = "~/uploads"
```

See [`examples/config.toml`](examples/config.toml) for every option (multi-host fan-out, clipboard format, custom templates, SSH port / key, etc.).

## Use

```bash
lzscp
```

Then drag a file in from your OS file manager, or `Cmd+V` / `Ctrl+Shift+V` a path. It rsyncs to the selected target and the remote absolute path is on your clipboard. Paste wherever you need it (Claude Code, Codex, editor, terminal, whatever).

### Keys

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

Works across iTerm2, Terminal.app, Ghostty, kitty, WezTerm, Alacritty, GNOME Terminal, Windows Terminal. Handles spaces, Chinese, Japanese, emoji filenames.

## Features

- **One gesture** – drag or paste, that's it
- **rsync backend** with live progress (`--info=progress2`), resumable via `--partial`
- **Multi-host fan-out** via named groups
- **Auto / manual modes** – auto fires on paste, manual queues until Enter
- **4 clipboard formats** – `remote_path`, `scp_style`, `ssh_path`, or a custom template with `{user} {host} {port} {path} {basename}` placeholders
- **Robust path parser** – handles quotes, escaped spaces, `file://` URLs, UTF-8 (Chinese / Japanese / emoji)
- **In-app update check** – `lzscp --check`

## Requirements

- **Local**: `rsync` (preinstalled on macOS and most Linux)
- **Remote**: `rsync` + `sshd`; SSH key auth highly recommended (no password prompts)

## Troubleshooting

**Pasted paths arrive character-by-character**
Your terminal doesn't have bracketed paste on. tmux 3.0+ passes it through by default. On iTerm2, enable *Applications in terminal may access clipboard*.

**`ssh unreachable` on a target**
lzscp uses `BatchMode=no` for the actual rsync (so it can prompt) but the startup preflight assumes a key is in ssh-agent. Run `ssh-add ~/.ssh/your_key` first.

**Remote `~` not expanded**
First sync to a host runs `ssh host echo $HOME` to resolve the home dir. If that fails for some reason, just set `remote_dir` to an absolute path.

## License

MIT — Do what you want. Claude wrote most of it anyway.
