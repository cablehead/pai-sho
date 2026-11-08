# A Peer Says Goodbye Before It Sleeps

## Context

iOS suspends the iPhone app soon after it goes to the background. The link to
the Mac is not closed. It goes quiet until it times out, and then, per
[0008](0008-both-ends-dial.md), the Mac redials the phone. A suspended phone
cannot answer, so the Mac dials it every 10 seconds for as long as the phone is
in someone's pocket.

## Decision

**On an explicit `Sleep` request, a daemon sends `PeerMessage::Goodbye` to every
connected peer.** The message means "I am going to sleep. Do not dial me. I will
call you." The iOS app sends `Sleep` when it goes to the background, inside a
short background task, and then stops the process. When it comes back it starts
the process again, which dials its saved peers.

**A peer that reads a goodbye marks the sender asleep and stops dialing it.** It
closes the connection itself after marking, so its connection loop sees the
close and waits instead of redialing. The next inbound connection from that
peer clears asleep. `list` shows `asleep: true` for such a peer, and the apps
show "asleep" in place of "offline". `core::dial::Reach` holds the decision;
`peer.rs` asks it before each dial.

**The sender waits for those closes, at most 800 ms, then closes its own end.**
Peers are told in parallel, so the request returns within that bound. After
`Sleep` the daemon dials no one until it is restarted.

**Goodbye promises a call, so the sender pins every peer it told to be dialed
after a restart.** A daemon that restarts dials only the peers whose pin says to.
Without this, a phone that had invited the Mac would come back waiting for the
Mac's call, while the Mac waits for the phone's.

**A daemon that stops or restarts says nothing.** `pai_sho_stop`, SIGTERM and a
restart behave as before. The reason is the same deadlock. A daemon loads every
peer it invited as one to wait for, so after a restart it dials none of them. If
a Mac said goodbye on restart, the phone would stop dialing it, and the
restarted Mac would never dial the phone.

## Tradeoffs

- **Asleep lives in memory.** If the side that was told goodbye restarts, it
  forgets, and dials the sleeping peer again if its pin says to.
- **A peer that never wakes is never dialed.** If the phone is wiped, or the app
  is deleted, the Mac waits for a call that never comes. `forget` is the way out.
- **An older daemon cannot parse `Goodbye`.** It logs the parse error and drops
  the message, as it does any message it does not know, and keeps dialing. A
  sender whose peer is that old waits the full 800 ms for a close that does not
  come.
