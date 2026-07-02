# Token Enrollment and Identity Persistence

## Context

[ADR 0001](0001-directed-grants.md) makes access default deny: a grant names
a peer's key, and an ungranted peer gets nothing. It left two questions open:
how does a workload learn the operator's key, and how does the operator learn
the workload's -- without a manual key exchange per VM?

The driving case: boot a workload VM, reach its terminal from the laptop with
no per-VM step, while a stranger or sibling VM that dials in gets nothing.

## Decision

Two halves, one per direction of trust:

**The workload trusts the operator by key.** The operator daemon persists its
secret key (`--key`, default `~/.local/state/pai-sho/key`, mode 0600), so its
ticket is constant across restarts. A launcher can bake that one ticket into
every workload it boots: `pai-sho daemon -a <operator-ticket> ...`. Dialing
the ticket is the authentication -- iroh proves the remote holds the key.

**The operator trusts the workload by token.** `grant-token --label <name>`
mints a one-time secret, valid 5 minutes. The workload presents it on first
connect (`--enroll TOKEN`); the operator claims the token, pins the workload's
key under the label, and from then on the key alone is enough. Pins persist
next to the operator's key (`<key>.peers.json`), so a laptop restart does not
orphan enrolled workloads.

The worked flow:

```
# laptop, once
pai-sho ticket                        -> kL     (stable across restarts)
pai-sho grant-token --label rustdev   -> TOKEN  (one-time, 5 min)

# workload at boot (seeded with kL + TOKEN by the launcher)
pai-sho daemon -a kL -e 42000,7777 --enroll TOKEN
```

An unknown incoming peer gets a short window to present a token, then is
closed -- no announcement, no tunnel. Reused, expired, or unknown tokens are
refused; a claim is atomic, so a token can never admit two peers.

There is no enrollment ack. The workload re-presents its token on every
reconnect; once pinned, the operator ignores the message. This makes the
handshake idempotent instead of stateful -- if the first connect dies before
the claim, the retry enrolls; if it died after, the retry is a no-op.

## Tradeoffs

- Tokens live in operator daemon memory. A daemon restart voids unclaimed
  tokens; with a 5-minute TTL that window is small, and minting again is one
  command. Persisting them was not worth a secrets file.
- Workload keys are not persisted by default: a rebooted VM is a new key and
  its old token is spent. Launchers should seed a fresh token per boot, or
  give the workload a `--key` on disk that survives its reboots.
- Grants are in-memory, re-authored at startup (`-e` grants to the `-a`
  peers). Pins record who a peer is, not what it may reach; an operator
  restart keeps enrolled workloads known but drops manually authored grants
  (like sharing a laptop port with one workload). Persisting grants is a
  natural follow-up if that sting is felt.
- The label is trusted at mint time, not claim time: whoever holds the token
  becomes "rustdev". Tokens are 32 random bytes and short-lived; treat them
  like the secrets they are while in flight.
