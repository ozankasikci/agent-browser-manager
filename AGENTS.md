# Project conventions

Use Rust for the CLI, browser protocol, MCP server, and automated tests. Shell wrappers may invoke the compiled binary. Do not add Node.js, npm, npx, JavaScript runtimes, or runtime package downloads.

Keep the default CDP ports fixed at 9222, 9223, and 9224. Named profiles receive distinct stable ports from 9225 through 9324 in the private registry. Bind Chrome only to 127.0.0.1. Never attach to an unmanaged browser, import a normal Chrome profile, disable Chrome's sandbox, or add permissive remote-origin flags.

Every MCP tab operation must validate the session owner. Do not add arbitrary CDP forwarding, JavaScript evaluation tools, cookie export tools, or automatic tab takeover. Explicit CLI export/import may transfer cookie state only inside an authenticated encrypted archive, as requested by the user. Tab ownership coordinates cooperating clients; it is not isolation from local processes with CDP access.

Never commit browser state, credentials, visited URLs, or page content. Keep stdout exclusively JSON-RPC when serving MCP. Do not modify existing agent configuration automatically.

Use descriptive commits for each coherent change. Avoid em dashes in generated prose.

Before completing a change run cargo fmt --check, cargo clippy --all-targets -- -D warnings, and cargo test --locked. Run the opt-in Chrome integration test only when the fixed ports are free. Document verified behavior and any untested surfaces precisely.
