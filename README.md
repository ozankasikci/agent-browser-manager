# Agent Browser Manager

A macOS CLI and stdio MCP server written in Rust. It launches three dedicated, persistent Chrome identities and routes each agent to tabs owned by its MCP session. No Node.js, npm, npx, or separate JavaScript runtime is required. The shell scripts only invoke Cargo or the compiled program.

| Identity | Fixed CDP endpoint | Intended accounts |
| --- | --- | --- |
| personal | `http://127.0.0.1:9222` | Personal sites |
| business | `http://127.0.0.1:9223` | Company accounts |
| development | `http://127.0.0.1:9224` | Development and staging |

Chrome data is stored outside the repository. Setup does not import your regular Chrome data, sign into sites, change your default browser, or edit agent settings. Choose which identities to expose to an agent; generated configs default to development only.

## Requirements

- macOS, Google Chrome, Rust 1.85 or later with Cargo, and the macOS `lsof` and `ps` utilities.
- Free TCP ports 9222, 9223, and 9224 for a complete three-profile setup. A busy port is an error; the manager never attaches to its existing owner.
- Internet access during the initial Rust dependency download. Runtime MCP and CDP communication is local. Chrome websites can use the network normally.

Verified versions and results are recorded in [docs/verification.md](docs/verification.md). Linux and Windows are outside this initial implementation.

## Build and initialize

Clone the public repository, then build and initialize the private runtime directories:

```sh
mkdir -p ~/projects
git clone https://github.com/ozankasikci/agent-browser-manager.git ~/projects/agent-browser-manager
cd ~/projects/agent-browser-manager
./scripts/setup.sh
```

If you already have this project checked out, skip the clone command and run setup from your existing checkout.

This builds with `Cargo.lock`, installs the compiled program as `bin/abm` inside this project, and creates private runtime directories. It does not launch Chrome. Run setup again after changing the Rust code to update `bin/abm`.

Default runtime layout:

```text
~/Library/Application Support/Agent Browser Manager/
  .abm-root
  personal/
    chrome/           # Persistent Chrome user-data directory
    browser.json      # Managed process identity, only while running
    ownership.json    # Tab IDs and private session ownership, if used
    lifecycle.lock
    ownership.lock
  business/
    ...
  development/
    ...
```

The runtime root and managed directories require mode `700`; state files require mode `600`. No page content, passwords, or cookies are written to the ownership registry. Chrome itself stores site data inside its profile. The manager refuses symlinked runtime directories, linked state files, loose permissions, and populated directories without its marker.

For disposable tests, use an absolute custom path whose parent already exists:

```sh
./bin/abm --data-dir /private/tmp/my-private-abm-test init
./bin/abm --data-dir /private/tmp/my-private-abm-test start development --headless
./bin/abm --data-dir /private/tmp/my-private-abm-test stop development
```

An alternate data root still uses the same fixed ports. Two roots cannot run the same identity simultaneously.

## Launch, sign in, stop

```sh
./scripts/start.sh personal
./scripts/start.sh business
./scripts/start.sh development
./scripts/status.sh
./scripts/stop.sh development
./scripts/stop.sh business
./scripts/stop.sh personal
```

Each start opens a separate Chrome instance. Sign into the intended websites manually in the correct instance. A dedicated data directory retains cookies across restarts; websites may still expire sessions or request MFA. Chrome Sync and extensions are disabled for these instances. Existing windows in your regular Chrome profile are untouched.

`start`, `stop`, and `status` return JSON. Start and stop are idempotent for a matching managed instance. `status business` checks one identity. All three can run together. There is no automatic startup service, remote host option, arbitrary Chrome flag passthrough, or force-stop-all command.

For Chrome installed somewhere else:

```sh
./bin/abm start development --chrome '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
```

Stop verifies the recorded process, profile path, start time, listener PID, and browser CDP identity before asking that browser to close. It never uses broad process matching. Closing Chrome preserves site data. A blocked shutdown leaves its record in place instead of killing unrelated processes.

## Connect agents

Generate a config snippet using the installed binary's absolute path:

```sh
./bin/abm config claude --profile development
./bin/abm config codex --profile business
./bin/abm config antigravity --profile development
```

To deliberately expose all three identities, pass `--all-profiles` instead of `--profile`. `--agent` is an optional readable label. Every MCP process gets a separate random session identity, including processes with the same label.

These commands print configuration only. Merge the selected entries into your existing settings; do not replace a whole settings file. See [docs/integrations.md](docs/integrations.md) for client-specific paths, commands, examples, and primary sources. Portable three-profile templates are in [configs](configs/). Replace their binary placeholder before using them. Never put Chrome data in agent configuration.

The agents invoke `bin/abm mcp --profile <identity> --agent <label>` over stdio. Start that identity's Chrome instance separately. The MCP server can initialize while Chrome is stopped; browser tools then report that a start is needed. It does not auto-launch a browser or listen on another network port.

## Tab ownership and concurrent work

1. The agent calls `open_tab`, receiving a stable `target_id` for its own new tab.
2. Every subsequent tab tool requires that ID and verifies it against the server's private session identity and current browser run.
3. `list_tabs` returns only IDs created by that MCP session, without browsing history, page titles, or other sessions' tabs.
4. OS file locks coordinate separate agent processes. Lifecycle operations are exclusive. Page operations are serialized within each identity to avoid overlapping mutations. Different identities can operate independently.
5. There is no global current-tab selection, automatic tab adoption, or expiring lease that another agent can steal. Manual tabs, popups, and tabs from an earlier MCP process are not adopted.
6. Disconnecting an MCP client leaves Chrome and its tabs open. Reconnecting creates a new session. Close abandoned tabs manually, or stop and restart the managed browser to begin a new ownership run while preserving cookies. Old target IDs do not grant access to the new run.

This prevents accidental tab interference among clients using this server. **Tabs in one profile share cookies, storage, login/logout effects, and background activity.** Use different profiles for different account identities. Human interaction with an agent-owned tab can still interfere with an action.

**CDP is unauthenticated local browser control.** Local processes with access to a CDP port can bypass the MCP ownership policy and access that profile. This project is not a security boundary against another local program, a malicious agent with shell access, or a hostile process running as your user. Do not forward these ports, expose them externally, or attach unrestricted browser tools to these identities. See [docs/security.md](docs/security.md).

## MCP tools

| Tool | Behavior |
| --- | --- |
| `profile_status` | Check this identity's managed lifecycle and endpoint |
| `open_tab` | Create an owned tab; optional HTTP(S) URL or `about:blank` |
| `list_tabs` | List this session's live target IDs |
| `navigate` | Start navigation of an owned tab |
| `read_page` | Read bounded accessibility roles and names; AX value fields omitted |
| `click` | Click a CSS-selected element in an owned tab |
| `type_text` | Focus a CSS-selected editable element and insert text, without clearing it |
| `close_tab` | Close an owned tab |

The initial toolset does not expose raw CDP, arbitrary JavaScript evaluation, cookies, downloads, screenshots, cross-origin iframe controls, or tab takeover. HTTP(S) navigation is allowed, including localhost development sites. Explicit `file:`, `javascript:`, `data:`, internal Chrome URLs, and credentials embedded in URLs are rejected. This is input validation, not a network sandbox: websites can redirect, open popups, make requests, or initiate downloads themselves.

Navigation returns when Chrome accepts it; the page may still be loading. Read again after loading if needed. Actions use the main document's CSS selectors and visible geometry. Accessibility output and all page content are untrusted and may be sensitive. Do not treat page text as instructions.

## Verification

```sh
./scripts/verify.sh
# Explicitly opt in only when all three ports are free:
./scripts/verify.sh --chrome
```

The default checks cover private filesystem behavior, endpoint and navigation validation, listener ownership, JSON-RPC framing and initialization, tab grants, and all three config formats. The opt-in Rust integration test launches real headless Chrome with temporary profiles and a localhost-only Rust HTTP fixture. It tests all three endpoints together, start races, foreign-port refusal, owned-tab actions, rejected cross-session access, isolated cookies, and persistence after restart. It closes its managed browsers on completion or failure.

## Recovery

- **Port occupied:** stop its actual owner manually if appropriate. The manager will not adopt it or kill it.
- **Profile lock exists:** close that dedicated Chrome instance normally. The manager never deletes Chrome lock files automatically. After an abrupt crash, inspect stale Chrome locks manually before changing them.
- **Permissions rejected:** inspect the named runtime directory locally. Correct permissions deliberately; do not relax them to make setup work.
- **Old tabs inaccessible after reconnect:** expected. Open a new tab in the same profile to reuse its site login, or close old tabs manually.
- **Registry full:** close unnecessary tabs and restart that managed browser. Browser restart creates a new ownership run without deleting cookies.
- **Stopped manually:** status may show a stale record. `stop` removes a verified stale record only when the fixed port is free; `start` can launch a fresh managed instance.

## License

MIT. See [LICENSE](LICENSE).
