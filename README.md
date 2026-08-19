<p align="center">
  <img alt="A forest spirit placing a tile on a pai sho board" width="380" src="docs/assets/pai-sho.png">
</p>

<h1 align="center">pai-sho</h1>

<p align="center">
  Forward ports between your own machines, peer to peer.<br>
  Neither side needs an open inbound port, a public IP, or an account.<br>
  Only what you grant is reachable.
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

Machines link by invitation: one side extends it, the other takes it up. Access
is default deny. You grant a port to a peer's key, and that peer alone can reach
it.

## Example

Say you boot a dedicated VM per task, a
[vibenv](https://github.com/cablehead/vibenv.dag), with no inbound ports. Invite
it from your laptop before it boots:

```sh
pai-sho invite --as vibenv-ndyg
# 5hc4bjqfp6booceusm3jrfebbegyfi6aiqwbgx4xxqmpvg5usoyq.7fd25613dd5e17cb...
# one-time, valid 5 minutes
```

That one value says who to dial and proves the VM may. (The laptop's daemon is
already running on its own network interface. [Install](#install) sets that up;
[Setting up the network](#setting-up-the-network) covers doing it by hand.)

The VM runs an [http-nu](https://github.com/cablehead/http-nu) app on `:3001` and
[stellar](https://github.com/cablehead/stellar) on `:7331` for live CSS editing.
Its daemon takes up the invitation and exposes both ports to the laptop:

```sh
pai-sho daemon --accept 5hc4bjqfp6...7fd25613dd... -e 3001,7331
```

The VM comes up as `vibenv-ndyg`, and only your laptop can reach it. Anyone else
who dials the VM is refused.

It is projected on acceptance, with no manual step: it gets an address like
`10.99.1.2` on the laptop's private network, and its ports bind there under the
name `vibenv-ndyg`. Both answer by name:

```sh
curl http://vibenv-ndyg.pai-sho:3001
open http://vibenv-ndyg.pai-sho:7331
```

Spin up something new on the VM and expose it live:

```sh
http-nu :3002 -c '{|req| "hello from a new experiment"}'
pai-sho expose 3002 --all
```

`--all` means every peer this VM knows right now, which is your laptop and
nothing else. It is not a standing rule: a peer admitted later gets nothing.

`vibenv-ndyg` already has an address, so `3002` binds under it too, reachable at
`http://vibenv-ndyg.pai-sho:3002` right away. Done with it? `pai-sho unexpose 3002`.

Close the laptop and reopen it: the connection restores on its own, the surface
rebinds, and no new invitation is needed.

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

Homebrew also ships a supervised launch for the operator: a launchd service that
creates the private network and points the system at the `.pai-sho` resolver. It
does not start on its own; see [Setting up the network](#setting-up-the-network).

## Setting up the network

`--tun` puts each peer on a private `10.99.0.0/16` network. The daemon sits at
`10.99.0.1`, peers land on `10.99.1.x`, and the daemon's resolver answers
`*.pai-sho` in-stack on `10.99.0.53`.

### macOS

Homebrew ships a supervised launch. Trust the tap as your user (once), then start
the service:

```sh
brew trust cablehead/tap
sudo --preserve-env=XDG_CONFIG_HOME brew services start pai-sho
```

`brew trust` records trust for your user. `sudo brew services` is the one root
use Homebrew allows (it loads a launchd service, runs no build scripts); `sudo
brew trust` and `sudo brew install` are refused, and that refusal is correct.
`--preserve-env=XDG_CONFIG_HOME` matters only if you set `XDG_CONFIG_HOME`: plain
`sudo` strips it, so brew looks for your trust file under `$HOME/.homebrew`
instead of your real config home and refuses the tap. Preserving it points brew
back where `brew trust` wrote. (Harmless if you don't set `XDG_CONFIG_HOME`.)

The service creates the utun, points the system at the `.pai-sho` resolver, and
hands you the control socket, so the CLI needs no sudo:

```sh
pai-sho key
```

To run it by hand instead of under the supervisor:

```sh
sudo pai-sho daemon --tun utun --socket-owner "$(stat -f%Su /dev/console)"
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

| Command | Description |
|---------|-------------|
| `daemon [options]` | Start the daemon |
| `key` | Print this daemon's key (hand this to a peer) |
| `invite [<key>] [--as <name>] [--expose <port>...]` | Extend an invitation. With a key, to that key alone (host-attested, no secret). Without one, print a one-time invitation valid 5 minutes. |
| `accept <invite\|key> [--as <name>]` | Take up an invitation, or reach a peer by key |
| `forget <peer>` | Forget a peer: close it, unbind its ports, revoke its grants |
| `expose <port> (--to <key> \| --all)` | Grant a local port to named peers. Nothing is reachable without a grant |
| `unexpose <port> [--to <key>]` | Revoke grants for a port, or just one peer's |
| `project <peer> [--ip <addr>] [--as <name>]` | Bind a peer's ports at a local address |
| `unproject <peer>` | Take a peer's surface down (unbind its ports) |
| `list` | Peers, grants, and where their ports are bound (JSON) |

`--socket` is global, not specific to `daemon`.

### Daemon Options

| Option | Default | Description |
|--------|---------|-------------|
| `--host` | `127.0.0.1` | Address to forward exposed ports to |
| `-a, --accept` | | Take up an invitation, or a peer's key, on startup (repeatable) |
| `-e, --expose` | | Expose port to the `--accept` peers (repeat or comma-separate) |
| `--key` | `~/.local/state/pai-sho/key` | Secret key path (created if missing) |
| `--tun` | | Put surfaces on a private TUN network (`utun` on macOS, a pre-created device like `ps0` on Linux); the resolver answers in-stack on `10.99.0.53:53` |
| `--resolver` | | Loopback mode, an alternative to `--tun`: serve the `*.pai-sho` resolver on this UDP address (e.g. `127.0.0.1:5353`) |

## How it works

**Identity.** Each daemon has a stable key, an iroh endpoint ID backed by a
keypair at `--key`. Because it does not change, a launcher can bake one
operator key into every workload it boots.

**Grants.** Access is default deny. A port becomes reachable only through a grant
that names the peers allowed to reach it, and is served to them alone. iroh
proves the connecting peer's key cryptographically, so a grant names a proven
identity, not a shareable address. You cannot hand out reach by leaking a string
([ADR 0001](docs/adr/0001-directed-grants.md)).

**Invitations.** A connection from an unknown key is refused unless it carries a
code from `invite`. The code is spent on use, and the peer it admitted survives
restarts, so a reboot does not orphan a workload
([ADR 0002](docs/adr/0002-token-enrollment.md)). An invitation is `<key>.<code>`:
who to dial, and the proof you may. When you already know a peer's key,
`invite <key>` authorizes it with no secret created at all
([ADR 0003](docs/adr/0003-host-attested-enrollment.md)).

**Connecting.** Peers dial by public key over
[iroh](https://github.com/n0-computer/iroh). It punches through NAT, so neither
side needs an open inbound port or a public IP. When it can't punch through, an
n0 relay forwards the traffic without being able to read it.

**Forwarding.** Each peer hears only the ports granted to it, and traffic runs
over the encrypted QUIC connection. It goes both ways: something on your own
`:4001` becomes reachable on a peer with `pai-sho expose 4001 --to <key>`, where
the key comes from `pai-sho key` on the machine you are granting to.

**The network.** With `--tun`, the daemon runs its own TCP/IP stack on a private
network interface. The daemon sits at `10.99.0.1`, peers get addresses on
`10.99.1.x`, and the resolver answers in-stack on `10.99.0.53:53`. On Linux the
interface is created ahead of time and owned by the daemon's user, so the daemon
needs no elevated capability. On macOS the daemon creates a utun itself, which
needs root.

**Surfaces.** A peer's ports are addressed together at one address, under the
name you gave it, or a short form of its key if nothing named it. A peer is
projected automatically the first time it announces a granted port. Because each
peer owns its address, two peers can serve the same port without colliding.
`project` overrides the automatic choice (pin an address with `--ip`, rename with
`--as`), `unproject` takes a surface down, and projections survive a restart
([ADR 0004](docs/adr/0004-peer-surfaces.md)).

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
your own machines, or sending an invitation to a friend so they can see
something you're working on.

[SSH tunnels](https://www.ssh.com/academy/ssh/tunneling) need inbound access on at
least one side. pai-sho works when neither machine has open inbound ports.

[WireGuard](https://www.wireguard.com/), [Tailscale](https://tailscale.com), and
[NetBird](https://netbird.io/) are mesh VPNs that put every machine on a virtual
network. pai-sho is narrower: you expose specific ports, not the whole machine,
which keeps it easy to reason about exactly what is reachable.

[dumbpipe](https://github.com/n0-computer/dumbpipe) is the direct inspiration.
[pigeons](https://pigeons.computer), SSH over iroh from the same team, is where
pai-sho's connection handling comes from.

## More

[docs/scenarios.md](docs/scenarios.md) works two flows end to end: a shared build
box reached from a laptop, and a laptop booting a vibenv. Each says what has to
be true, what travels between the machines, and why the commands are shaped the
way they are.

The [ADRs](docs/adr) record the decisions: directed grants, invitations,
host-attested enrollment, peer surfaces, and the owned resolver.

Questions or ideas: come by the [Discord](https://discord.com/invite/YNbScHBHrh).
