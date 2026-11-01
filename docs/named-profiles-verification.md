# Named profile verification

Verified on 1 November 2026 on the same macOS, Rust, and Chrome environment recorded in [the initial verification](verification.md).

- Formatting and strict Clippy checks passed.
- All 18 unit tests and both CLI integration tests passed.
- The extended real-Chrome integration test passed in debug and release builds.
- A user-named profile started automatically with a separate private directory and stable port.
- Chrome retained the custom name in both its profile preferences and its profile-info cache after a real launch and graceful shutdown.
- Named-profile MCP sessions were restricted to their own tabs. A second session could not read the first session's tab.
- Restart retained the assigned endpoint, and the three default endpoints remained unchanged.
- The release integration test terminated the isolated launcher's process group after startup and confirmed that the named Chrome profile stayed running in its independent session. A separate normal-mode status command also confirmed survival after the original launch command finished.
- Concurrent CLI registration of the same name produced one profile and one port. A different registered name received a different port.
- Generated Claude Code, Codex, and Antigravity configs selected the custom name instead of a default identity.
- Unsafe names, reserved ports, duplicate registry entries, and unregistered existing directories were rejected.
- A release-binary smoke check also started a new custom profile in normal non-headless Chrome mode and verified its running status on its assigned loopback endpoint.

The original 14-test snapshot remains in `verification.md` as the initial checkpoint. The feature follow-up has 20 non-browser tests plus the opt-in real-browser integration test. Tests use disposable directories, generic profile names, and a local fixture; private user registry entries and browser data are not included in this repository.

The named-profile update does not rename, copy, or delete existing profile data. It does not update agent-global settings. Real site authentication, client UI activation, and visual review of Chrome windows remain outside the automated verification.
