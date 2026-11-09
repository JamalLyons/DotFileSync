# dotfilesyncd manual QA (macOS)

The automated tests use temp directories, a fake `brew`, and a fake `launchctl`. This checklist
covers what they cannot: the real per-user launchd domain on a real Mac. Run it as a normal user,
never with `sudo`.

```bash
cargo build --release --workspace
export PATH="$PWD/target/release:$PATH"
```

## 1. Fresh machine, no daemon

| Step | Command | Expected |
|---|---|---|
| 1.1 | `dotfilesync service status` | `installed: no`, `loaded in launchd: no`, `Daemon: not running`. |
| 1.2 | `dotfilesync scan` | Succeeds. stderr says `dotfilesyncd is not running; running scan standalone`. |
| 1.3 | `ls ~/Library/LaunchAgents/io.github.jamallyons.dotfilesyncd.plist` | Does not exist. `scan` never installs the service. |
| 1.4 | `dotfilesync scan --mode daemon` | Fails, explaining how to install or start the service. |

## 2. Install is not start

| Step | Command | Expected |
|---|---|---|
| 2.1 | `dotfilesync service install` | Prints `Installed: …plist` and `Start at login: disabled`, and says the daemon was not started. |
| 2.2 | `launchctl print gui/$(id -u)/io.github.jamallyons.dotfilesyncd` | Fails with "Could not find service". |
| 2.3 | `plutil -lint ~/Library/LaunchAgents/io.github.jamallyons.dotfilesyncd.plist` | `OK`. `RunAtLoad` is `false` and there is no `KeepAlive`. |

## 3. Start, use, reconnect

| Step | Command | Expected |
|---|---|---|
| 3.1 | `dotfilesync service start` | `dotfilesyncd … is running (pid N)`. |
| 3.2 | `stat -f '%Sp %Su' ~/Library/Application\ Support/dotfilesync{,/run,/run/dotfilesyncd.sock}` | `drwx------` for the directories, `srw-------` for the socket, all owned by you. |
| 3.3 | `lsof -nP -a -p <pid> -i` | No output: no TCP or UDP sockets. |
| 3.4 | `dotfilesync scan` | stderr says `running scan as job j-…`, followed by the report. |
| 3.5 | `dotfilesync status --manifest dotfilesync.toml --detach` | Prints a job ID. |
| 3.6 | Close the terminal, open a new one, then run `dotfilesync jobs watch <id>` | Shows the job's progress and ends with `succeeded`. |
| 3.7 | `dotfilesync jobs result <id>` | The status report. |

## 4. Stop, crash, login behavior

| Step | Command | Expected |
|---|---|---|
| 4.1 | `dotfilesync service stop` | Stopped and unloaded. `service status` shows installed yes, loaded no. The socket file is gone. |
| 4.2 | `dotfilesync service start`, then `kill -9 <pid>`, then `dotfilesync scan` | Falls back to standalone with the "not running" note. There is no automatic restart. |
| 4.3 | `dotfilesync service start` | Starts again despite the stale socket left by `kill -9`. |
| 4.4 | Log out and back in (start-at-login disabled) | `service status`: installed, and loaded or not, but `Daemon: not running`. |
| 4.5 | `dotfilesync service install --start-at-login --force`, then log out and in | `Daemon: reachable`. |

## 5. Uninstall keeps data

| Step | Command | Expected |
|---|---|---|
| 5.1 | `dotfilesync service uninstall` | Unloads the agent and removes the plist. Prints that state and logs were kept. |
| 5.2 | `ls ~/Library/Application\ Support/dotfilesync` | Still present. Your dotfiles and repositories are untouched. |
| 5.3 | `grep -ri -e ghp_ -e password ~/Library/Application\ Support/dotfilesync` | No secrets. Only job IDs, kinds, statuses and timestamps appear. |

Clean up afterwards with `rm -rf ~/Library/Application\ Support/dotfilesync` once you no longer
need the state.
