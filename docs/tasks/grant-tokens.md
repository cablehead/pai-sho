# Task: Grant Tokens (enrollment)

Build the enrollment path from [ADR 0001](../adr/0001-directed-grants.md): how a
new peer becomes a *known, pinned* peer without anyone copying keys by hand.

## Where things stand

- Branch `adr/authorized-mesh` (PR #1) — **pull latest first**.
- Done: `list` emits JSON; protocol fields renamed (`me`, `key`, `online`,
  `they_expose`, `i_expose`, `peer`).
- Not this task: grant enforcement, stable key, phone-home. **Only build tokens.**

## The flow

You mint a one-time secret; the workload uses it to introduce itself, so no key is
ever typed or pasted.

```
laptop:    tok = pai-sho grant-token --label A     # minted, remembered, expires in 5 min
           (tok reaches the workload out-of-band — not this task's concern)
workload:  connects, presents tok
laptop:    tok valid & unexpired?  → pin the peer (its key + label "A"), consume tok
                                   → otherwise: don't enroll
```

Properties: **one-time** (consumed on use → replay-safe) and **TTL** (default
300s; an unclaimed token just expires, leaving nothing behind).

## Build

**CLI** — `main.rs`, `client.rs`
- `pai-sho grant-token [--label <s>] [--ttl <secs=300>]` → prints the token.
- `pai-sho list` gains a `pending` array: `[{ "label": ..., "expires": ... }]`.

**Daemon** — `daemon.rs`
- Hold pending tokens: `token → { label, expires_at }`.
- Mint on `grant-token`; reap expired (lazily, on access, is fine).
- On an incoming connection presenting a valid token: record the peer as known
  (key + label) and consume the token. Invalid / expired / absent → don't enroll.

**Protocol** — `protocol.rs`, `peer.rs`
- The connecting peer presents its token first — add a `PeerMessage` (e.g.
  `Enroll { token }`), validated in `handle_connection` against the pending set.
- A pinned peer carries its label so `list` can show it (add `name` to `PeerInfo`).

Rejecting *un-pinned* peers belongs to the authz item, not here — leaving today's
accept-behavior in place is fine; note whichever you choose.

## Done when

- `grant-token` mints a token; `list` shows it under `pending` with an expiry.
- A peer presenting a valid token appears under `peers` with its `name`.
- An expired or unknown token does not enroll; a reused token fails.
- `cargo check` is clean.

## House rules

- Control plane stays JSON in/out; keep the nice field names.
- Commit per stable change (`cargo check` passing) and push to this branch.
- No `Co-Authored-By` trailer — the repo's `.claude/settings.json` handles it; don't hand-write one.
- Docs/ADRs: reader-first, example-led, no wall of words.
