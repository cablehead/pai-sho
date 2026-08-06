## Git Commit Style Preferences

When committing: review `git diff`

- Use conventional commit format: `type: subject line`
- Keep subject line concise and descriptive
- **NEVER include marketing language, promotional text, or AI attribution**
- **NEVER add "Generated with Claude Code", "Co-Authored-By: Claude", or similar spam**
- Follow existing project patterns from git log
- Prefer just a subject and no body, unless the change is particularly complex

Example good commit messages:
- `feat: add peer auto-reconnection`
- `fix: handle port binding conflicts gracefully`
- `refactor: simplify tunnel forwarding logic`
- `test: add integration tests for peer discovery`

## ASCII Only

All text in the repo must be ASCII only. No em-dashes, smart quotes, emoji, or other non-ASCII characters. Use `--` instead of em-dashes, plain quotes, and plain text markers like `WARNING:` instead of emoji.

## Code Quality

Before committing:
1. `cargo fmt` - fix formatting
2. `cargo clippy` - fix lints
3. `cargo test` - run tests

## Key Concepts

- **Daemon**: Single iroh Endpoint, one ticket, manages all peers
- **Peer**: Remote daemon identified by EndpointId
- **Expose**: Declare a specific TCP port available to peers (explicit, not full network access)
- **Surface**: A peer's ports addressed as a unit at a dedicated local IP, named after its enrollment label
- **Auto-project**: On its first announced port a peer is projected automatically -- an address is allocated and its ports bind there, so reach is automatic. `project`/`unproject` are the override (pin an IP, rename, toggle off). See docs/adr/0004 and 0005
- **Resolver**: With `--resolver`, the daemon answers `<name>.pai-sho` from live surfaces (`vibenv-ndyg.pai-sho`). Authoritative for one suffix; never touches the system resolver
