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
  it in a name. `vibenv-ndyg.ps:42200` and `vibenv-goo-test.ps:42200` reading the
  same role port, disambiguated by name, beats a scarce per-VM port.

## Decision

**Auto-project by default.** When a peer announces its first granted port and has
no surface, it is projected automatically: an address is allocated, the surface is
named after the peer's enrollment label, and the ports bind there. Reach is
automatic again. `project` / `unproject` remain as the override: pin a specific
address, rename, or toggle a peer off.

**Names come from an owned resolver, not `/etc/hosts`.** The daemon serves a small
authoritative resolver for one suffix (`.ps`) from the live surface table:
`<label>.ps` resolves to that surface's address and stops resolving when it goes
away. This replaces the `/etc/hosts` writing from 0004, which needed root on every
change and could not run as the unprivileged `app` user inside a VM. The resolver
is a plain UDP listener on a high port, so it needs no privilege. The OS is wired
to send only `.ps` to it: `/etc/resolver/ps` on macOS, and a dnsmasq
`server=/ps/127.0.0.1#5353` forward on Linux (the provisioning stack owns this,
and pai-sho never touches the global resolver itself).

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
- pai-sho is a good DNS citizen: it answers one suffix on its own socket and never
  seizes `/etc/resolv.conf`. Wiring the OS to it is the operator's explicit step.
