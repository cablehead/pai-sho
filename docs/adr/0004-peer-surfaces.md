# Peer Surfaces

> **Partly superseded.** [0005](0005-auto-project-and-owned-resolver.md) makes
> projection automatic and replaces the `/etc/hosts` handle below with an owned
> resolver. The `surfaces` command is gone, folded into `list`, and "enrollment
> label" is now the name from `--as`, or a truncated key. What stands: a surface
> is a peer's ports at their own address, and `project` / `unproject` control it.

## Context

A peer exposes ports to us; we reach them on `localhost`. Today every peer's
ports bind to a single hardcoded `127.0.0.1` (`tunnel.rs`). Two consequences
follow:

- **Ports collide across peers.** If peer A and peer B both expose `7676`, only
  one can bind `127.0.0.1:7676`. `update_peer_ports` carries a whole arbitration
  dance for this: probe the current holder, evict the loser, retry the bind. It
  exists only because everyone shares one address.
- **Reach is implicit and unnamed.** A port shows up on `localhost` the moment a
  peer announces it. You address it by a bare number and cannot tell which peer's
  `:7676` you are talking to.

`CLAUDE.md` already described the model we want at the time ("Peer: assigned
local `127.0.0.x` IP", "Auto-bind: bind `<peer-ip>:<port>`"), but the code never
implemented it.

## Decision

A **surface** is a peer's set of granted ports, addressed as a unit at a
dedicated local IP.

You **project** a surface to make it reachable, and **unproject** to take it
down. A peer that is not projected binds nothing: its ports are known but have no
local listener. Reach is explicit.

```
pai-sho project <peer> [--ip 127.0.1.2] [--as broker]
pai-sho unproject <peer>
pai-sho surfaces
```

- `--ip` picks the address; omit it and one is allocated from `127.0.1.0/24`.
- `--as` gives the surface a DNS handle in `/etc/hosts`, so `broker:7676` resolves.
- `<peer>` is a key or an enrollment label.

Once projected, every port granted to that peer binds `IP:port`. New grants bind
under the same IP; revoked grants unbind. The IP is the stable handle; the ports
under it come and go.

Projections persist next to the key (`<key>.surfaces.json`). On restart the
daemon re-applies each one, so a projected surface keeps its IP and name across a
reboot.

## The local-address mechanics differ by OS

Binding a listener to the stock `127.0.0.1` needs no privilege anywhere. Giving a
surface its *own* address is where the two platforms split:

- **Linux:** all of `127.0.0.0/8` already routes to `lo`, so a listener can bind
  `127.0.1.2` with no setup and no root. An address with a DNS name still needs a
  `/etc/hosts` line, which needs root; without a name, projection is unprivileged.
- **macOS:** `lo0` has only `127.0.0.1` configured. Any other address must be
  added with `ifconfig lo0 alias <ip> up`, which needs root. `127.0.0.1` itself is
  always available with no privilege.

Both cases route through one seam (`surface.rs`): the interface alias is
OS-specific, the `/etc/hosts` handle is shared.

## What this retires

With a unique IP per surface, two peers never contend for one address. The
collision arbitration in `update_peer_ports` (`find_holder`, `probe_peer`, the
evict-on-collision branch, the same check in `handle_enrollment`) has nothing
left to resolve and is removed. Binding becomes: project assigns the IP, announces
bind under it.

## Tradeoffs

- **Reach is no longer automatic.** Before, an exposed port appeared on
  `localhost` on its own. Now you run `project` once per peer. This is the point
  (explicit, named, toggleable reach), but it is a behavior change: existing
  setups add a `project` call per peer they drive.
- **Named surfaces need root** for the `/etc/hosts` edit, and non-`127.0.0.1`
  addresses need root on macOS. Projecting to `127.0.0.1` with no name stays
  fully unprivileged, which keeps the single-peer case friction-free.
