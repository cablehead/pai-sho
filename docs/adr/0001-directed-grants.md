# Directed Grants

## Context

pai-sho forwards ports between peers, but today it's all-or-nothing: once a peer
connects, it can reach any port a node forwards. There's no way to say "this
port, only for that peer."

We need per-peer control. Running one laptop plus many untrusted workloads, we
want: the laptop can open each workload's terminal; a workload cannot reach the
laptop's other ports; and two workloads can never see each other.

## Decision

A **grant** is the unit of authorization:

```
(owner, port) -> grantee
```

Read: *owner* exposes *port* to *grantee*, and to no one else. Default deny -- no
grant, no access.

### Worked example

Three nodes: your laptop **L** and two workloads **A**, **B**, with keys
`kL, kA, kB`. You want to:

- open each workload's terminal from the laptop,
- see a web app running on A,
- share a scratch server on your laptop with A only,
- keep A and B blind to each other.

That's four grants:

```
(A, 4000) -> kL      A's terminal      -> laptop
(A, 3000) -> kL      A's web app       -> laptop
(B, 4001) -> kL      B's terminal      -> laptop
(L, 7331) -> kA      laptop's server   -> A only
```

Which gives exactly:

```
             A:4000  A:3000  B:4001  L:7331
  L (you)      y       y       y       -
  A            -       -       n       y
  B            n       n       -       n

  y = reachable   n = denied   - = own port (n/a)
```

No grant names `kB`, so B reaches nothing but its own ports. A can reach `L:7331`
because it was granted; B cannot, because it wasn't. A and B never appear in each
other's grants, so they're mutually invisible.

## Enforcement

When a peer connects, the owner checks its key against the grants. iroh gives the
key cryptographically as `conn.remote_id()` -- the peer proved it holds the private
half, so it can't be faked. Ungranted peer: refuse, announce nothing, forward
nothing. A tunnel is opened only to a port granted to that specific peer.

The credential is therefore the grantee's **private key**, not a shareable
address -- you can't hand someone access by leaking a string.

## Tradeoffs

- Grants have to be authored. In the laptop-hub case the common ones are set when
  a workload enrolls; cross-workload exceptions (like `L:7331 -> A`) are added by
  hand.
- A grant is only as trustworthy as knowing the grantee's real key. We take it
  from the connection itself, never from something typed.

Identity persistence and enrollment (how a workload gets `kL` and how the laptop
learns `kA`) are separate concerns -- see forthcoming ADRs.
