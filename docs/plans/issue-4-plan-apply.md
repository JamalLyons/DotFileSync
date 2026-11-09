# Implementation plan — Issue #4: manifest profiles + plan/apply via the daemon

Branch: `feature/plan-apply-profiles`. Issue: https://github.com/JamalLyons/dotfiles/issues/4

Read before starting: `docs/daemon-architecture.md`, `crates/dotfilesync-core/src/ops.rs`,
`crates/dotfilesync-core/src/manifest/`, `crates/dotfilesync-daemon/src/{jobs,approval,journal,locks,server}.rs`,
`crates/dotfilesync-protocol/src/messages.rs`, `crates/dotfilesync-cli/src/commands/status.rs`, `setup.sh`, `lib/*.sh`.

## Ground rules (all workstreams)

- Rust 1.85, edition 2021. No new heavy deps. Allowed new deps: none beyond what the workspace
  already uses (serde, serde_json, toml, sha2, hex, thiserror, glob, libc, tempfile for tests).
  Hostname: use `libc::gethostname`. Timestamps: use existing `now_ms` style helpers / `std::time`.
- Never execute caller-supplied commands. Every action is a typed enum variant; the executor maps
  variants to fixed argv through a `CommandRunner` (so tests use `MockCommandRunner`).
- Deterministic everything: sort inputs, use `BTreeMap`/`BTreeSet`, no timestamps or random ids
  inside the plan body (the plan id is derived from the digest).
- `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace` must pass at the end of each workstream.
- Do not delete `setup.sh` or `lib/`. Do not commit; the orchestrator commits.

---

## Workstream A — core (crate `dotfilesync-core`) — runs FIRST

### A1. Manifest v2 with profiles (`manifest/model.rs`, `manifest/profile.rs` new, `validation.rs`)

- `CURRENT_SCHEMA_VERSION = 2`. `parse_manifest` accepts 1 and 2 (v1 = base only, no profiles).
- New top-level fields (all `#[serde(default)]`):
  ```toml
  [select]                      # optional: hostname → profile mapping
  hostnames = { "jamals-mbp" = "personal", "work-mac" = "work" }
  default = "personal"          # optional fallback

  [profiles.work]
  os = "macos"                  # optional condition; v1 only implements "macos"
  extends = []                  # optional, other profile names applied before this one
  [profiles.work.package_managers.homebrew]
  formulae = ["awscli"]         # added
  exclude_formulae = ["minecraft"]
  casks = ["slack"]
  exclude_casks = ["steam", "roblox"]
  [[profiles.work.files]]       # added / replaces same-target base entry
  source = "fish/conf.d/work.fish"
  target = "~/.config/fish/conf.d/work.fish"
  mode = "copy"
  exclude_files = ["~/.config/zed/settings.json"]   # by target
  ```
- `TrackedFile` gains `mode: FileMode` (`copy` default | `symlink`), serialized lowercase.
- Base layer may also carry `os` conditions on files (`os = "macos"` optional field on TrackedFile).
- New `[[actions]]` section for setup.sh parity, typed (see A3): e.g.
  ```toml
  [setup]
  login_shell = "fish"          # optional; resolves via brew prefix
  fisher = true                 # install Fisher + `fisher update` from fish_plugins
  ```
  (profiles can override `setup.*` keys).
- Public API:
  ```rust
  pub enum ProfileSelection { Auto, Explicit(String), BaseOnly }
  pub struct ResolvedManifest { pub profile: Option<String>, pub os: TargetOs,
      pub homebrew: HomebrewSection, pub files: Vec<TrackedFile>, pub setup: SetupSection,
      pub exclusions: ExclusionsSection, pub overrides: Vec<MachineOverride> }
  pub fn resolve_profile(m: &Manifest, sel: &ProfileSelection, hostname: &str, os: TargetOs)
      -> Result<ResolvedManifest, ManifestError>
  pub fn current_hostname() -> String   // libc::gethostname, strip ".local", lowercase
  ```
- **Merge order (document it in `docs/manifest.md`):** base → each `extends` profile depth-first
  in listed order (cycle = validation error) → selected profile. For each layer: package adds are
  unioned, then `exclude_*` removes (exclude wins over an add from an *earlier* layer; an add in a
  *later* layer re-adds). Files are keyed by expanded `target`; a later layer replaces the earlier
  entry wholesale; `exclude_files` removes by target. Scalar `setup.*` keys: last layer wins.
  Exclusions/overrides: union. Output lists are sorted and deduplicated. Entries whose `os` does
  not match are dropped.
- Selection: Explicit name → must exist (error otherwise). Auto → `select.hostnames[hostname]`,
  else `select.default`, else base only. Unknown profile names in `select` = validation error.
- `run_status`/`run_scan`/`compare_status` must keep working: they use the resolved manifest.
  Add `profile: Option<String>` to `StatusInput`/`ScanInput` with `#[serde(default)]` (None = Auto).
- Unit tests (`tests/profile_tests.rs`): base only; overlay adds; exclude removes; later re-add;
  conflicting file target (later wins); extends chain + cycle error; hostname auto-select; default
  fallback; unknown explicit profile; os mismatch filtered; v1 manifest still parses.

### A2. Plan model + planner (`plan/mod.rs`, `plan/model.rs`, `plan/planner.rs`)

```rust
pub enum Risk { Low, Medium, High }            // serde snake_case
pub enum ActionKind {                          // serde tag = "kind", snake_case
    BrewInstallFormula { name: String },
    BrewInstallCask { name: String },
    BrewUpgradeFormula { name: String },       // only when `brew outdated` lists it and --upgrade
    BrewUpgradeCask { name: String },
    LinkFile { source: PathBuf, target: PathBuf },   // symlink mode
    CopyFile { source: PathBuf, target: PathBuf },   // copy mode
    EnsureLocalOverride { target: PathBuf, template: Option<PathBuf> }, // create-if-missing only
    InstallFisher,
    FisherUpdate,
    SetLoginShell { shell: PathBuf },          // needs sudo for /etc/shells → High risk, prompts
}
pub struct PlanAction { pub id: String /* "a-001" stable index */, pub kind: ActionKind,
    pub summary: String, pub risk: Risk, pub reversible: bool,
    pub backup: bool /* target exists and differs → will be backed up */ }
pub struct Plan { pub schema: u32, pub profile: Option<String>, pub manifest_path: PathBuf,
    pub home: PathBuf, pub repo_root: PathBuf, pub actions: Vec<PlanAction>,
    pub state_fingerprint: String /* sha256 of observed state used */, }
impl Plan { pub fn digest(&self) -> String; pub fn plan_id(&self) -> String /* "p-" + first 16 hex of digest */;
            pub fn is_empty(&self) -> bool }
pub struct PlanInput { pub manifest: PathBuf, pub home: PathBuf, pub repo_root: Option<PathBuf>,
    pub profile: Option<String>, pub upgrade: bool }
pub fn run_plan(input: &PlanInput, runner: &dyn CommandRunner, hostname: &str,
    progress: &mut dyn FnMut(OpStage), is_cancelled: &dyn Fn() -> bool) -> Result<Plan, OpError>
```
- Planner reuses `discover_homebrew` + `ConfigInspector` (don't re-implement). Emits only what
  differs → **converged system = empty plan**. Ordering is fixed: formulae, casks, upgrades,
  files (sorted by target), local overrides, fisher, login shell.
- File rules: target equal content (copy) or correct symlink (symlink) → no action. Paths in
  `overrides` / machine-local / `local.fish` → never a Copy/Link action; only `EnsureLocalOverride`
  when the file is missing.
- Risk/reversible table (put it in `docs/plan-apply.md`):
  | action | risk | reversible |
  |---|---|---|
  | brew install formula/cask | low/medium(cask) | no (we never auto-uninstall) |
  | brew upgrade | medium | no (no historical versions) |
  | copy/link file, target absent | low | yes (remove the file) |
  | copy/link file, target exists | medium | yes (restore from backup) |
  | ensure local override | low | yes |
  | install fisher / fisher update | medium | no |
  | set login shell | high | yes (`chsh` back; previous shell recorded) |
- `state_fingerprint`: sha256 over sorted installed formulae/casks + each tracked target's
  observed state (sha/symlink dest/missing) + login shell. Deterministic.
- Human renderer lives in the CLI; core provides serde JSON. Test: two `run_plan` calls on the
  same fixture produce byte-identical JSON.

### A3. Apply executor (`apply/mod.rs`, `apply/journal.rs`, `apply/backup.rs`)

```rust
pub struct ApplyContext<'a> { pub runner: &'a dyn CommandRunner, pub backup_root: PathBuf,
    pub run_dir: PathBuf /* holds journal */, pub allow_sudo: bool, pub now: &'a dyn Fn() -> u64 }
pub enum StepStatus { Pending, Running, Done, Failed, Skipped }
pub struct RunJournal { pub run_id: String, pub plan_digest: String, pub steps: Vec<StepRecord> }
pub struct StepRecord { pub action_id: String, pub status: StepStatus,
    pub backup_path: Option<PathBuf>, pub error: Option<String>, pub at_ms: u64 }
pub fn apply_plan(plan: &Plan, ctx: &ApplyContext, progress: &mut dyn FnMut(&StepRecord),
    is_cancelled: &dyn Fn() -> bool) -> Result<RunJournal, ApplyError>
pub fn resume_run(run_dir: &Path, plan: &Plan, ctx, progress, is_cancelled) -> Result<RunJournal, ApplyError>
```
- Journal: `<run_dir>/journal.jsonl` append-only + `plan.json` copy, written with fsync after
  each step transition. Resume loads it, verifies `plan_digest`, **skips `Done` steps**, retries
  `Failed`/`Running`/`Pending`. Stops at the first failure (later steps stay Pending).
- Backups: before overwriting/replacing any existing target, copy it to
  `<backup_root>/<run_id>/<path relative to home>` **and** keep the setup.sh-compatible name
  pattern in the journal (`<name>.backup.YYYYmmdd_HHMMSS`). Never delete user files: replacing a
  file with a symlink = move original into backup dir, then symlink. Create parent dirs 0755.
  Write via temp file + rename in the target dir.
- Brew: `brew install <name>` / `brew install --cask <name>` / `brew upgrade [--cask] <name>`
  through `CommandRunner` with the discovered brew path. Fisher: fixed argv `fish -c '<const script>'`
  (constant string from setup.sh, not user input). Login shell: if shell not in `/etc/shells`
  and `!allow_sudo` → step Failed with "requires sudo; rerun with --allow-sudo"; never sudo silently;
  `chsh -s <shell>` records previous shell in the journal.
- Refuse any Copy/Link whose target is an override or matches exclusions (defense in depth).
- Tests (`tests/apply_tests.rs`) with temp HOME + `MockCommandRunner`: backup created for a
  differing file; journal written; injected failure on step 3 then `resume_run` skips 1–2;
  `local.fish` untouched; empty plan after apply on converged fixture (re-plan → empty);
  symlink mode; login shell without sudo fails cleanly.

Export everything from `lib.rs`. Add `ops::run_plan` alongside `run_status`.

---

## Workstream B — protocol + daemon + CLI — runs AFTER A (parallel with C)

### B1. Protocol (`dotfilesync-protocol/src/messages.rs`)
- Remove `"plan."` from `RESERVED_OPS`; add ops `plan.create`, `plan.approve`, `plan.apply`,
  `plan.resume`, `plan.runs`, `plan.run_status` to `IMPLEMENTED_OPS` and `Request`:
  - `PlanCreate { input: PlanInput }` → starts a job of `JobKind::Plan`; result `JobOutput::Plan { plan, plan_id }`.
  - `PlanApprove { plan_id: String, digest: String }` → `Response::Approval { token, expires_at_ms }`
    (daemon looks up the plan it computed; the client echoes the digest it reviewed).
  - `PlanApply { plan_id, token, allow_sudo: bool }` → starts `JobKind::Apply`; returns `JobSnapshot`.
  - `PlanResume { run_id, token }` → new approval required (re-plan not needed; the run's plan is reused).
  - `PlanRuns {}` / `PlanRunStatus { run_id }` → `Response::Runs` / `Response::Run(RunJournal)`.
  - Use `#[serde(rename = "plan.create")]` etc. so op names match.
- New error codes: `plan_not_found`, `approval_rejected` (map `ApprovalError` variants into `details`).
- Update round-trip tests and the reserved-op test (use `runtime.x` instead of `plan.apply`).

### B2. Daemon
- Store computed plans in memory (`PlanStore`, bounded 32, keyed by plan_id).
- Replace `approval::Plan` internals so the digest bound is the core `Plan::digest()`; keep the
  token semantics (single-use, uid-bound, expiry 10 min, state fingerprint). On apply, recompute the
  fingerprint via a fresh observe step; mismatch → `approval_rejected/state_changed`.
- Apply job: takes **exclusive** locks on `homebrew` and `home:<path>`; runs `core::apply_plan` on a
  worker thread; progress event per step; journal dir `<data>/state/runs/<run_id>/` (0700),
  backups `<data>/backups/` (0700). Continues if the client disconnects (existing job model).
- On daemon restart, a run with unfinished steps shows `interrupted` and is resumable via `plan.resume`.
- `docs/daemon-architecture.md` layout + ops table updated.
- Integration test (`tests/plan_apply_tests.rs`, use `tests/common`): start daemon with temp data
  dir, temp HOME and fake brew bin (existing test pattern); create plan → approve → apply via a
  client, **drop the client**, reconnect with a new client, `job_status`/`plan.run_status` show
  completion and backups exist. Also: replayed token rejected; tampered digest rejected.

### B3. CLI
- `dotfilesync plan [--manifest] [--profile NAME] [--upgrade] [--json]` — prints human or JSON.
- `dotfilesync apply [--manifest] [--profile] [--yes] [--allow-sudo] [--dry-run]` — default
  behaviour: create plan, print it, and if not `--yes` ask `Apply N actions? [y/N]` on a TTY
  (non-TTY without `--yes` = dry-run + exit code 0 with a note). `--dry-run` never applies.
  Then approve + apply via daemon and follow progress (reuse `jobs` follow code). Ctrl-C detaches
  without cancelling (print the job id and `dotfilesync jobs follow <id>`).
- `dotfilesync apply --resume <run_id>`; `dotfilesync runs [list|show <id>]`.
- Standalone fallback: `plan` may run in-process; `apply` **requires the daemon** (error with
  hint `dotfilesync service start`), since the journal/locks live in daemon state.
- Human plan format: one line per action: `[risk] [rev|irrev] [copy|link] summary`, then totals.
- CLI tests: `plan --json` stable across two runs; `apply` without daemon errors; non-TTY without
  `--yes` does not apply.

---

## Workstream C — manifest migration, docs, parity — runs AFTER A (parallel with B)

Touch ONLY: `dotfilesync.toml`, `fixtures/`, `docs/manifest.md` (new), `docs/plan-apply.md` (new),
`README.md`, and core test fixtures under `crates/dotfilesync-core/tests/fixtures*` if needed.
- Migrate `dotfilesync.toml` to `schema_version = 2`: add `[setup] login_shell = "fish"`,
  `fisher = true`; add a `personal` and `work` profile example (work excludes games:
  `minecraft`, `roblox`, `steam`, `melonds`); `[select] default = "personal"`. Add missing fish
  `conf.d` files that exist in the repo. Keep `local.fish` as override only.
- Fonts: `font-0xproto` is already a cask → covered by brew actions; note it in docs.
- `docs/plan-apply.md`: workflow (plan → review → approve → apply → resume), safety (dry-run
  default, `--yes`, no silent sudo, backups dir, local overrides never overwritten), the
  **reversible vs irreversible action table**, setup.sh parity matrix (what's ported, what's not
  yet: fnm/node/pnpm, xcode, LM Studio models → #5), and that `setup.sh` remains until parity.
- `docs/manifest.md`: v2 schema reference, profiles, selection rules, merge order with a worked example.
- README: short section linking both docs; update "read-only" wording.
- Verify the migrated manifest parses with `cargo run -p dotfilesync-cli -- plan --manifest dotfilesync.toml --json`
  only if B is done; otherwise add a core test that `parse_manifest(include_str!("../../../dotfilesync.toml"))` succeeds.

## Acceptance checklist (from the issue)
- [ ] Profile merge unit tests (base, overlay, conflicting keys) — A1
- [ ] Plan output identical across runs; JSON + human formats — A2, B3
- [ ] Apply tested with temp HOME + fake brew: backups, journal, resume skips completed — A3
- [ ] Converged system → empty plan — A2/A3
- [ ] CLI-started apply continues after CLI exits; status re-readable — B2
- [ ] Docs list reversible vs irreversible actions — C
