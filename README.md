# lzsync

> Because `scp` is 3 characters but still too much trouble.

## What is this?

A local TUI that makes pushing a file from your laptop to an SSH server brain-dead simple:

**drop file into TUI → remote absolute path lands on your clipboard → paste into Claude Code on the remote box → done.**

Or point it at a folder (`~/Desktop`, `~/Screenshots`, whatever) and it will auto-upload every new file that lands there. Screenshot → 2 seconds later the remote path is already on your clipboard.

> **Previously known as `lzscp`.** v0.4.0 renamed the project to `lzsync` because it's no longer just one-shot SCP — it also watches folders. Your old `~/.config/lzscp/` config + history are auto-migrated to `~/.config/lzsync/` on first launch.

## Origin Story

I SSH into a server all day and run Claude Code / Codex inside tmux. Every time I wanted to show the agent a local screenshot, the flow was:

1. Open Cursor Remote SSH (wait forever)
2. Drag file into a folder
3. Copy the path
4. Paste into Claude Code
5. Curse

So I made `lzsync`. Drag into TUI, get path, paste. One step. Or toggle folder-watch on `~/Desktop` and take the gesture out entirely — the screenshot shows up ready to paste.

Like lzgit, I don't write Rust. Claude Code wrote all of it. I mostly said "no, spaces and Chinese filenames *also* need to work."

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/FanFusion/lzsync/main/install.sh | bash
```

Auto-detects Linux / macOS, x86_64 / arm64, downloads the matching binary to `~/.local/bin/lzsync`. Works on everything the GH Actions matrix builds.

Overrides:

```bash
# Install to somewhere else
INSTALL_DIR=/usr/local/bin curl -fsSL https://raw.githubusercontent.com/FanFusion/lzsync/main/install.sh | bash

# Pin to a specific version
VERSION=v0.4.0 curl -fsSL https://raw.githubusercontent.com/FanFusion/lzsync/main/install.sh | bash
```

### Manual download

Grab a prebuilt binary from [Releases](https://github.com/FanFusion/lzsync/releases/latest):

```bash
# macOS (Apple Silicon)
curl -fsSL https://github.com/FanFusion/lzsync/releases/latest/download/lzsync-macos-aarch64 -o ~/.local/bin/lzsync && chmod +x ~/.local/bin/lzsync

# macOS (Intel)
curl -fsSL https://github.com/FanFusion/lzsync/releases/latest/download/lzsync-macos-x86_64 -o ~/.local/bin/lzsync && chmod +x ~/.local/bin/lzsync

# Linux (x86_64)
curl -fsSL https://github.com/FanFusion/lzsync/releases/latest/download/lzsync-linux-x86_64 -o ~/.local/bin/lzsync && chmod +x ~/.local/bin/lzsync

# Linux (aarch64)
curl -fsSL https://github.com/FanFusion/lzsync/releases/latest/download/lzsync-linux-aarch64 -o ~/.local/bin/lzsync && chmod +x ~/.local/bin/lzsync
```

### From source

```bash
cargo install --path .
# or
cargo build --release && install -m 755 target/release/lzsync ~/.local/bin/lzsync
```

### AI-era install

```
claude "install lzsync from https://raw.githubusercontent.com/FanFusion/lzsync/main/README.md"
```

## Configure

Drop a `config.toml` at either:

- `$PWD/.lzsync/config.toml` — per-project (takes priority)
- `~/.config/lzsync/config.toml` — global fallback

Minimum:

```toml
default_target = "dev"

[[target]]
name       = "dev"
host       = "dev.example.com"
user       = "ubuntu"
remote_dir = "~/uploads"
```

Optional folder-watch (v0.4.0+):

```toml
[[watch]]
name     = "screenshots"
path     = "~/Desktop"
targets  = ["dev"]
patterns = ["*.png", "*.jpg", "*.heic", "*.webp"]
catchup  = "prompt"     # prompt | auto | ignore — what to do with files added while lzsync was off
enabled  = true         # try to acquire the folder watch lock on launch
```

See [`examples/config.toml`](examples/config.toml) for every option (multi-host fan-out, clipboard format, custom templates, SSH port / key, watch catchup semantics, etc.).

## Use

```bash
lzsync
```

Two tabs:

- **Drop** — drag a file in from your OS file manager, or `Cmd+V` / `Ctrl+Shift+V` a path. rsyncs to the selected target; remote absolute path ends up on your clipboard.
- **Watch** — one row per configured folder. `Space` toggles watching on/off, `r` catches up on files that appeared while lzsync was off, `a`/`d`/`e` add / delete / edit a folder.

Paste the remote path wherever you need it (Claude Code, Codex, editor, terminal, whatever).

### Keys

| Key              | Action                                    |
| ---------------- | ----------------------------------------- |
| `1` / `2`        | Switch tab (Drop / Watch)                 |
| `Tab` / `S-Tab`  | Cycle focus between panels inside the tab |
| `Up` / `Down`    | Move cursor                               |
| `Space`          | Toggle target / watch folder              |
| `Enter`          | Sync (manual mode) / run catchup (Watch)  |
| `Ctrl+A`         | Auto mode                                 |
| `Ctrl+N`         | Manual mode                               |
| `Ctrl+F`         | Cycle clipboard format                    |
| `Ctrl+T`         | Cycle theme                               |
| `Ctrl+P`         | Open action menu                          |
| `Ctrl+H`         | Help overlay                              |
| `Ctrl+U`         | Check & install updates                   |
| `Ctrl+Q`         | Quit                                      |

Works across iTerm2, Terminal.app, Ghostty, kitty, WezTerm, Alacritty, GNOME Terminal, Windows Terminal. Handles spaces, Chinese, Japanese, emoji filenames.

## Features

- **One gesture** – drag or paste, that's it
- **Folder watch** – point lzsync at a folder and new files auto-sync to the target. One folder, multiple folders, each with its own target mapping.
- **Multi-instance safe** – folder-level lock; two lzsyncs can watch different folders, and the second one trying to watch the same folder gets a friendly `locked by PID …` message.
- **rsync backend** with live progress (`--info=progress2`), resumable via `--partial`
- **Multi-host fan-out** via named groups
- **Auto / manual modes** – auto fires on paste/watch-event, manual queues until Enter
- **4 clipboard formats** – `remote_path`, `scp_style`, `ssh_path`, or a custom template with `{user} {host} {port} {path} {basename}` placeholders
- **Robust path parser** – handles quotes, escaped spaces, `file://` URLs, UTF-8 (Chinese / Japanese / emoji)
- **In-app update check** – `lzsync --check`

## Requirements

- **Local**: `rsync` (preinstalled on macOS and most Linux)
- **Remote**: `rsync` + `sshd`; SSH key auth highly recommended (no password prompts)

## Migrating from lzscp

If you had lzscp ≤0.3.6 installed:

- Your `~/.config/lzscp/config.toml` and `~/.config/lzscp/history/*.jsonl` are **automatically copied** to `~/.config/lzsync/` the first time v0.4.0 runs.
- The old directory is kept in place as a backup. Remove it yourself (`rm -rf ~/.config/lzscp`) once you've confirmed everything works.
- The old `lzscp` binary stays installed side-by-side until you delete it. It will keep reading from the legacy dir.
- GitHub automatically redirects `FanFusion/lzscp` → `FanFusion/lzsync`, so any old URLs keep working.

## Troubleshooting

**Pasted paths arrive character-by-character**
Your terminal doesn't have bracketed paste on. tmux 3.0+ passes it through by default. On iTerm2, enable *Applications in terminal may access clipboard*. lzsync also has a fallback paste-burst detector that kicks in after 120ms of idle typing in the drop zone.

**`ssh unreachable` on a target**
lzsync uses `BatchMode=no` for the actual rsync (so it can prompt) but the startup preflight assumes a key is in ssh-agent. Run `ssh-add ~/.ssh/your_key` first.

**Remote `~` not expanded**
First sync to a host runs `ssh host echo $HOME` to resolve the home dir. If that fails for some reason, just set `remote_dir` to an absolute path.

**Watch folder says `locked by PID 12345`**
Another lzsync instance is already watching that folder. Stop the other instance, or watch a different folder in this one. The lock is per-folder, not per-instance.

## License

MIT — Do what you want. Claude wrote most of it anyway.
