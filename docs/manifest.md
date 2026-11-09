# Manifest schema

`dotfilesync.toml` is the desired state for a machine. Schema version **1** includes profiles, hostname selection, and setup keys. Profiles are optional: a manifest with only the base layer is valid. Any other `schema_version` is rejected.

The repository manifest is schema 1. `personal` keeps that base layer (including game casks). `work` adds `awscli` and `slack` and removes `melonds`, `minecraft`, `roblox`, and `steam`. `[select] default = "personal"`.

## Top-level fields

| Field | Required | Meaning |
|---|---|---|
| `schema_version` | yes | `1`. |
| `metadata` | no | `name`, `description`. |
| `package_managers.homebrew` | no | Base `formulae` and `casks`. |
| `files` | no | Base tracked files (`source`, `target`). |
| `exclusions` | no | `paths` and glob `patterns` left alone by scan, status, and apply. |
| `overrides` | no | Machine-local targets. Never copied or linked over. |
| `setup` | no | `login_shell` (optional string) and `fisher` (bool, default false). |
| `select` | no | `default` profile name, and `hostnames` map. |
| `profiles` | no | Named overlays. Keys are profile names. |

### Tracked files

```toml
[[files]]
source = "fish/config.fish"          # relative to the repo root
target = "~/.config/fish/config.fish" # ~ expands against the plan's home
mode = "copy"                         # copy (default) or symlink
os = "macos"                          # optional; omitted means every OS
machine_local = false
description = "optional"
```

`mode` serializes in lowercase. `copy` writes file bytes. `symlink` points `target` at `source`.

`os` is kept only when it matches the host. The only implemented host OS is `macos`. Any other value parses and is then dropped at resolve time.

File identity during a merge is the **raw manifest `target` string**, not the expanded path. `~/.config/fish/config.fish` and `/Users/me/.config/fish/config.fish` are different keys.

`fish/conf.d` in this repo is `fnm.fish` (tracked) and `local.fish` (override and exclusion only). No other `conf.d` file is declared.

### Setup

```toml
[setup]
login_shell = "fish"   # absent means "do not change the login shell"
fisher = true          # absent or false means no Fisher actions
```

`login_shell` is a Homebrew binary name. The planner resolves it as `<brew --prefix>/bin/<name>`. It does not use `PATH`.

`fisher = true` emits `InstallFisher` and `FisherUpdate` together, and only when `~/.config/fish/functions/fisher.fish` is missing. If Fisher is already installed, neither action is emitted.

### Profiles

```toml
[profiles.work]
os = "macos"                 # optional; same filter as file os
extends = ["shared"]         # optional; those profiles apply first
exclude_files = ["~/.config/zed/settings.json"]

[profiles.work.package_managers.homebrew]
formulae = ["awscli"]
exclude_formulae = ["minecraft"]
casks = ["slack"]
exclude_casks = ["steam"]

[[profiles.work.files]]
source = "fish/conf.d/work.fish"
target = "~/.config/fish/conf.d/work.fish"
mode = "copy"

[profiles.work.setup]
login_shell = "fish"         # omit the key to keep the earlier layer
fisher = false

[profiles.work.exclusions]
paths = []
patterns = []

[[profiles.work.overrides]]
target = "~/.config/fish/conf.d/local.fish"
```

An empty profile (`[profiles.personal]`) is valid. It still records the profile name and inherits the base layer unchanged. That is how `select.default = "personal"` works in the repo manifest.

## Selection

`resolve_profile` takes `Auto`, `Explicit(name)`, or `BaseOnly`, plus a hostname and a target OS.

| Selection | Result |
|---|---|
| `BaseOnly` | Base layer. Profiles and `select` are ignored. `profile` is `None`. |
| `Explicit("work")` | That profile must exist. Unknown name is an error. |
| `Auto` | `select.hostnames[hostname]`, else `select.default`, else base only. |

Hostname lookup strips a trailing `.local` and compares case-insensitively, same as `current_hostname()` (`libc::gethostname`). `Work-Mac.local` matches `work-mac`.

A name in `select.hostnames` or `select.default` that is not a profile is a validation error. A cycle in `extends` is a validation error. Unknown `extends` targets are a validation error.

On this repo's manifest, an unmatched hostname resolves to `personal`. `work-mac` resolves to `work`.

## Merge order

Layers apply in this order:

1. The base manifest.
2. Each `extends` profile, depth-first, in the order listed. A profile is emitted only after the profiles it extends.
3. The selected profile.

A profile (and the files it adds) whose `os` does not match the host is skipped. The selected profile name is still recorded.

Within one layer:

- Package names are **added**, then `exclude_formulae` / `exclude_casks` **remove**. An exclude beats an add from an earlier layer. An add in a later layer puts the name back.
- Files are keyed by the raw `target`. A later layer **replaces** that entry wholesale. `exclude_files` deletes by that same target string.
- `setup.login_shell` and `setup.fisher` use last-layer-wins, and only when that key is present on the overlay. An omitted key leaves the earlier value.
- Exclusions and overrides are **unioned**. A later override with the same target replaces the earlier one.
- Output lists are sorted and deduplicated.

### Worked example

```toml
schema_version = 1

[package_managers.homebrew]
formulae = ["git", "minecraft"]
casks = ["steam", "zed"]

[[files]]
source = "fish/config.fish"
target = "~/.config/fish/config.fish"
mode = "copy"

[[files]]
source = "zed/settings.json"
target = "~/.config/zed/settings.json"

[setup]
login_shell = "bash"
fisher = false

[profiles.shared.package_managers.homebrew]
formulae = ["fish"]
exclude_formulae = ["minecraft"]

[[profiles.shared.files]]
source = "fish/config.work.fish"
target = "~/.config/fish/config.fish"
mode = "symlink"

[profiles.work]
extends = ["shared"]
exclude_files = ["~/.config/zed/settings.json"]

[profiles.work.package_managers.homebrew]
formulae = ["awscli", "minecraft"]   # puts minecraft back after shared removed it
casks = ["slack"]
exclude_casks = ["steam"]

[profiles.work.setup]
login_shell = "fish"
fisher = true
```

Resolving `Explicit("work")` on macOS walks **base → shared → work**.

| After layer | Formulae | Casks | Files (by raw target) | Setup |
|---|---|---|---|---|
| base | `git`, `minecraft` | `steam`, `zed` | `~/.config/fish/config.fish` copy of `fish/config.fish`; `~/.config/zed/settings.json` copy | `bash`, fisher off |
| shared | `fish`, `git` (`minecraft` removed) | `steam`, `zed` | fish target **replaced** by a symlink to `fish/config.work.fish`; Zed unchanged | unchanged (`shared` sets no setup keys) |
| work | `awscli`, `fish`, `git`, `minecraft` | `slack`, `zed` (`steam` removed) | Zed target removed by `exclude_files`; fish symlink kept | `fish`, fisher on |

`BaseOnly` on the same file stops after the base row. `Explicit("shared")` stops after the shared row and does not re-add `minecraft`.

The fish-file replace matches because both entries use the target string `~/.config/fish/config.fish`. A later entry that used the expanded home path would be a second file, not a replacement.
