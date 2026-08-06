# Auto-project and the owned resolver

Extends [0004](0004-peer-surfaces.md).

## Context

0004 made surfaces explicit: a peer bound nothing until you ran `project`. That
killed cross-peer port collisions and gave reach a name, but it turned the old
"an exposed port just shows up on localhost" into a manual step per peer. That is
a regression for the common case, where you want every enrolled workload
reachable without ceremony.

Two facts change the tradeoff:

- On Linux, claiming a per-peer address is free. All of `127.0.0.0/8` routes to
  `lo`, so a listener binds any `127.x` with no setup and no privilege (verified
  on the target VMs). The friction that motivated explicit projection was
  macOS-only (extra `lo0` addresses need an alias) plus the `/etc/hosts` write.
- Encoding a workload's identity in a unique port number is worse than encoding
  it in a name. `vibenv-ndyg.pai-sho:42200` and `vibenv-goo-test.pai-sho:42200` reading the
  same role port, disambiguated by name, beats a scarce per-VM port.

## Decision

**Auto-project by default.** When a peer announces its first granted port and has
no surface, it is projected automatically: an address is allocated, the surface is
named after the peer's enrollment label, and the ports bind there. Reach is
automatic again. `project` / `unproject` remain as the override: pin a specific
address, rename, or toggle a peer off.

**Names come from an owned resolver, not `/etc/hosts`.** The daemon serves a small
authoritative resolver from the live surface table: `<label>.pai-sho` resolves to that
surface's address and stops resolving when it goes away. This replaces the
`/etc/hosts` writing from 0004, which needed root on every change and could not
run as the unprivileged `app` user inside a VM.

The resolver is authoritative for `.pai-sho` and **nothing else**. It never recurses or
forwards; a query outside `.pai-sho` gets an empty answer. dnsmasq stays the front door
in the VMs, splitting `.pai-sho` to this resolver and sending everything else upstream.
Keeping the resolver `.pai-sho`-only is deliberate: a bug or compromise in it can only
affect `.pai-sho` name resolution, not all DNS on the box. pai-sho never touches
`/etc/resolv.conf`; dnsmasq owns that.

**The resolver listens on a fixed owned address, at port 53 under TUN.** Once the
daemon owns a TUN and its subnet, the resolver answers at a reserved address on
that subnet, port 53, the way Tailscale's MagicDNS answers at `100.100.100.100:53`.
That is not a host `bind()`: the daemon reads the query off the tun fd and replies
in its userspace stack, so it needs no `CAP_NET_BIND_SERVICE` and cannot collide
with any other listener. It rides the one `CAP_NET_ADMIN` the tun already costs.
This drops the port suffix everywhere downstream:

- Linux dnsmasq: `server=/pai-sho/<owned-ip>` (no `#5353`).
- macOS: `/etc/resolver/pai-sho` with `nameserver <owned-ip>` (no `port` line).

Before the TUN backend exists, the interim resolver is a real UDP socket on
`127.0.0.1:5353` (a high port, so still no privilege), bridged by dnsmasq
`server=/pai-sho/127.0.0.1#5353`. The `--resolver <addr>` flag is the same either way;
only the address and whether the answer comes from a real socket or the stack
change.

**The address backend is pluggable, TUN is the target.** Today the address is a
loopback IP, which is free on Linux and needs an `ifconfig lo0 alias` on macOS.
The intended backend is a daemon-owned TUN device that claims a whole subnet in
one privileged step at startup, after which every per-peer address is free on both
platforms. On Linux that needs a guest kernel with `CONFIG_TUN` and `CAP_NET_ADMIN`
(the current VM kernel has neither, so this is gated on a kernel rebuild). Loopback
stays as the fallback when `/dev/net/tun` is absent, so a no-TUN VM still works.

## Consequences

- The 0004 behavior change is undone: enrolled peers are reachable without a manual
  `project`. The explicit command is now an override, not a requirement.
- Per-VM unique ports stop being necessary. The `42xxx` number stays the VM's host
  identity, but the pai-sho-exposed role port can be constant across VMs, since the
  name and address disambiguate. That simplification lives in the provisioning
  stack, not here.
- pai-sho stays a good DNS citizen: authoritative for `.pai-sho` only, never recursing
  and never seizing `/etc/resolv.conf`. dnsmasq remains in the VMs as the front
  door, and wiring it to the resolver is the provisioning stack's job.

## Suffix: why `.pai-sho`

The suffix is `.pai-sho`, not the bare `.ps` we first reached for. `.ps` is the
live Palestinian ccTLD, so it would shadow a real public TLD on any box pointed
at the resolver (genuine `*.ps` names unreachable, internal names leaking on a
split misfire). `.pai-sho` is an invented single-label suffix: not a current TLD,
and a hyphenated brand string ICANN is not going to delegate, so the collision
risk is practically nil. It is not the permanent guarantee `.internal` (ICANN's
reserved private-use TLD) gives; we took the short branded form over
`pai-sho.internal` for readability, accepting "no one will register it" instead
of "can never be registered." The mesh is internal-only, names resolve solely
through our own resolver, dnsmasq, and `/etc/hosts`, never public DNS, so even a
future collision would affect only us and we would rename. Unlike Tailscale's
`.ts.net` we register and run no domain; public TLS stays on `*.cross.stream` via
caddy, and `.pai-sho` names intentionally cannot get public-CA certs.

## Tradeoffs

- **macOS `/etc/resolver` for custom suffixes.** There were reports of a macOS 26
  regression breaking `/etc/resolver/<domain>` for non-public TLDs. Verified
  working on the fleet's build mini (macOS 26.4.1): `/etc/resolver/pai-sho` ->
  `nameserver 10.99.0.53` resolves `*.pai-sho` system-wide, and `curl
  vibenv-goo.pai-sho:9000` reached a peer over the utun by name. If a future
  macOS version does break it, the fallback is a local forwarder (dnsmasq-style)
  rather than `/etc/resolver`.
- **Name integrity rests on the enrollment label.** `<label>.pai-sho` is
  trustworthy only because the operator mints the label into the token, not the
  peer. If a peer could set its own label it could claim another surface's name.
