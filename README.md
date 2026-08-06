<p align="center">
  <img alt="A forest spirit placing a tile on a pai sho board" width="380" src="docs/assets/pai-sho.png">
</p>

<h1 align="center">pai-sho</h1>

<p align="center">
  Encrypted peer-to-peer port forwarding for machines with no open ports.<br>
  Default deny -- every port granted to exactly the peer you choose.
</p>

<p align="center">
  <a href="https://github.com/cablehead/pai-sho/actions/workflows/ci.yml">
    <img src="https://github.com/cablehead/pai-sho/actions/workflows/ci.yml/badge.svg" alt="CI">
  </a>
  <a href="https://crates.io/crates/pai-sho">
    <img src="https://img.shields.io/crates/v/pai-sho.svg" alt="Crates">
  </a>
</p>

pai-sho forwards specific TCP ports between your machines over an encrypted,
peer-to-peer QUIC connection (built on [iroh](https://github.com/n0-computer/iroh)).
Neither machine needs an open inbound port, a public IP, or a relay you run --
iroh handles discovery, NAT traversal, and relay fallback.

Access is default deny and per peer. Each machine runs one long-lived daemon with
a stable identity (a keypair). You grant a specific port to a specific peer's key;
that peer, and no one else, can reach it. A machine you have not met enrolls with a
one-time token -- so you can boot a fleet of untrusted workloads that phone home and
each get exactly the access you granted, with no manual key exchange, and with
siblings invisible to each other.

The case it was built for: a dedicated VM per task -- a
[vibenv](https://github.com/cablehead/vibenv.dag) -- with no inbound ports. Boot it,
it dials your laptop, and the two or three ports you care about (a web app, a
live-reload server, a terminal) show up on `localhost`, reachable by you alone.

## Example

On my laptop the daemon is already running. Its ticket is stable, so I look it up
once, and I mint a one-time token for the VM I'm about to boot:

```sh
pai-sho ticket
# 5hc4bjqfp6booceusm3jrfebbegyfi6aiqwbgx4xxqmpvg5usoyq
pai-sho grant-token --label vm
# 7fd25613dd5e17cb...   (one-time, valid 5 minutes)
```

The VM runs an [http-nu](https://github.com/cablehead/http-nu) app on `:3001` and
[stellar](https://github.com/cablehead/stellar) on `:7331` for live CSS editing.
I start its daemon pointing home, exposing both ports to my laptop:

```sh
pai-sho daemon -a 5hc4bjqfp6booceusm3jrfebbegyfi6aiqwbgx4xxqmpvg5usoyq \
    -e 3001,7331 --enroll 7fd25613dd5e17cb...
```

The VM enrolls under the label `vm`, and only my laptop can reach it; anyone else
who dials the VM is refused. Close the laptop, reopen it, and the connection
restores on its own, no new token needed.

The VM is **auto-projected** on enrollment: it gets its own local address, and its
ports bind there under the name it enrolled with. With the daemon serving its
resolver (`--resolver 127.0.0.1:5353`) and the OS pointed at it for `.ps.internal`, both
ports answer by name:

```sh
# vm.ps.internal:3001 and vm.ps.internal:7331 are reachable, no manual step
```

Every port a VM exposes binds under that one address, so a second VM's `:3001`
never collides with this one, and every VM can use the same port for the same job.
`project` is the override when you want to pin an address or rename; `unproject`
takes a surface down.

Spin up something new on the VM and expose it live:

```sh
http-nu :3002 -c '{|req| "hello from a new experiment"}'
pai-sho expose 3002
```

Because `vm` is already projected, `3002` binds under it too, immediately at
`http://vm.ps.internal:3002` in my browser. Done with it? `pai-sho unexpose 3002`.

## Install

```sh
cargo install pai-sho
```

```sh
brew install cablehead/tap/pai-sho
```

```sh
eget cablehead/pai-sho
```

Or grab a binary from [releases](https://github.com/cablehead/pai-sho/releases).

## Usage

```
pai-sho [--socket <path>] <command>
```

### Commands

```
daemon [options]           Start the daemon
ticket                     Print the daemon's ticket
grant-token --label <l>    Mint a one-time enrollment token (valid 5 min)
pin <key> --label <l>      Enroll a peer by key, no token (host-attested)
add-peer <ticket>          Connect to a peer
remove-peer <ticket>       Disconnect from a peer (and drop its pin)
expose <port> [--to <key>] Grant a local port to peers (default: all known)
unexpose <port> [--to <k>] Revoke grants for a port (or one peer's grant)
project <peer> [--ip <a>] [--as <name>]  Bind a peer's ports at a local address
unproject <peer>           Take a peer's surface down (unbind its ports)
surfaces                   Show each peer and its projection (JSON)
list                       Show peers, grants, and bindings (JSON)
```

### Daemon Options

| Option | Default | Description |
|--------|---------|-------------|
| `--host` | `127.0.0.1` | Address to forward exposed ports to |
| `-a, --add` | | Add peer on startup (repeatable) |
| `-e, --expose` | | Expose port to the `-a` peers (repeat or comma-separate) |
| `--enroll` | | One-time token to present to the `-a` peers |
| `--key` | `~/.local/state/pai-sho/key` | Secret key path (created if missing) |
| `--socket` | `/tmp/pai-sho.sock` | Unix socket path |
| `--resolver` | | Serve the owned `*.ps.internal` resolver on this UDP address (e.g. `127.0.0.1:5353`) |

## How it works

**Identity.** Each daemon has a stable ticket -- an iroh endpoint ID backed by a
keypair persisted at `--key`. Because it is stable, a launcher can bake one operator
ticket into every workload it boots.

**Grants.** Access is default deny. A port is exposed by a grant -- `(port) -> peer
key` -- and served only to the peers named in one. iroh gives the connecting peer's
key cryptographically, so a grant names a proven identity, not a shareable address:
you cannot hand out reach by leaking a string
([ADR 0001](docs/adr/0001-directed-grants.md)).

**Enrollment.** An incoming connection from an unknown key is refused unless it
presents a one-time token minted by `grant-token`. A valid claim pins the peer's key
under the token's label and is then spent; pins persist across restarts, so a reboot
does not orphan enrolled workloads ([ADR 0002](docs/adr/0002-token-enrollment.md)).

**Forwarding.** Each peer is announced only the ports granted to it. When a peer
announces a granted port it is auto-projected: it gets a local address and its
ports bind there, reachable without a manual step. Traffic runs over the encrypted
QUIC connection. It works both ways -- something running locally on `:4001` becomes
reachable on the peer with `pai-sho expose 4001`.

**Surfaces.** A peer's ports are addressed as a unit at one local IP, named after
its enrollment label. Because each peer owns its address, two peers can expose the
same port without colliding, so every peer can use the same port for the same job.
`project` overrides the auto-assignment (pin an address with `--ip`, rename with
`--as`), `unproject` takes a surface down, and projections persist across a restart
([ADR 0004](docs/adr/0004-peer-surfaces.md)).

**Resolver.** With `--resolver`, the daemon answers `<name>.ps.internal` from the live
surface table, so `vibenv-ndyg.ps.internal` reaches that peer's ports. It is authoritative
for one suffix and never touches the system resolver; you point the OS at it for
`.ps.internal` only (`/etc/resolver/ps.internal` on macOS, a dnsmasq `server=/ps.internal/...` forward on
Linux). See [ADR 0005](docs/adr/0005-auto-project-and-owned-resolver.md).

**Reconnection.** If the connection drops, both sides reconnect with exponential
backoff. Projected surfaces stay in place and rebind when the link comes back.

## See also

[ngrok](https://ngrok.com) and [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/)
are great when you need a public URL anyone can reach. pai-sho is for connecting your
own machines, or sharing a ticket with a friend so they can see something you're
working on.

[SSH tunnels](https://www.ssh.com/academy/ssh/tunneling) need inbound access on at
least one side. pai-sho works when neither machine has open inbound ports.

[WireGuard](https://www.wireguard.com/), [Tailscale](https://tailscale.com), and
[NetBird](https://netbird.io/) are mesh VPNs that give every machine an IP on a
virtual network. pai-sho is narrower: you expose specific ports, not your whole
machine, which makes it easier to reason about exactly what is reachable.

[dumbpipe](https://github.com/n0-computer/dumbpipe) is the direct inspiration.
