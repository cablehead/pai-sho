# Scenarios

Worked end-to-end flows, written from the operator's side. Each one states what
has to be true, what travels between machines, and where the flow is rougher
than it needs to be.

The commands below are the **proposed** surface, not what ships today. Where a
scenario needs to show the difference, the current commands are marked as such.

## Proposed CLI

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
                                          one-time code (valid 5 minutes).
accept <invite|key> [--as <name>]         Take up an invitation.
forget <peer>                             Evict: close, unbind, revoke grants,
                                          drop the record.

expose <port> (--to <key>... | --all)     Grant a local port to named peers.
unexpose <port> [--to <key>]              Revoke; bare revokes every grant.

project <peer> [--ip <addr>] [--as <name>]  Override the automatic projection.
unproject <peer>                          Unbind ports, drop address and name.

list                                      Peers, grants, bindings, surfaces, and
                                          how each peer was admitted.
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

### The flow today

Two values have to travel, and the flow does not actually complete:

```sh
# build box
pai-sho ticket
# 5hc4bjqf...
pai-sho grant-token --label andy-laptop
# 7fd25613...
```

Both go to me out of band. Then, on my laptop:

```sh
pai-sho add-peer 5hc4bjqf...
```

This does not work. `add-peer` cannot present a token: `--enroll` exists only on
`pai-sho daemon`, so a laptop with a daemon already running has no way to use
the token it was just handed. The options are to restart the laptop's daemon,
which is not a reasonable ask when it is already serving other peers:

```sh
pai-sho daemon -a 5hc4bjqf... --enroll 7fd25613...
```

or to skip tokens entirely and have both sides pin each other by key:

```sh
# build box
pai-sho pin <laptop-key> --label andy-laptop
pai-sho expose 8080 --to <laptop-key>

# laptop
pai-sho add-peer <buildbox-key>
```

### The friction

- **A token is unusable by a running daemon.** `--enroll` is a daemon flag, so
  the only way to present one is at startup.
- **Two values travel for one handshake.** The key says who to dial, the token
  proves I may. They are always used together, so they should be one value.
- **The grant is a separate step, easy to forget.** Steps 1 and 3 both happen on
  the build box and express one intention: "andy's laptop may see the
  dashboard." Split across two commands, the common failure is a peer that
  connects fine and sees nothing, which reads like a bug rather than a missing
  grant.
- **The inviter has to name the claimer.** `grant-token --label andy-laptop`
  asks the build box to pick a name it will never type. The name matters to me,
  because I am the one who will type it in a URL.
- **Nothing says the link is mutual.** `add-peer` reads like it completes the
  job, and the failure when it does not is a log line on the other machine.

### Proposed flow

```sh
# build box: hi, be friends, and you may have 8080
pai-sho invite --expose 8080
# psi1qy3f8k...   one-time, valid 5 minutes

# laptop: yeah, be friends, and I will call you buildbox
pai-sho accept psi1qy3f8k... --as buildbox
```

```sh
curl http://buildbox.pai-sho:8080
```

Two commands, one per machine, one value between them.

- **The invite embeds the issuer's key**, so it is self-contained: it says who
  to dial and proves I may. This is what `ticket` should have been. The current
  `ticket()` is `endpoint.id().to_string()` with a `TODO: proper ticket
  serialization` above it, and this is the serialization it was waiting for.
  `key` stays as the bare identifier for the host-attested path, where a public
  key is exactly what you want to move.
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
# psi1qy3f8k...
```

The code goes into the VM's boot config, and its daemon says yes on startup:

```sh
# vm
pai-sho daemon --accept psi1qy3f8k... -e 3001,7331
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
