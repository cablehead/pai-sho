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

- **Daemon**: Single iroh Endpoint with a stable key, manages all peers
- **Peer**: Remote daemon identified by EndpointId
- **Invitation**: `<key>.<code>` -- who to dial, and the proof you may. One side runs `invite`, the other `accept`. `invite <key>` authorizes a key you already know and creates no secret. See docs/adr/0006
- **Expose**: Grant a specific TCP port to specific peer keys. Default deny: no grant, no access. `--to` or `--all` is required
- **Surface**: A peer's ports addressed as a unit at a dedicated local IP, under the name from `--as`, or a truncated key if nothing named it
- **Auto-project**: On its first announced port a peer is projected automatically -- an address is allocated and its ports bind there, so reach is automatic. `project`/`unproject` are the override (pin an IP, rename, toggle off). See docs/adr/0004 and 0005
- **Resolver**: With `--resolver`, the daemon answers `<name>.pai-sho` from live surfaces (`vibenv-ndyg.pai-sho`). Authoritative for one suffix; never touches the system resolver

## Architecture

`src/core/` is pure: it decides, and it does no IO. `session.rs` owns admission
and authorization and returns `Action`s; `invite.rs` parses invitations. The
shell in `peer.rs` feeds it events and carries out what it returns.

Put decisions in `core`, keep IO in the shell. A security decision made inline in
`peer.rs` is unreachable from the unit tests, which is how `expose` shipped a
default-allow once already. See docs/adr/0007.

- `src/core/session.rs` -- admission, grants, tunnel authorization (pure, unit tested)
- `src/peer.rs` -- connections, dialing, retry, binding
- `src/daemon.rs` -- control socket, request handling, state files
- `src/live_tests.rs` -- two real daemons over loopback, relays disabled

## Where the design lives

- `docs/adr/` -- the decisions and why. Append a new ADR that supersedes rather than editing one in place
- `docs/scenarios.md` -- worked end-to-end flows and the invariants they depend on
- `changes/<version>.md` -- one file per release, written before tagging
