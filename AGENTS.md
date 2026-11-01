# Project conventions

Use Rust for the CLI, browser protocol, MCP server, and automated tests. Shell wrappers may invoke the compiled binary. Do not add Node.js, npm, npx, JavaScript runtimes, or runtime package downloads.

Keep the three CDP ports fixed and bind Chrome only to 127.0.0.1. Never attach to an unmanaged browser, import a normal Chrome profile, disable Chrome's sandbox, or add permissive remote-origin flags.

Every MCP tab operation must validate the session owner. Do not add arbitrary CDP forwarding, JavaScript evaluation tools, cookie export, or automatic tab takeover. Tab ownership coordinates cooperating clients; it is not isolation from local processes with CDP access.

Never commit browser state, credentials, visited URLs, or page content. Keep stdout exclusively JSON-RPC when serving MCP. Do not modify existing agent configuration automatically.

Use descriptive commits for each coherent change. Avoid em dashes in generated prose.

Before completing a change run cargo fmt --check, cargo clippy --all-targets -- -D warnings, and cargo test --locked. Run the opt-in Chrome integration test only when the fixed ports are free. Document verified behavior and any untested surfaces precisely.
