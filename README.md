<p align="center">
  <img alt="A forest spirit placing a tile on a pai sho board" width="380" src="docs/assets/pai-sho.png">
</p>

<h1 align="center">pai-sho</h1>

<p align="center">
  Spin up a box in the middle of nowhere, with no way in. Drop one binary on it.<br>
  No open ports, no public IP: it dials home and punches through.<br>
  Reach your boxes from your laptop, each under its own name like <code>vibenv-ndyg.pai-sho</code>.
</p>

<p align="center">
  <a href="https://github.com/cablehead/pai-sho/actions/workflows/ci.yml">
    <img src="https://github.com/cablehead/pai-sho/actions/workflows/ci.yml/badge.svg" alt="CI">
  </a>
  <a href="https://crates.io/crates/pai-sho">
    <img src="https://img.shields.io/crates/v/pai-sho.svg" alt="Crates">
  </a>
  <a href="https://discord.com/invite/YNbScHBHrh">
    <img src="https://img.shields.io/discord/1182364431435436042?logo=discord" alt="Discord">
  </a>
</p>

pai-sho forwards specific TCP ports between your machines over an encrypted
peer-to-peer QUIC connection, built on
[iroh](https://github.com/n0-computer/iroh). Neither machine needs an open
inbound port, a public IP, or a relay you run. iroh handles discovery, NAT
traversal, and relay fallback.

Access is default deny and per peer. Each machine runs one long-lived daemon with
a stable identity, a keypair. You grant a specific port to a specific peer's key,
and that peer alone can reach it. A machine you have not met enrolls with a
one-time token, so you can boot a fleet of untrusted workloads that phone home,
each with exactly the access you granted and none aware of its siblings.

The peers you can reach live on a private network the daemon runs for you. Each
one gets its own address on that network and a name to match, so you reach its
ports at `peer.pai-sho:<port>`. Two peers can serve the same port without
clashing, because each has its own address. None of it is published to the rest
of your machine or the rest of DNS.

The case it was built for is a dedicated VM per task, a
[vibenv](https://github.com/cablehead/vibenv.dag), with no inbound ports. Boot it,
it dials your laptop, and the ports you care about (say a web app and a
live-reload server) come up at `vibenv-ndyg.pai-sho:3001` and `vibenv-ndyg.pai-sho:7331`, reachable
by you alone.

## Example

On my laptop the daemon is already running on its own network interface (the
[Homebrew install](#install) sets that up; [Setting up the network](#setting-up-the-network)
covers doing it by hand). I print its ticket and mint a one-time token for the VM
I'm about to boot:

```sh
pai-sho ticket
# 5hc4bjqfp6booceusm3jrfebbegyfi6aiqwbgx4xxqmpvg5usoyq
pai-sho grant-token --label vibenv-ndyg
# 7fd25613dd5e17cb...   (one-time, valid 5 minutes)
```

The VM runs an [http-nu](https://github.com/cablehead/http-nu) app on `:3001` and
[stellar](https://github.com/cablehead/stellar) on `:7331` for live CSS editing.
Its daemon dials home and exposes both ports to my laptop:

```sh
pai-sho daemon -a 5hc4bjqfp6booceusm3jrfebbegyfi6aiqwbgx4xxqmpvg5usoyq \
    -e 3001,7331 --enroll 7fd25613dd5e17cb...
```

The VM enrolls under the label `vibenv-ndyg`, and only my laptop can reach it. Anyone else
who dials the VM is refused.

On enrollment the VM is projected onto my network on its own: it gets an address
like `10.99.1.2`, and its ports bind there under the name `vibenv-ndyg`. Both answer by
name, with no manual step:

```sh
curl http://vibenv-ndyg.pai-sho:3001
open http://vibenv-ndyg.pai-sho:7331
```

Spin up something new on the VM and expose it live:

```sh
http-nu :3002 -c '{|req| "hello from a new experiment"}'
pai-sho expose 3002
```

`vibenv-ndyg` is already on my network, so `3002` binds under it too, reachable at
`http://vibenv-ndyg.pai-sho:3002` right away. Done with it? `pai-sho unexpose 3002`.

Close the laptop and reopen it: the connection restores on its own, the surface
rebinds, and no new token is needed.

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

The Homebrew install also sets up the private network under a supervisor: it
brings up the interface, routes `10.99.0.0/16` into it, and points the system at
the daemon's resolver for `.pai-sho`. After `brew install`, the daemon is running
and `*.pai-sho` names resolve.

## Setting up the network

`--tun` puts each peer on a private `10.99.0.0/16` network. The daemon sits at
`10.99.0.1`, peers land on `10.99.1.x`, and the daemon's resolver answers
`*.pai-sho` in-stack on `10.99.0.53`.

### macOS

The daemon creates the interface, which needs root:

```sh
sudo pai-sho daemon --tun utun
echo "nameserver 10.99.0.53" | sudo tee /etc/resolver/pai-sho
```

### Linux

Create the interface ahead of time and hand it to the daemon's user, so the
daemon itself runs unprivileged:

```sh
sudo ip tuntap add dev ps0 mode tun user "$USER"
sudo ip addr add 10.99.0.1/16 dev ps0
sudo ip link set ps0 up
pai-sho daemon --tun ps0
```

Then send `.pai-sho` to `10.99.0.53`, for example with a dnsmasq
`server=/pai-sho/10.99.0.53` forward.

Without `--tun`, surfaces fall back to loopback addresses (`127.0.1.x`) and you
serve the resolver with `--resolver <addr>`. You lose the private network but keep
the names.

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
| `--tun` | | Put surfaces on a private TUN network (`utun` on macOS, a pre-created device like `ps0` on Linux); the resolver answers in-stack on `10.99.0.53:53` |
| `--resolver` | | Loopback mode, an alternative to `--tun`: serve the `*.pai-sho` resolver on this UDP address (e.g. `127.0.0.1:5353`) |

## How it works

**Identity.** Each daemon has a stable ticket, an iroh endpoint ID backed by a
keypair at `--key`. Because it does not change, a launcher can bake one operator
ticket into every workload it boots.

**Grants.** Access is default deny. A port becomes reachable only through a grant that names the peers allowed to
reach it, and is served to them alone. iroh proves the connecting
peer's key cryptographically, so a grant names a proven identity, not a shareable
address. You cannot hand out reach by leaking a string
([ADR 0001](docs/adr/0001-directed-grants.md)).

**Enrollment.** A connection from an unknown key is refused unless it carries a
one-time token from `grant-token`. A valid token pins the peer's key under the
token's label and is then spent. Pins survive restarts, so a reboot does not
orphan enrolled workloads ([ADR 0002](docs/adr/0002-token-enrollment.md)). When you
already know a peer's key, `pin` does the same without a token
([ADR 0003](docs/adr/0003-host-attested-enrollment.md)).

**Forwarding.** Each peer hears only the ports granted to it, and traffic runs
over the encrypted QUIC connection. It goes both ways: something on your own
`:4001` becomes reachable on a peer with `pai-sho expose 4001`.

**The network.** With `--tun`, the daemon runs its own TCP/IP stack on a private
network interface. The daemon sits at `10.99.0.1`, peers get addresses on
`10.99.1.x`, and the resolver answers in-stack on `10.99.0.53:53`. On Linux the
interface is created ahead of time and owned by the daemon's user, so the daemon
needs no elevated capability. On macOS the daemon creates a utun itself, which
needs root.

**Surfaces.** A peer's ports are addressed together at one address, named after
its enrollment label. A peer is projected automatically the first time it
announces a granted port. Because each peer owns its address, two peers can serve
the same port without colliding. `project` overrides the automatic choice (pin an
address with `--ip`, rename with `--as`), `unproject` takes a surface down, and
projections survive a restart ([ADR 0004](docs/adr/0004-peer-surfaces.md)).

**Resolver.** The daemon answers `<name>.pai-sho` from the live surface table, so
`vibenv-ndyg.pai-sho` reaches that peer's ports and stops resolving when the peer goes
away. It is authoritative for the one suffix and never touches the rest of your
DNS. Point the OS at it for `.pai-sho` only: `/etc/resolver/pai-sho` on macOS, a
dnsmasq `server=/pai-sho/10.99.0.53` forward on Linux
([ADR 0005](docs/adr/0005-auto-project-and-owned-resolver.md)).

**Reconnection.** If the connection drops, both sides retry with exponential
backoff. Projected surfaces stay put and rebind when the link returns.

## See also

[ngrok](https://ngrok.com) and [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/)
are great when you need a public URL anyone can reach. pai-sho is for connecting
your own machines, or sharing a ticket with a friend so they can see something
you're working on.

[SSH tunnels](https://www.ssh.com/academy/ssh/tunneling) need inbound access on at
least one side. pai-sho works when neither machine has open inbound ports.

[WireGuard](https://www.wireguard.com/), [Tailscale](https://tailscale.com), and
[NetBird](https://netbird.io/) are mesh VPNs that put every machine on a virtual
network. pai-sho is narrower: you expose specific ports, not the whole machine,
which keeps it easy to reason about exactly what is reachable.

[dumbpipe](https://github.com/n0-computer/dumbpipe) is the direct inspiration.
[pigeons](https://pigeons.computer), SSH over iroh from the same team, is where
pai-sho's connection handling comes from.

Questions or ideas: come by the [Discord](https://discord.com/invite/YNbScHBHrh).
