# Encrypted whole-profile transfer

Export/import is an explicit Rust CLI operation. It transfers one manager profile's Chrome user-data directory, together with portable cookies collected from its persistent Chrome profiles. There is no website allowlist. It does not add cookie access to the MCP tools.

This version supports **Mac-to-Mac transfers**. The destination needs this manager and a compatible current Google Chrome installation. Cross-platform imports are rejected rather than treated as verified.

## Export on the source Mac

Install the current binary with `scripts/setup.sh` if needed. Export while the selected profile is running to capture its available live session cookies before closing it:

```sh
cd ~/projects/agent-browser-manager
mkdir -p -m 700 exports
./bin/abm profile export longhorn-games --stop \
  --output "$PWD/exports/longhorn-games.abm.age"
```

The command prompts twice for a passphrase without echoing it. Use a strong passphrase, at least 12 bytes long. The output must be an absolute path outside the manager runtime directory. This project's `exports/` folder is gitignored. Inside any Git repository, export verifies that both the destination directory and archive are ignored, and refuses tracked archive paths, even if deleted from disk. Git must be available for that check. Filename-only ignore rules are insufficient because a temporary encrypted file is created in the same directory. Existing output files are never overwritten. Keep the encrypted archive and its passphrase separate.

The `--stop` option explicitly allows the command to capture the active Chrome profile's cookies and then close that browser gracefully. Without it, a running source is refused. Export holds the lifecycle lock, briefly launches any remaining persistent internal Chrome profiles headlessly, asks verified local Chrome for their available cookies, and closes them. A source that is already stopped can also be exported, but session cookies already discarded by Chrome cannot be recovered.

The command streams browser files and cookie state directly into an authenticated encrypted archive. It does not write a plaintext archive or a plaintext cookie sidecar file. Source profile data is retained; normal Chrome startup/shutdown may update browser bookkeeping. Do not open that profile manually during export.

The export leaves the source stopped. Start it again normally if you want to keep using it:

```sh
./scripts/start.sh longhorn-games
```

## Import on the destination Mac

Copy the `.abm.age` file through your chosen transfer method. Install and initialize the manager, then import:

```sh
cd ~/projects/agent-browser-manager
./scripts/setup.sh
./bin/abm profile import "$HOME/Downloads/longhorn-games.abm.age"
./scripts/start.sh longhorn-games
```

Import prompts for the passphrase. By default, it uses the archived manager profile name. It requires a new name and refuses to overwrite any existing profile or directory. If that name is already in use, choose a different manager name:

```sh
./bin/abm profile import "$HOME/Downloads/longhorn-games.abm.age" \
  --name longhorn-copy
```

The manager name can differ from Chrome's preserved internal display names. A port is assigned from the destination's registry; use `--port` to request a specific available custom port. Removed defaults retain their fixed default ports when explicitly recreated. Existing profiles and their port assignments are preserved.

Import decrypts into a new private staging directory with directories set to mode `700` and files to `600`. It validates the archive and consumes the final encryption authentication tag before launching Chrome or publishing a profile. It briefly starts a managed headless Chrome using staged data, restores cookies through Chrome, and closes it. Chrome writes those cookies using the destination's own encryption. Only then is the profile installed and registered. The final imported profile is stopped.

Imported profiles use Chrome's session-resume option on subsequent launches so restored session cookies survive the import shutdown. Export also enables this for the source when session cookies were captured. This can reopen saved tabs. It does not extend cookie expiry or bypass a site's session checks.

If temporary Chrome cannot be stopped, private recovery data is retained and its location is reported. A failed import does not overwrite an existing destination profile. Crashes or forced termination can leave a private `.abm-import-*` staging directory; inspect it before any deliberate cleanup.

## What transfers

- Persistent browser files, including bookmarks, history, preferences, local storage, IndexedDB, extension data, and other data stored beneath the selected Chrome user-data directory.
- Available cookies from every persistent internal Chrome profile recorded in its profile-info cache, including HttpOnly cookies, session cookies, expiry, SameSite and supported partitioning attributes.
- Original encrypted password database files as browser files. This does **not** make their OS-protected password contents portable.

Machine/process state such as Chrome singleton locks, debugging endpoint files, and running-version links is excluded. Manager process records, agent tab grants, manager registry entries, agent configuration backups, and removed-profile archives are outside the selected Chrome directory and are not exported. Incognito/guest in-memory sessions are not transferred. Symbolic links, hard links, sockets and other special files are rejected, except the explicitly excluded Chrome runtime links.

## Login limits and reporting

A restored cookie is not proof that a website will accept the session. Device-bound sessions, hardware keys, passkeys, operating-system credentials, expired or revoked sessions, and website risk checks may still require signing in or MFA again. The manager does not extract macOS Keychain keys, export saved passwords as plaintext, or bypass device binding.

Opaque partitioned cookies that Chrome cannot represent portably are skipped and counted. Import reports cookies restored as observed through Chrome, cookies not restored, cookie-profile count, and skipped opaque-cookie count. It does not print cookie names, values, domains, or passphrases. It does not automatically visit every website to check its login.

Live tab state and authentication held only in a page's memory cannot be guaranteed. Use `--stop` with the running source for the best available cookie capture. Other internal Chrome profiles are captured through a temporary launch, so their previously discarded session cookies cannot be recovered. The package carries the browser data and available cookie state; it cannot promise that every login is transferable.

The archive uses the standard age format with passphrase-based scrypt at work factor 18 and authenticated streaming encryption. Its payload is a versioned tar archive. Import caps scrypt work at 18, accepts regular files and directories only, rejects unsafe/duplicate paths and unsupported metadata, and limits extracted data to 20 GiB and 200,000 entries by default. `--max-bytes` deliberately changes the data-size limit on export or import. No compression is used.

For automation, both commands accept `--passphrase-file /absolute/private/file`. The file must be a regular, unlinked file owned by you with mode `600`; it contains a UTF-8 passphrase, with trailing line endings ignored. Passphrases are not accepted as literal command-line arguments or environment variables. Interactive use avoids creating a passphrase file.

## Connect agents after import

MCP configuration contains computer-specific paths, so generate it on the destination:

```sh
./bin/abm config claude --profile longhorn-games
./bin/abm config codex --profile longhorn-games
./bin/abm config antigravity --profile longhorn-games
```

Merge only the intended entries into the destination agent's settings. For Claude Desktop's Code tab, also account for its local/SSH configuration sources as described by that client; a terminal registration on one Mac is not a registration on a different SSH host.

## Verification scope

Automated checks use disposable profiles and synthetic credentials. They cover encryption round trips, wrong passphrases, tampered/truncated archives, unsafe extraction, private file permissions, source-running refusal, destination conflicts, and a real-Chrome round trip for persistent and session cookies, local storage, IndexedDB, and ordinary profile files. A second Chrome check verifies that two internal profiles retain separate cookie jars. Run these opt-in browser checks with `scripts/verify.sh --chrome` when the managed test ports are free. Real account logins and a physical second-Mac transfer still require user verification. No actual Longhorn login state is exported by the tests.

Sources: [age Rust API](https://docs.rs/age/0.11.5/age/), [Chrome Storage protocol](https://chromedevtools.github.io/devtools-protocol/tot/Storage/), [Chromium credential storage](https://github.com/chromium/chromium/blob/main/docs/security/faq.md), and [device-bound sessions](https://developer.chrome.com/blog/dbsc-windows-announcement).
