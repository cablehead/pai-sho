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

## Code Quality

Before committing:
1. `cargo fmt` - fix formatting
2. `cargo clippy` - fix lints
3. `cargo test` - run tests

## Key Concepts

- **Daemon**: Single iroh Endpoint, one ticket, manages all peers
- **Peer**: Remote daemon identified by EndpointId, assigned local 127.0.0.x IP
- **Expose**: Declare a specific TCP port available to peers (explicit, not full network access)
- **Auto-bind**: When peer exposes port, automatically bind `<peer-ip>:<port>` locally
