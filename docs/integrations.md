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

`config <client>` exposes only development by default. `--all-profiles` prints all three connections; each entry starts its own stdio server restricted to its named profile. `--agent codex-ticket-123` sets a readable ownership label. Labels are not authorization tokens and do not allow a new process to resume old tabs.

If you use a custom data directory, generate the config with that global option:

```sh
./bin/abm --data-dir '/absolute/private/runtime' config codex --profile development
```

The generated args retain that path. Static templates use the default data directory.

## First connected task

1. Start the matching Chrome identity yourself through the CLI.
2. Sign into a site manually in that identity when needed.
3. Ask the agent to use `profile_status`, `open_tab`, and then `read_page` with the returned `target_id`.
4. Verify that `list_tabs` contains only the tabs that MCP session created.
5. Close task tabs with `close_tab`. Stopping the MCP server leaves Chrome running.

Chrome tools cannot operate until the profile is started. The MCP initialize handshake still works while Chrome is stopped, so a client can discover tools without launching browsers.

The included Rust test exercises the stdio protocol and real Chrome operations directly. It does not prove that every installed agent UI has loaded or enabled the supplied config. Existing client settings are left intact by setup, config generation, and tests.
