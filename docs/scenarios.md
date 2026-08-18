# Scenarios

Worked end-to-end flows, written from the operator's side. Each one states what
has to be true, what travels between machines, and where the flow is rougher
than it needs to be.

## The CLI

```
pai-sho [--socket <path>] <command>

daemon [--accept <invite|key>]... [-e <port>,...]
       [--host <ip>] [--key <path>] [--name <n>]
       [--tun <dev>] [--resolver <addr>]
       [--socket-owner <user>] [--socket-mode <octal>]

key                                       Print this daemon's key

invite [<key>] [--as <name>] [--expose <port>...]
                                          Extend an invitation. With a key, to
                                          that key alone. Without one, prints a
                                          one-time invitation (valid 5 minutes).
accept <invite|key> [--as <name>]         Take up an invitation.
forget <peer>                             Evict: close, unbind, revoke grants,
                                          drop the record.

expose <port> (--to <key>... | --all)     Grant a local port to named peers.
unexpose <port> [--to <key>]              Revoke; bare revokes every grant.

project <peer> [--ip <addr>] [--as <name>]  Override the automatic projection.
unproject <peer>                          Unbind ports, drop address and name.

list                                      Peers, their grants, where their ports
                                          are bound, and how each was admitted.
```

Five things are worth stating once, because every scenario below depends on
them.

**A link is mutual.** `invite` and `accept` are the two halves of one handshake:
hi, be friends, and yeah, be friends. Neither works alone. A dial from a peer
that has not invited you is refused with `not authorized`, logged on the
receiving machine, not the dialing one.

**The accepter dials, the inviter waits.** Neither machine needs an open inbound
port, but the inviter's daemon has to be running when the accepter says yes. If
it is not, the accepter retries with backoff until it is.

**An invitation is not access.** Being someone's peer lets you talk to their
daemon. It does not entitle you to a single port. That is `expose --to`, always
naming a key. There is no ambient grant: `--all` means every peer known at that
moment, not a standing rule for peers admitted later.

**Naming is local.** Each side names the other for its own use, whenever it
likes. A name is what you type in a URL, so it belongs to the machine doing the
typing. Neither end has to agree with, or even know, what the other calls it. If
neither side passes `--as`, a peer gets a truncated key as its name, renameable
later with `project --as`.

**A peer arrives one of two ways, and `list` says which.** By code, trusted
because it held a one-time secret. By key, trusted because something vouched for
that exact key. The two carry different weight when you are auditing who is on
your network, so the record keeps them apart.

## A shared build box

A team runs a long-lived build box. It serves a dashboard on `8080`. I want to
reach it from my laptop, and my laptop is already running a daemon of its own
for unrelated work.

This is not the README's case. Both machines are long-lived, both already have
an identity, and neither is a fresh workload booting into an operator's network.

### What has to be true

Three separate facts, and missing any one of them fails in its own way:

1. The build box has invited my laptop. Missing, my dial is refused with
   `not authorized` in the build box's log and nothing in mine.
2. My laptop has accepted. Missing, their dial is refused.
3. The build box grants `8080` to my laptop. Missing, we connect cleanly and I
   am announced no ports, so the surface comes up empty.

### The flow before this

Two values had to travel, and the flow did not complete:

```sh
# build box
pai-sho ticket
# 5hc4bjqf...
pai-sho grant-token --label andy-laptop
# 7fd25613...
```

Both went to me out of band. Then, on my laptop, `pai-sho add-peer 5hc4bjqf...`,
which did not work: `add-peer` could not present a token. `--enroll` existed
only on `pai-sho daemon`, so a laptop already running a daemon had no way to use
the token it had just been handed. The choice was to restart the laptop's
daemon, or to abandon tokens and have both sides pin each other by key.

What was wrong with it:

- **A token was unusable by a running daemon.** Restarting a daemon to add one
  peer is not reasonable when it is already serving others.
- **Two values travelled for one handshake.** The key said who to dial, the
  token proved I may. They were always used together.
- **The grant was a separate step.** Steps 1 and 3 both happen on the build box
  and express one intention. Split apart, the common failure was a peer that
  connected fine and saw nothing, which reads like a bug.
- **The inviter had to name the claimer.** `grant-token --label andy-laptop`
  asked the build box to pick a name it would never type.
- **Nothing said the link was mutual.** `add-peer` read like it finished the
  job, and the failure when it did not was a log line on the other machine.

### The flow now

```sh
# build box: hi, be friends, and you may have 8080
pai-sho invite --expose 8080
# 5hc4bjqfp6booceusm3jrfebbegyfi6aiqwbgx4xxqmpvg5usoyq.7fd25613dd5e17cb...
# one-time, valid 5 minutes

# laptop: yeah, be friends, and I will call you buildbox
pai-sho accept 5hc4bjqfp6...7fd25613dd... --as buildbox
```

```sh
curl http://buildbox.pai-sho:8080
```

Two commands, one per machine, one value between them.

- **The invitation is `<key>.<code>`**, so it is self-contained: it says who to
  dial and proves I may. This is what `ticket` should have been. `ticket()` was
  `endpoint.id().to_string()` under a `TODO: proper ticket serialization`, and
  the word is gone: a bare key is just a key, which is what the host-attested
  path wants to move.
- **`--expose` on the invitation** attaches the grant to the friendship that
  justifies it. Still default deny, still directed at one key; the key is filled
  in on acceptance instead of typed twice.
- **The build box never names me.** It does not care what I am called. If it
  wants a name for its own listing it can pass `--as`, but nothing forces it.
- **Accepting is what makes the link mutual**, in one step. There is no third
  command and no second value.

## A laptop boots a vibenv

The case pai-sho was built for. I boot a VM per task, a
[vibenv](https://github.com/cablehead/vibenv.dag), with no inbound ports. It
runs an app on `3001` and a live-reload server on `7331`, and it dials home.

The roles are the reverse of the build box: here the inviter is the consumer. I
extend the invitation and I am the one who will type the name, so `--as` belongs
on my end.

### With an invitation

```sh
# laptop
pai-sho invite --as vibenv-ndyg
# 5hc4bjqfp6...7fd25613dd...
```

The code goes into the VM's boot config, and its daemon says yes on startup:

```sh
# vm
pai-sho daemon --accept 5hc4bjqfp6...7fd25613dd... -e 3001,7331
```

`-e` grants those ports to the peers named by `--accept`, and to no one else. On
acceptance the VM is projected automatically: it gets an address on my private
network and its ports bind there under the name I chose.

```sh
curl http://vibenv-ndyg.pai-sho:3001
open  http://vibenv-ndyg.pai-sho:7331
```

Expose something new later and it appears under the same name, no new value
exchanged:

```sh
# vm
http-nu :3002 -c '{|req| "hello from a new experiment"}'
pai-sho expose 3002 --to <laptop-key>
```

### Without a secret in the guest

An invitation code is a bearer secret, and as boot config moves onto the kernel
cmdline it would sit in `/proc/cmdline`, readable by every process in the guest,
and in the host's `ps` and journal.
[ADR 0003](adr/0003-host-attested-enrollment.md) takes the other path: the
workload generates its own key and the host vouches for it.

The VM generates and persists a keypair at boot, reports its **public** key to
the host over vsock, and the launcher relays it to me over the link it already
holds:

```sh
# laptop, told by the launcher: "this VM's key is kW"
pai-sho invite kW --as vibenv-ndyg
```

```sh
# vm: nothing secret on the cmdline
pai-sho daemon --accept <laptop-key> -e 3001,7331 --key /var/lib/vibenv/key
```

An invitation addressed to a key needs no code, so both values on the VM's
cmdline are public: my key and its own key path. There is no secret to leak and
no race, because I only ever accept the one key the host named.

The ordering does not matter. If the VM says yes before I have invited it, it is
refused and retries with backoff, and the link establishes as soon as the
`invite` lands.

This is where the union in `accept <invite|key>` pays off. The two paths differ
only in what the launcher can safely put on a cmdline; the VM's command is
otherwise identical.
