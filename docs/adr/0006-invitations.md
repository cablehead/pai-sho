# Invitations

Supersedes the command surface of [0002](0002-token-enrollment.md) and
[0003](0003-host-attested-enrollment.md). Their decisions stand; their names do
not.

## Context

0002 and 0003 settled how trust is established. Neither settled what the user
types, and what shipped had five commands doing pieces of one job: `ticket`,
`grant-token`, `add-peer`, `pin`, `remove-peer`.

Trying to reach a shared build box from a laptop that was already running a
daemon exposed the shape of the problem (docs/scenarios.md records the flow).
Two values had to travel out of band, a key and a token, and they were always
used together. `add-peer` could not present a token; `--enroll` existed only on
`pai-sho daemon`, so a running daemon had to be restarted to admit one peer. The
grant was a third step on the same machine as the first, so the common failure
was a peer that connected cleanly and saw nothing.

Underneath the naming, one thing was unstated: a link is mutual, and none of the
names said so. `add-peer` read like it finished the job.

## Decision

**Two commands, named for the two halves of one handshake.**

```
invite [<key>] [--as <name>] [--expose <port>...]
accept <invite|key> [--as <name>]
```

`invite` extends: hi, be friends. `accept` takes it up: yeah, be friends.
Neither works alone. A dial from a peer that has not invited you is refused.

**An invitation is one value**, `<key>.<code>`: who to dial, and the proof you
may. The separator is `.` because keys print base32 and codes print hex, so
neither contains it. This is what `ticket` should have been; `ticket()` was
`endpoint.id().to_string()` under a `TODO: proper ticket serialization`.

**`accept` takes an invitation or a bare key.** The two paths differ only in
what the launcher can safely put on a kernel cmdline. `invite <key>` is 0003's
host-attested path: it authorizes that key alone and creates no secret.

**A grant always names its grantees.** `expose <port>` used to grant to every
known peer, a default-allow inside a system whose first ADR opens with default
deny. `--to <key>` or `--all` is now required, and `--all` means every peer known
at that moment, never a standing rule.

**`--expose` on the invitation** attaches the grant to the friendship that
justifies it, so the two steps that happen on the same machine are one command.
The grantee key is filled in on acceptance instead of typed twice.

**Naming is local.** Each side names the other with `--as`, for its own use,
whenever it likes. A name is what you type in a URL, so it belongs to the machine
doing the typing. Neither end has to know what the other calls it. With no
`--as`, a peer gets a truncated key, renameable later with `project --as`.

**`forget <peer>`** replaces `remove-peer`: close it, unbind its ports, revoke
its grants, drop the record. Distinct from `unproject`, which only takes the
surface down and leaves the peer admitted.

`list` records how each peer arrived, by code or by key, because the two carry
different weight when auditing who is on your network.

## Tradeoffs

- **Breaking.** `ticket`, `grant-token`, `pin`, `add-peer`, `remove-peer`, and
  `surfaces` are gone, and `expose` refuses to run without a grantee. There is no
  deprecation window; pre-1.0, the minor version is the signal.
- **The inviter's daemon must be running when the accepter says yes.** Neither
  side needs an open inbound port, but the wait is real. The accepter retries
  with backoff until the invitation lands, so ordering does not matter.
- **`invite` with no key still mints a bearer secret**, with the leak surface
  0003 describes. It stays because it is the only path when you cannot learn the
  peer's key in advance. `invite <key>` is the better option whenever you can.
- **`--all` reads like a standing rule and is not.** The name is short and the
  behavior is a snapshot. The help text carries the correction, which is weaker
  than a name that could not be misread.
