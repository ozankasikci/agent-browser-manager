# Local verification

Verified on 1 November 2026, Europe/London.

This is the initial three-preset snapshot. See [named profile verification](named-profiles-verification.md) for the later custom-profile checks.

## Environment inspected

- macOS 26.1, Apple Silicon arm64.
- Rust and Cargo 1.90.0.
- Google Chrome 156.0.8078.74 installed at its normal macOS application path.
- Claude Code and Codex launchers present; Antigravity application and existing MCP configuration present.
- The requested project directory did not exist before this work. No existing project was overwritten.
- `~/projects` and `~/Projects` resolve to the same directory on this computer. The repository's resolved path is `/Users/ozan/Projects/agent-browser-manager`.
- Ports 9222, 9223, and 9224 had no listeners before verification.

## Implemented and installed

- Rust CLI and Rust stdio MCP server, version 0.1.0.
- Release binary installed at `/Users/ozan/Projects/agent-browser-manager/bin/abm`.
- Three private persistent profile directories initialized beneath `/Users/ozan/Library/Application Support/Agent Browser Manager`.
- README, setup/start/stop/status/verification scripts, portable client configs, config generator, security documentation, and Rust tests.
- Local Git repository on `main`, with separate descriptive commits. This initial verification checkpoint preceded GitHub repository creation and publication.

## Checks completed

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Passed |
| `cargo clippy --all-targets --locked -- -D warnings` | Passed |
| `cargo test --locked` | 14 tests passed; live Chrome test intentionally skipped by default |
| Opt-in live Chrome test, debug build | Passed after endpoint-discovery and loading-state fixes |
| `cargo test --release --locked --test live_chrome -- --ignored --nocapture --test-threads=1` | Passed against the release build |
| Shell syntax checks for all scripts | Passed |
| JSON/TOML generated config and static template parsing | Passed in Rust tests |
| `scripts/setup.sh` | Built release binary, installed it, and initialized the private runtime root |
| Normal non-headless launch/status/stop via installed scripts | Passed for personal, business, and development together |
| Actual listener inspection in normal Chrome mode | Exactly `127.0.0.1:9222`, `127.0.0.1:9223`, and `127.0.0.1:9224`, owned by their managed Chrome processes |
| Private root and identity directory permissions | Confirmed `drwx------` |

The live Rust integration test used disposable, signed-out Chrome profiles and a Rust HTTP fixture listening on an ephemeral localhost port. It exercised:

- All three fixed endpoints running together, with verified process and CDP identity.
- Two processes racing to start one profile, yielding one new start and one idempotent result.
- Refusal to adopt or stop an unrelated process occupying a fixed port.
- Failed-launch cleanup using a harmless executable that exits immediately.
- MCP initialize, initialized notification, tool discovery, and tools/call over actual subprocess stdio.
- Tab creation, listing, navigation, accessibility reading, text insertion, clicking a form submit button, and tab closure.
- Rejection of foreign-session navigation, reading, clicking, typing, and closure, including clients with identical agent labels.
- Rejection of explicit file-URL navigation.
- Abandoned-session tabs remaining unavailable to a new session.
- Distinct cookie values in all three profile directories, remaining distinct and present after graceful browser stop and restart.
- A fresh ownership run after browser restart, without deleting persistent site data.
- Cleanup of managed test browsers and a final stopped state.

A separate test verifies that a forged browser record cannot cause the manager to stop an unrelated live process. Other tests cover wildcard/external listener rejection, wrong-PID rejection, endpoint validation, filesystem permissions and links, unknown tab ownership, bounded MCP framing, and protocol initialization.

## Final state and limits

All managed browsers were stopped after verification. The persistent profile directories and release binary remain ready for use. No startup daemon was installed and no CDP endpoint was left intentionally running.

No real accounts were signed into, and no credentials were copied from regular Chrome. Synthetic cookie persistence demonstrates retained browser storage; it does not prove a real site's authentication or MFA behavior.

The three existing agent settings files were byte-identical before and after the normal-mode verification, as checked by SHA-256. Setup and config generation did not edit them. The supplied configurations have not been enabled or tested inside the installed Claude Code, Codex, or Antigravity UIs. Their formats were checked against primary documentation and parsed locally. Actual MCP/browser operation was tested through the Rust stdio client harness.

Normal-mode process startup and endpoints were verified without a visual review of the browser windows. Advanced browser features, screenshots, downloads, shadow DOM, iframe actions, blocked shutdown dialogs, and Linux/Windows behavior were not validated.

CDP remains unauthenticated local browser control. Tab ownership protects cooperating MCP clients from accidental direct tab interference. It does not isolate shared account state or prevent a local program with direct CDP or same-user filesystem access from bypassing the policy.

## Next use

```sh
cd ~/projects/agent-browser-manager
./scripts/start.sh development
./bin/abm config codex --profile development
```

Sign into sites manually in the appropriate dedicated identity, then deliberately merge the chosen client's generated snippet. Start personal or business only when you need that identity. Stop through the matching script when finished.
