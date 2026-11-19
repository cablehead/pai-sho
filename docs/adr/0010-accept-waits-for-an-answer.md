# Accept Waits for an Answer

Amends the retry tradeoff in [0006](0006-invitations.md): an accepter no
longer retries a code the inviter has refused. Amends
[0002](0002-token-enrollment.md): a code is presented until the inviter
answers, not on every reconnect, and a stale one is refused even from a key
the inviter knows.

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
inviter closes with "not authorized" and we presented a code. A bare key keeps
dialing, since the inviter may run `invite <key>` after we `accept`. An
"enroll timeout" keeps dialing too: it is the one refusal a retry can fix.
`core::accept` makes these calls; `peer.rs` asks it when a connection closes.

**A code is presented until it is answered, then never again.** The
inviter's first announce means it read the code, so the accepter drops it.
Until then, say while the inviter is offline, the code goes out on every dial.
A peer that has let us in reconnects with its key alone.

**A stale code is refused, even from a known key.** So a code arriving from a
key the inviter knows means that device just took up an invitation. A live
code grants its ports (0006). A used, expired or unknown one gets the key
refused with "not authorized" and forgotten, grants and all; a fresh
invitation brings it back. This covers a device that forgot the inviter and
scans an old invitation: before, the inviter let the known key straight back
in with everything it had. A known key that dials in has its announce held
until its first Enroll, at most 2 seconds, so a stale code is refused before
the device hears it was let in.

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
- Devices on 0.5.6 or older present their used code on every reconnect. An
  inviter with this change refuses and forgets them at their next reconnect,
  until they take a fresh invitation or update.
- If a first connection drops after the inviter read the code but before its
  announce arrived, the retry presents a used code and is refused. The
  accepter has already waited for an answer by then, so this is narrow.
- A known key that dials in waits up to 2 seconds for its ports when it sends
  no Enroll, as an older client may not.
- 15 seconds is one dial's worth (0008). An inviter that takes longer to
  answer gets `Pending`, and the accepter keeps dialing.
