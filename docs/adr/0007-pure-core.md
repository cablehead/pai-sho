# A Pure Core for Admission and Authorization

## Context

[0001](0001-directed-grants.md) makes access default deny, and the whole security
model rests on two decisions: whether to admit a connection, and whether to open
a tunnel for a port. Both lived inline in `peer.rs`, interleaved with dialing,
retry, and binding.

That placement made them untestable in practice. Reaching either one meant two
live iroh endpoints, a relay, and a real TCP listener, so the suite tested the
transport and left the decisions uncovered. The cost showed up as a shipped bug:
bare `expose <port>` granted to every known peer, a default-allow that no test
could have caught because no test could reach the grant check without standing up
a network.

[Sans-io](https://fasterthanli.me/articles/the-case-for-sans-io) names the
pattern: a state machine that consumes events and emits actions, with all IO in a
thin shell.

## Decision

**`src/core/` is pure. It decides, and it does no IO.**

`session.rs` owns the peer table, grants, and pending enrollments. It takes
events and returns `Action`s for the shell to carry out:

```rust
pub enum Action {
    Admit { conn: ConnId, peer: EndpointId, replacing: bool },
    Refuse { conn: ConnId, reason: Refusal },
    Announce { peer: EndpointId, ports: Vec<u16> },
    ApplyPorts { peer: EndpointId, ports: Vec<u16> },
    ServeTunnel { port: u16 },
    RejectTunnel { reason: Refusal },
    PersistPin { key: EndpointId, label: Option<String> },
    DropPin { key: EndpointId },
}
```

The security perimeter is one function with no IO in it:

```rust
pub fn on_tunnel(&self, peer: &EndpointId, port: u16) -> Action {
    if self.grants.allows(port, peer) {
        Action::ServeTunnel { port }
    } else {
        Action::RejectTunnel { reason: Refusal::NotGranted }
    }
}
```

`peer.rs` keeps the connections, dialing, retry, and binding, and routes its four
decision points through the core. `invite.rs` parses invitations, also pure.

**Tests split to match.** Unit tests in `core` cover the decisions and run in
microseconds. `live_tests.rs` runs two real daemons over loopback with relays
disabled and `MemoryLookup` seeded from `bound_sockets()`, covering what only a
real connection can: dialing, reconnection, binding, eviction.

## Tradeoffs

- **A lock around the session.** The shell holds `Arc<Mutex<Session>>` and the
  core is synchronous, so a decision must not span an await. That is a real
  constraint on future work, and it is the price of keeping the core free of
  async.
- **Actions are a second vocabulary.** Adding behavior means adding a variant and
  handling it in the shell, which is more ceremony than calling the thing
  directly. Worth it where the decision is a security decision, and not obviously
  worth it elsewhere.
- **Purity is a convention, not enforced.** Nothing stops someone adding IO to
  `core`. The module doc says so and review has to hold the line.

## What it bought

Two bugs surfaced immediately once the seam existed: `add_peer` dialed before
recording the peer, so a failed dial left nothing to retry, and `bind_one`
dropped failed binds on the theory the next announce would retry, but a peer with
a stable port set never re-announces.

Mutation testing confirms the tests have teeth. Replacing the grant check with
`if true` fails five tests; setting the bind retry count to one fails the retry
test.
