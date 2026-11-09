# my-shell

Brewfile-driven macOS coding environment bootstrap: Fish shell, Tide prompt, fnm (Node), and the CLI/GUI tools you declare in a Brewfile.

**Repo:** [https://github.com/JamalLyons/my-shell](https://github.com/JamalLyons/my-shell)

## Quick start

```bash
bash ./setup.sh
```

Useful flags:

```bash
bash ./setup.sh --dry-run       # preview only
bash ./setup.sh --skip-casks    # formulas only (no GUI apps)
bash ./setup.sh --skip-shell    # keep your current login shell
bash ./setup.sh --skip-xcode    # skip latest Xcode download/install
bash ./setup.sh --skip-fonts    # skip 0xProto font cask
bash ./setup.sh --skip-zed      # skip Zed settings sync
bash ./setup.sh --skip-models   # skip LM Studio model downloads
```

## What it does

1. Installs Homebrew if missing
2. Applies [`Brewfile`](Brewfile) (CLI formulas + casks)
3. Installs **0xProto** via Homebrew `font-0xproto` (prefer [2.502](https://github.com/0xType/0xProto/releases/tag/2.502); still runs after `--skip-casks` unless `--skip-fonts`)
4. Syncs checked-in [`fish/`](fish/) templates into `~/.config/fish` (backs up changed files)
5. Installs Fisher + plugins (Tide, done, bass)
6. Syncs checked-in [`zed/settings.json`](zed/settings.json) into `~/.config/zed/settings.json` (prompts before overwrite if a different file already exists)
7. Installs **Node.js LTS** via [fnm](https://github.com/Schniz/fnm) and enables **pnpm** via Corepack
8. Installs the **latest Xcode** via [xcodes](https://github.com/XcodesOrg/xcodes) (Apple ID may be required; large download)
9. Downloads preferred local models via **LM Studio CLI** (`lms get`)
10. Optionally sets Fish as your login shell

## Layout

```
my-shell/
├── setup.sh              # Thin orchestrator
├── Brewfile              # Declarative packages + casks
├── lib/
│   ├── log.sh
│   ├── brew.sh
│   ├── fonts.sh
│   ├── fish.sh
│   ├── node.sh
│   ├── xcode.sh
│   ├── zed.sh
│   └── models.sh
├── fish/
│   ├── config.fish
│   ├── fish_plugins
│   ├── conf.d/fnm.fish
│   └── functions/
├── zed/
│   └── settings.json     # Global Zed editor settings
└── README.md
```

## Adding software

- **CLI / Homebrew apps:** edit `Brewfile`, then re-run `bash ./setup.sh` (or `brew bundle --file=Brewfile`).
- **Fish plugins:** edit `fish/fish_plugins`, then re-run setup (or `fisher update` in Fish).
- **Fish config / functions:** edit files under `fish/`, then re-run setup to sync.
- **Zed settings:** edit [`zed/settings.json`](zed/settings.json), then re-run setup to sync to `~/.config/zed/settings.json`.

### Local environment variables

Shared config must not contain secrets. Setup creates (once):

```text
~/.config/fish/conf.d/local.fish
```

from the documented empty template [`fish/conf.d/local.fish`](fish/conf.d/local.fish). Later `setup.sh` runs **never overwrite** that file. Put project IDs, tokens, and machine-only paths there (`editenv`), then `reload`.

### GUI apps (casks)

Included today (matched to `/Applications` where a cask exists):

- Ghostty, Zed, Raycast, LLM-Studio, Rectangle, JetBrains Toolbox
- Discord, Lark, Docker Desktop, Brave, Chrome, Spotify, OBS
- GitHub Desktop, Cursor, Claude, DBeaver, Inkscape, MEGAsync
- Steam, Audacity, Cloudflare WARP, eqMac, pgAdmin 4
- Antigravity, Google Gemini, Juicy, Epic Games, Minecraft, Roblox
- Windows App, melonDS

**No Homebrew cask** (install/update manually or via Mac App Store):

- Blackmagic Proxy Generator Lite, Blackmagic RAW, DaVinci Resolve
- VMware Fusion, LockDown Browser, Magic Garden, Developer (WWDC)
- Keynote, Numbers, Pages, Safari, Xcode
- Python 3.13.app (python.org; CLI Python is covered by Brewfile `python`)

Full **Xcode** is installed by `setup.sh` via `xcodes` (not a Homebrew cask). Use `--skip-xcode` to skip.

### Fonts

**0xProto** is installed with Homebrew cask `font-0xproto` (user fonts under `~/Library/Fonts`). Preferred release: **2.502** (see `OXPROTO_PREFERRED_VERSION` in [`lib/fonts.sh`](lib/fonts.sh)). Setup still installs it when you pass `--skip-casks`; use `--skip-fonts` to skip. Ghostty, Zed, and other apps need the font selected in their settings after install.

### Zed settings

Global Zed config lives in [`zed/settings.json`](zed/settings.json) and installs to `~/.config/zed/settings.json`.

- Missing destination → copies the template quietly
- Identical to template → skips
- Different existing file → warns and asks `Overwrite? [y/N]` (default **N**); only overwrites on `y`/`Y`
- Use `--skip-zed` to skip; `DRY_RUN=1` / `--dry-run` prints intent without writing or prompting to overwrite

### Updating packages

In Fish, run `brewup` to refresh Homebrew formulas/casks (and Fisher plugins) — similar to `apt update && apt upgrade` on Linux.

## Node via fnm

This setup uses **fnm**, not nvm / nvm.fish.

- Default: latest **LTS** (`fnm install --lts`)
- Per-project: `.node-version` or `.nvmrc` with `fnm env --use-on-cd`

```fish
fnm list
fnm install lts
fnm use 22
```

If you previously used nvm, `~/.nvm` is left alone. After confirming fnm works you can remove it and `brew uninstall nvm` if present.

## Prompt

Uses [Tide](https://github.com/IlanCosman/tide) (`ilancosman/tide@v6`). Configure with `tide configure`.

## Custom Fish commands

In Fish, run `helpme` for the full list. Highlights:

| Command | Description |
|---------|-------------|
| `helpme` | List custom commands (links to this repo) |
| `brewup` | Update Homebrew formulas, casks, and Fisher |
| `reload` | Reload Fish config |
| `editfish` | Open config in `$EDITOR` |
| `editenv` | Open `~/.config/fish/conf.d/local.fish` in `$EDITOR` |
| `mkcd <dir>` | mkdir + cd |
| `extract <file>` | Unpack common archives |
| `ff` / `fdir` | Find files / directories |
| `ql <file>` | Quick Look |
| `cdf` | cd to frontmost Finder window |
| `flushdns` | Flush macOS DNS cache |
| `path` | Print `$PATH` one entry per line |
| `serve [port]` | Python HTTP server |
| `note [text]` | Quick notes |

Git abbreviations: `gst`, `gco`, `gaa`, `gcm`, `gps`, `gpl`, …

## Requirements

- macOS
- Network access (Homebrew, Fisher, fnm)
- Git (usually via Xcode CLT / Homebrew)

## License

Apache-2.0

---

## dotfilesync (Rust Environment Manager)

`dotfilesync` is the macOS-first developer environment manager written in Rust, laying the foundation for a future GPUI-based environment manager. In this first milestone, `dotfilesync` provides read-only inspection commands (`scan`, `status`) and a safe Brewfile importer (`import-brewfile`) that creates new manifest files without modifying existing system configurations or tracking repositories.

### Architecture

- **`dotfilesync-core`**: Reusable domain models, manifest schema validation, command runner abstraction with timeout protection, static Brewfile parser, filesystem inspection, and diff comparison engine. Completely UI-framework agnostic.
- **`dotfilesync-protocol`**: Typed, versioned local IPC protocol and blocking client shared by the CLI and the future GPUI app.
- **`dotfilesync-daemon`**: The per-user daemon `dotfilesyncd`. It hosts the core behind an owner-only Unix socket (no TCP) and manages jobs, locks, a job journal, rotated logs, and the launchd LaunchAgent.
- **`dotfilesync-cli`**: Command-line binary (`dotfilesync`) providing human-friendly and `--json` structured output. It is a client of the daemon, with a standalone fallback.

See [docs/daemon-architecture.md](docs/daemon-architecture.md) for the process, protocol, security and state model.

### Toolchain and Building

Requires **macOS** and **Rust 1.85+** (MSRV 1.85). The `dotfilesync` and `dotfilesyncd` binaries are built exclusively for macOS; building them on Linux or Windows is prevented at compile time. The `dotfilesync-core` and `dotfilesync-protocol` libraries compile on any Unix, as groundwork for the headless Linux target (#12).

```bash
# Build the workspace
cargo build --workspace

# Run tests (fully fixture-isolated)
cargo test --workspace

# Check formatting and clippy
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

### CLI Commands

Commands `scan` and `status` are strictly **read-only** inspection commands. They never install software, uninstall packages, modify shell or environment files, or mutate system configurations. Meanwhile, `import-brewfile --output <path>` explicitly creates a **new** file and strictly refuses to replace or overwrite an existing file.

#### 1. Scan (`dotfilesync scan`)
Strictly read-only: discovers installed Homebrew formulae and casks, their observed versions, and inspects only explicitly tracked configuration paths without altering system state.

```bash
# Human-readable output using default dotfilesync.toml if present
cargo run -p dotfilesync-cli -- scan

# Structured JSON output
cargo run -p dotfilesync-cli -- scan --manifest dotfilesync.toml --json
```

#### 2. Status (`dotfilesync status --manifest <path>`)
Strictly read-only: compares the desired state defined in a versioned manifest against the observed system state without making any modifications.

```bash
cargo run -p dotfilesync-cli -- status --manifest dotfilesync.toml

# JSON output
cargo run -p dotfilesync-cli -- status --manifest dotfilesync.toml --json
```

Differences are classified into:
- **`matching`**: Reserved strictly for verified equivalence between repository source and machine target under the comparison policy.
- **`missing`**: Declared in the manifest but absent from the machine. (Absent machine-local overrides are clearly distinguished in human-readable output from ordinary missing managed configs that require restoration from repository templates).
- **`changed`**: Config file drift detected (content/hash differs between repository template and machine).
- **`unmanaged`**: Present on the machine but not declared in the manifest (unmanaged does *not* mean unwanted).
- **`unknown`**: Diagnostic recorded if Homebrew is unavailable or package enumeration failed (ensures an unavailable tool never falsely claims all packages are missing), or if a config file is unreadable.
- **`skipped`**: Explicitly excluded or machine-local override files. These are preserved and never counted as matching content.

#### 3. Static Brewfile Import (`dotfilesync import-brewfile`)
Statically parses a Brewfile without executing Ruby code (prohibits arbitrary code execution). Straightforward `brew` and `cask` declarations are captured, while options and dynamic Ruby constructs (`if`, `unless`, `def`, `system`, `ENV`, string interpolation, `mas`, `tap`, trailing expressions) are explicitly reported with line numbers.

When `--output <path>` is provided, `dotfilesync` explicitly creates a **new** manifest file using atomic filesystem semantics (`create_new`). It strictly **refuses to replace or overwrite** an existing manifest file, preserving existing file bytes, mappings, and exclusions untouched.

```bash
# Parse existing Brewfile and generate a new manifest skeleton
cargo run -p dotfilesync-cli -- import-brewfile --brewfile Brewfile --output dotfilesync.new.toml
```

### Daemon (`dotfilesyncd`) and service management

`scan` and `status` take `--mode auto|daemon|standalone` (default `auto`):

| Mode | Behavior |
|---|---|
| `auto` | Runs in `dotfilesyncd` when it is running. Otherwise runs in-process and prints a one-line note on stderr. |
| `daemon` | Requires the daemon. Add `--detach` to print the job ID and return immediately. |
| `standalone` | Never contacts the daemon. |

The report on stdout is the same in every mode. Neither command ever installs, loads or starts the service.

Daemon jobs keep running when the CLI exits (Ctrl-C only detaches). Reconnect from any terminal:

```bash
dotfilesync jobs list
dotfilesync jobs watch <job-id>     # progress since the start, then waits
dotfilesync jobs result <job-id> --json
dotfilesync jobs cancel <job-id>    # also stops the job's brew subprocess
```

First-time setup on a Mac:

```bash
cargo build --release --workspace
# 1. Write ~/Library/LaunchAgents/io.github.jamallyons.dotfilesyncd.plist (does NOT start anything).
#    Add --start-at-login to opt in to automatic start at login.
./target/release/dotfilesync service install
# 2. Load and start it now.
./target/release/dotfilesync service start
# 3. Inspect: installed? loaded in launchd? reachable?
./target/release/dotfilesync service status
# Stop for this session (stays installed), or remove the definition.
./target/release/dotfilesync service stop
./target/release/dotfilesync service uninstall   # keeps state/logs, configs and repos
```

You can also run the daemon in the foreground with `./target/release/dotfilesyncd`.

The daemon keeps its state in `~/Library/Application Support/dotfilesync`, which must be mode 0700. You can override the location with `--data-dir` or `DOTFILESYNC_DATA_DIR`. It contains:

| Path | Contents |
|---|---|
| `run/` | Socket and lock file |
| `state/jobs.jsonl` | Job metadata only |
| `logs/` | Rotated, redacted logs |

A manual QA checklist is in [docs/daemon-manual-qa.md](docs/daemon-manual-qa.md).

### Manifest Schema (`dotfilesync.toml`)

The repository includes a versioned `dotfilesync.toml` manifest modeling the current `Brewfile`, `fish/` configuration, and `zed/settings.json`.

Key capabilities:
- **Explicit Tracking**: Separate repository `source` and machine `target` paths (e.g. `~/.config/fish/config.fish`).
- **Exclusions**: Explicit paths and glob patterns (`*.backup.*`, `*.key`, `*secret*`, `*credential*`) are excluded from drift checks.
- **Machine-Local Overrides**: Files like `~/.config/fish/conf.d/local.fish` are recognized as machine-local and preserved without drift warnings or overwrite risks.
- **Credential Protection**: Strict validation prevents tracking sensitive paths like `~/.ssh/id_*`, `~/.aws/credentials`, or `~/.gnupg`.

### Existing Behaviors Preserved

- **Bash Installer (`setup.sh`)**: The existing Bash bootstrapper, `Brewfile`, and helper scripts in `lib/` remain 100% untouched and fully operational.
- **Safety First**: Inspection commands `scan` and `status` are strictly read-only with zero system mutations. `import-brewfile` only writes when `--output` is explicitly supplied, and then strictly creates a new file refusing to overwrite existing files. There are no package installs/removals, deletions, shell modifications, or `sudo` operations. File contents are never printed in scan or status output.
- **Test Isolation**: All discovery and runner mechanisms are abstracted behind traits with unit fixtures and mock outputs, preventing tests from modifying or relying on actual machine state.

### Current Limitations

- **Read-Only Inspection / Safe Creation**: `scan` and `status` are strictly read-only and never install packages, copy files to system locations, or mutate existing system configurations. `import-brewfile` creates a new manifest skeleton file only when `--output` is explicitly passed and refuses to overwrite existing files.
- **No GUI**: GPUI interface integration is slated for future milestones.
- **Daemon scope**: The daemon serves `scan` and `status` jobs only. Plan/apply, toolchains, config editing, sync and the watcher are reserved in the protocol and answered with an explicit `unsupported_operation` until their issues land. It has no crash auto-restart (no launchd `KeepAlive`), and it supports no Linux service yet.
- **No Remote Git Sync**: Future state sync will use a dedicated user-owned private state repository rather than personal branches in this repo.
- **Language Managers**: Integrations for `fnm`, `nvm`, and Java manager diagnostics will be introduced in subsequent milestones.

