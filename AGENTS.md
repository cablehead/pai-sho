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

All text in the repo must be ASCII only. No smart quotes, emoji, or other
non-ASCII characters. Use plain quotes, and plain text markers like `WARNING:`
instead of emoji.

ASCII art and diagrams are a separate case. Dashes, pipes and arrows drawing a
picture are structure, not punctuation, and no writing rule applies to them.

No em-dashes, ASCII or otherwise. Not the character, and not `--` standing in for
one. Restructure with a period, colon, comma, or parentheses.

## Code Quality

Before committing:
1. `cargo fmt` - fix formatting
2. `cargo clippy` - fix lints
3. `cargo test` - run tests

## Writing style

Applies to READMEs, changelogs, ADRs, PR descriptions, commit messages, and code
comments.

Write it the way you would explain it to the reader across a table. Start from
their first question and let each sentence answer the one the last sentence
raised. Do not compose for the page: no opener that announces the section, no
closer that lands it, no short sentence placed where a paragraph turns.

Then make three passes, in this order.

1. Placement. For each sentence, is it there for what it says or for where it
   sits? Cut the second kind.
2. Truth. Every claim checks against the code at the tag, a PR, or an ADR.
   Appearing in a repo doc is not enough; two docs can be wrong the same way.
3. The reader. After each sentence, what do they know that they did not before?
   If nothing, cut it. Compressing for length is where this goes wrong: the
   clause that made the point is the one that gets cut. The reader is a
   stranger. They do not know you, and they close the tab at the first sentence
   that sounds generated.

This applies when editing, too. A paragraph assembled from phrases quoted out of
an ADR or a scenario doc can have every phrase correct and still not say
anything, and replacing one quoted phrase with another will not fix it. Work out
what the paragraph is trying to tell the reader and write that.

Headings say what the section answers. "Where this came from" withholds; "Why
the commands changed" tells.

Explain a decision as what was chosen and why. Do not narrate the moment of
choosing ("So we stopped adding to it. We wrote down what you actually type.").

The rules below name the shapes the page-composing habit produces. They catch
the shapes, not the habit.

- **Plain, not rhetorical.** No setup-then-reversal. State the point directly.
- **No trailing participial coda.** Do not tack a ", ...ing/...ed ..." clause onto
  a sentence that already finished its job.
- **Avoid the rule of three.** Balanced parallel triads are a tell. Use two, or an
  uneven list, or vary the clause shapes.
- **No redundant summary coda.** If the sentence made the point, stop.
- **No landing beats.** A paragraph that descends to a short punchy close, over
  and over, is the strongest tell there is. Do not end paragraphs on a reveal.
- **No em-dashes, ASCII or otherwise.** Not the character, and not `--` standing
  in for one. Restructure with a period, colon, comma, or parentheses.
- **No opaque jargon.** Name the actual thing.
- **No wasted words.** Each word earns its place.
- **Vary sentence length.** Cadenced balance reads machine-generated.
- **Invent nothing.** Every claim traces to the repo, the tests, or something the
  author said. No plausible-sounding history.

The common thread: de-cadence. Say each thing once, plainly, and let the
sentences be uneven.

## Key Concepts

- **Daemon**: Single iroh Endpoint with a stable key, manages all peers
- **Peer**: Remote daemon identified by EndpointId
- **Invitation**: `<key>.<code>`. The key says who to dial, the code admits you. One side runs `invite`, the other `accept`. `invite <key>` authorizes a key you already know and creates no secret. See docs/adr/0006
- **Expose**: Grant a specific TCP port to specific peer keys. Default deny: no grant, no access. `--to` or `--all` is required
- **Surface**: A peer's ports addressed as a unit at a dedicated local IP, under the name from `--as`, or a truncated key if nothing named it
- **Auto-project**: On its first announced port a peer is projected automatically: an address is allocated and its ports bind there, so reach is automatic. `project`/`unproject` are the override (pin an IP, rename, toggle off). See docs/adr/0004 and 0005
- **Resolver**: With `--resolver`, the daemon answers `<name>.pai-sho` from live surfaces (`vibenv-ndyg.pai-sho`). Authoritative for one suffix; never touches the system resolver

## Architecture

`src/core/` is pure: it decides, and it does no IO. `session.rs` owns admission
and authorization and returns `Action`s; `invite.rs` parses invitations. The
shell in `peer.rs` feeds it events and carries out what it returns.

Put decisions in `core`, keep IO in the shell. A security decision made inline in
`peer.rs` is unreachable from the unit tests, which is how `expose` shipped a
default-allow once already. See docs/adr/0007.

- `src/core/session.rs`: admission, grants, tunnel authorization (pure, unit tested)
- `src/core/grants.rs`: the `(port) -> grantees` table (ADR 0001)
- `src/core/invite.rs`: parsing `<key>.<code>`
- `src/peer.rs`: connections, dialing, retry, binding
- `src/daemon.rs`: control socket, request handling, state files
- `src/live_tests.rs`: two real daemons over loopback, relays disabled

## Where the design lives

- `docs/adr/`: the decisions and why. Append a new ADR that supersedes rather than editing one in place
- `docs/scenarios.md`: worked end-to-end flows and the invariants they depend on
- `changes/<version>.md`: one file per release, written before tagging
