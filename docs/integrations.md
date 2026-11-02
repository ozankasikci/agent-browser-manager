# Agent integrations

These instructions use the Rust stdio MCP server bundled with this project. No third-party browser MCP package or separate language runtime is needed.

Build and initialize with `scripts/setup.sh`, then start the intended profile. Config generation is read-only and uses an absolute path to the executable you invoke. Generate snippets with `bin/abm`, not a temporary build path.

## Claude Code

Generate a JSON snippet:

```sh
./bin/abm config claude --profile development
```

For a project-local connection, merge the generated `mcpServers` entries into that project's `.mcp.json`. The checked-in `configs/claude.mcp.example.json` contains portable templates for all three profiles. Replace the placeholder binary path. Do not overwrite unrelated servers or enable personal/business identities unless intended.

Alternatively, use Claude Code's registration command after substituting your actual binary path:

```sh
claude mcp add --transport stdio --scope user abm-development -- \
  /ABSOLUTE/PATH/agent-browser-manager/bin/abm mcp \
  --profile development --agent claude
```

`--scope user` intentionally makes that connection available across projects. Omit it for Claude Code's local scope, or use `--scope project` to write a project entry. Options before `--` belong to Claude; everything after it starts this Rust server. Inspect the connection through `/mcp` or `claude mcp get abm-development`. Project configuration may require client approval.

Reference: [Claude Code MCP documentation](https://code.claude.com/docs/en/mcp).

## Codex

Generate TOML tables:

```sh
./bin/abm config codex --profile business
```

Merge those tables into `~/.codex/config.toml`, or a trusted project's `.codex/config.toml`. Do not create a duplicate table with an existing name. The desktop app, CLI, and IDE extension use the host's MCP configuration. The portable template is `configs/codex.config.example.toml`.

An equivalent registration command is:

```sh
codex mcp add abm-business -- \
  /ABSOLUTE/PATH/agent-browser-manager/bin/abm mcp \
  --profile business --agent codex
```

The manager invokes no Node runtime. If your Codex CLI itself is distributed through npm, that is independent of this project's Rust server. You can merge the TOML directly without invoking that CLI. Inspect the connection in the client's MCP controls or `/mcp` where available.

Reference: [Official Codex MCP configuration documentation](https://developers.openai.com/codex/mcp/).

## Antigravity

Generate a JSON snippet:

```sh
./bin/abm config antigravity --profile development
```

Open Antigravity's MCP management settings and raw configuration view. Merge the generated entries under `mcpServers`. Current official documentation gives the shared global path as `~/.gemini/config/mcp_config.json`, with workspace configuration in `.agents/mcp_config.json`. Older installations used `~/.gemini/antigravity/mcp_config.json`.

On the inspected computer, the legacy path is a symlink to the shared global file. Do not replace that symlink or overwrite the full shared config. Refresh MCP servers or reload the client after a deliberate merge. The portable template is `configs/antigravity.mcp.example.json`.

Reference: [Google Antigravity MCP documentation](https://antigravity.google/docs/mcp).

## Choosing profiles and labels

`config <client>` exposes only development by default. `--all-profiles` prints connections for all defaults and registered custom profiles; each entry starts its own stdio server restricted to its named profile. The static templates contain only the three defaults. `--agent codex-ticket-123` sets a readable ownership label. Labels are not authorization tokens and do not allow a new process to resume old tabs.

Custom names work with every client. Create or start the profile first, then generate the selected client's snippet:

```sh
./scripts/start.sh studio-team
./bin/abm config claude --profile studio-team
./bin/abm config codex --profile studio-team
./bin/abm config antigravity --profile studio-team
```

These snippets use the server name `abm-studio-team` and route to that profile's registered directory and stable port. They do not alias the business profile or copy its login data.

Profiles removed through `profile remove NAME` are omitted from `--all-profiles` output, including removed defaults. Existing client settings are not rewritten automatically; remove an old connection from the client deliberately if it is no longer wanted.

If you use a custom data directory, generate the config with that global option:

```sh
./bin/abm --data-dir '/absolute/private/runtime' config codex --profile development
```

The generated args retain that path. Static templates use the default data directory.

## Extension-based browser tools

Visible `start` launches allow extensions. Install each extension inside the intended ABM Chrome profile; installation in your regular Chrome profile does not install it here. Headless launches and temporary export/import launches continue to disable extensions. Sync remains disabled.

After updating and rebuilding the manager, finish unsaved work in the ABM window, then restart that profile:

```sh
./scripts/stop.sh longhorn-games
./scripts/start.sh longhorn-games
./bin/abm status longhorn-games
```

The running status should report `extensions_allowed: true`. This means the launch permits extensions, not that any particular extension is installed or connected. If `start` finds an existing browser with different extension settings, it reports `restart_required: true` and leaves it open.

### Claude in Chrome

Install the [official Claude extension](https://chromewebstore.google.com/detail/claude/fcoeoabgfenejglbffodgkkbkcdhcgfn) in the ABM window and sign in with the Claude account and organization used by your client. For Claude Code, enable Chrome with `claude --chrome`, or run `/chrome` to reconnect and explicitly choose the Longhorn browser. In Claude Desktop, use the Claude in Chrome connector's connection controls to select this browser.

Because ABM uses a dedicated Chrome user-data directory, Claude's native messaging registrations can need to be copied from their normal Chrome location into this profile's `chrome/NativeMessagingHosts/` directory. These files describe the local connection programs; they do not contain browser cookies. First let the installed Claude clients create their registrations. With the ABM profile stopped, the following copies only missing Claude registrations and leaves the original files and unrelated registrations alone:

```sh
abm_chrome_root="$HOME/Library/Application Support/Agent Browser Manager/longhorn-games/chrome"
mkdir -p -m 700 "$abm_chrome_root/NativeMessagingHosts"
(
  umask 077
  for host in com.anthropic.claude_browser_extension com.anthropic.claude_code_browser_extension; do
    source_host="$HOME/Library/Application Support/Google/Chrome/NativeMessagingHosts/$host.json"
    if [ -f "$source_host" ]; then
      cp -n "$source_host" "$abm_chrome_root/NativeMessagingHosts/$host.json"
    fi
  done
)
./scripts/start.sh longhorn-games
```

Adjust the runtime path for a different ABM profile or data root. Existing destination registrations are not refreshed by this command; if Claude is updated and a connection breaks, inspect the registration against the currently installed client before deliberately replacing it. On another computer, prepare that computer's own registrations after import. Exports omit native messaging registrations, and imports remove copies found in older archives before launching Chrome. No native connection program is installed by ABM.

References: [Claude Chrome connection setup](https://code.claude.com/docs/en/chrome), [Claude Desktop connection troubleshooting](https://support.claude.com/en/articles/12902405-claude-in-chrome-troubleshooting), and [Chrome's native messaging host lookup](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging).

### Playwright

Install the [official Playwright Extension](https://chromewebstore.google.com/detail/playwright-extension/mmlmfjhmonkocbjadbfplnigmagldckm) in the same ABM window. In your existing Playwright MCP server configuration, use its `--extension` option and approve the connection to the intended ABM tab. Verify which browser was selected; its default last-used profile can be your regular Chrome. The `--profile-dir-name` option names an internal Chrome directory such as `Profile 1`, not the ABM manager name `longhorn-games`; those directory names can occur in multiple Chrome user-data roots.

For deterministic routing, an existing Playwright MCP server can instead use `--cdp-endpoint=http://127.0.0.1:9225` for this profile's registered endpoint. Check `abm profile list` for the actual port on the current computer. Direct CDP attachment does not require the Playwright extension. Neither method requires adding another runtime or MCP implementation to this Rust project; any existing external Playwright installation remains separate. Do not put connection tokens in tracked configuration.

Claude's extension and Playwright have their own permissions and do not participate in ABM's MCP tab ownership. Give each agent separate tabs, and avoid simultaneous control of one tab. ABM does not automatically change your external MCP settings or prove that those clients have connected.

Reference: [Playwright browser connections](https://playwright.dev/mcp/configuration/browser-extension).

## First connected task

1. Start the matching Chrome identity yourself through the CLI.
2. Sign into a site manually in that identity when needed.
3. Ask the agent to use `profile_status`, `open_tab`, and then `read_page` with the returned `target_id`.
4. Verify that `list_tabs` contains only the tabs that MCP session created.
5. Close task tabs with `close_tab`. Stopping the MCP server leaves Chrome running.

Chrome tools cannot operate until the profile is started. The MCP initialize handshake still works while Chrome is stopped, so a client can discover tools without launching browsers.

The included Rust test exercises the stdio protocol and real Chrome operations directly. It does not prove that every installed agent UI has loaded or enabled the supplied config. Existing client settings are left intact by setup, config generation, and tests.
