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
- **`dotfilesync-cli`**: Command-line binary (`dotfilesync`) providing human-friendly and `--json` structured output.

### Toolchain and Building

Requires **macOS** and **Rust 1.85+** (MSRV 1.85). Dotfilesync Milestone 1 is built exclusively for macOS; building on Linux or Windows is unsupported and prevented at compile time.

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
- **No Remote Git Sync**: Future state sync will use a dedicated user-owned private state repository rather than personal branches in this repo.
- **Language Managers**: Integrations for `fnm`, `nvm`, and Java manager diagnostics will be introduced in subsequent milestones.

