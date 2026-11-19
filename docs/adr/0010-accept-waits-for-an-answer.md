# Accept Waits for an Answer

Amends the retry tradeoff in [0006](0006-invitations.md): an accepter no
longer retries a code the inviter has refused.

## Context

`accept` printed `OK` as soon as the daemon had saved the peer. Whether the
inviter let you in was something you found out later, by watching `list`.

An invitation's code works once, for 5 minutes, and lives only in the
inviter's memory. A visitor to the pai-sho.ndyg.cross.stream demo scanned one
that was no longer live. The demo refused it with "not authorized" on every
dial, and the phone redialed it every 10 seconds with the same code. The app
showed the demo flickering between online and connecting, with no ports and no
word of why. On the demo's side there was a log line, and only with
`RUST_LOG=info`.

## Decision

**`accept` waits for the inviter's answer, up to 15 seconds.** The control
request carries `wait: true`, and the daemon replies `Accepted`, one of:

- `Joined { name, ports }`: the inviter let us in. An inviter announces its
  ports the moment it admits a peer, even when there are none, so the first
  announce is the answer.
- `Refused { reason }`: the inviter refused the code. Nothing is kept: the
  peer is unpinned and forgotten on this side.
- `Pending { turned_away }`: no answer in time. The peer stays saved and is
  dialed with backoff, as before. That is the inviter being offline, or, with
  `turned_away` set, a peer named by key alone that has not invited ours yet.

A request without `wait` answers `Ok` at once, so an older CLI or app still
works against this daemon. `daemon --accept` does not wait, because a VM
booting before its inviter is up is the normal case there.

**A refused code ends the attempt.** The accepter drops the peer when the
inviter closes with "not authorized", we presented a code, and the inviter has
never let us in. A bare key keeps dialing, since the inviter may run
`invite <key>` after we `accept`. An "enroll timeout" keeps dialing too: it is
the one refusal a retry can fix. A peer that has let us in before is never
dropped this way. `core::accept` makes these calls; `peer.rs` asks it when a
connection closes.

**The inviter keeps the keys it turned away.** Each refusal is recorded with
its reason, a count, and the time of the last one, for an hour, at most 50
keys. `list` shows them as `refused`. A key that later gets in drops off. An
operator who recognizes a key can admit it with `invite <key>`.

## Tradeoffs

- A dead code now fails loudly instead of quietly waiting. If an inviter is
  restarted while a visitor is between scanning and connecting, the visitor's
  live-looking code is refused, and they need a new invitation where before
  they would have flickered forever. That is the point.
- The refusal list is in memory and lost on restart, like the codes it is
  about.
- 15 seconds is one dial's worth (0008). An inviter that takes longer to
  answer gets `Pending`, and the accepter keeps dialing.
