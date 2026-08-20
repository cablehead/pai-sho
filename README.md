<p align="center">
  <img alt="A forest spirit placing a tile on a pai sho board" width="380" src="docs/assets/pai-sho.png">
</p>

<h1 align="center">pai-sho</h1>

<p align="center">
  Forward ports between your own machines, peer to peer over <a href="https://github.com/n0-computer/iroh">iroh</a>.<br>
  Neither side needs an account, a public IP, or an open inbound port.<br>
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

## Example scenarios

### A shared build box

A team runs a long-lived build box. Its dashboard is on `localhost:8080`. The
box invites your laptop and grants the port in the same command:

```sh
# build box
pai-sho invite --expose 8080
```

```sh
# laptop
pai-sho accept 5hc4bjqfp6...7fd25613dd... --as buildbox
curl http://buildbox.pai-sho:8080
```

### A laptop boots a VM

The roles reverse here: the consumer invites, and picks the name. You boot a
dedicated VM per task, a [vibenv](https://github.com/cablehead/vibenv.dag), with
no inbound ports. Invite it from your laptop before it boots:

```sh
pai-sho invite --as vibenv-ndyg
# 5hc4bjqfp6...7fd25613dd...   one-time, valid 5 minutes
```

The VM runs an [http-nu](https://github.com/cablehead/http-nu) app on
`localhost:3001` and [stellar](https://data-star.dev/pro#stellar-css) on
`localhost:7331`. Its daemon takes the invitation up on startup and exposes
both:

```sh
pai-sho daemon --accept 5hc4bjqfp6...7fd25613dd... -e 3001,7331
```

It is projected on acceptance, with no manual step: an address on your laptop's
private network, ports bound there under the name you chose. Only your laptop can
reach it, and anyone else who dials is refused.

```sh
curl http://vibenv-ndyg.pai-sho:3001
open http://vibenv-ndyg.pai-sho:7331
```

Close the laptop and reopen it. The connection restores and the ports rebind.

[docs/scenarios.md](docs/scenarios.md) works both through in full.

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
| `--name` | | This node's own name. The resolver answers `<name>.pai-sho` with `--host`, so a service reached locally and from a peer has one origin, which is what CORS needs |
| `--socket-owner` | | Username to own the control socket, chowned right after bind. Lets the CLI skip sudo when the daemon runs as root |
| `--socket-mode` | | Octal mode for the control socket, e.g. `660` |

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
restarts, so a reboot does not orphan a workload. An invitation is
`<key>.<code>`: the key says who to dial, the code admits you. When you already
know a peer's key, `invite <key>` authorizes it with no secret created at all
([ADR 0006](docs/adr/0006-invitations.md),
[ADR 0003](docs/adr/0003-host-attested-enrollment.md)).

**Connecting.** Peers dial by public key over
[iroh](https://github.com/n0-computer/iroh). It punches through NAT, so neither
side needs an open inbound port or a public IP. When it can't punch through, an
[n0](https://n0.computer/) relay forwards the traffic without being able to read
it.

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

**Structure.** `src/core/` decides and does no IO: admission, grants, and tunnel
authorization, unit tested without a network. The shell in `peer.rs` feeds it
events and carries out the actions it returns
([ADR 0007](docs/adr/0007-pure-core.md)).

The rules that hold whatever you type are in
[docs/scenarios.md](docs/scenarios.md#invariants).

## See also

[ngrok](https://ngrok.com) and [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/)
are great when you need a public URL anyone can reach. pai-sho is for connecting
your own machines, or sending an invitation to a friend so they can see
something you're working on.

[SSH tunnels](https://www.ssh.com/academy/ssh/tunneling) need inbound access on at
least one side. pai-sho works when neither machine has open inbound ports.

[WireGuard](https://www.wireguard.com/) has no control plane and no relays. It
only goes direct, so a peer entry in the
[config file](https://www.wireguard.com/quickstart/) needs an `Endpoint` with a
routable address. There is no hole punching and no fallback. If both machines are
behind NAT, you are standing up a bounce host yourself. Tailscale adds that
machinery around WireGuard; pai-sho gets it from iroh, over QUIC.

[dumbpipe](https://github.com/n0-computer/dumbpipe) is the direct inspiration.
[pigeons](https://pigeons.computer), SSH over iroh from the same team, is where
pai-sho's connection handling comes from.

## Why not Tailscale?

You probably should use [Tailscale](https://tailscale.com). It solves this
problem well, and there is a company behind it.

### No account

The connection machinery is the same. Servers negotiate the initial connection,
then [hole punching](https://tailscale.com/blog/how-nat-traversal-works) gets a
direct path. When it can't, a relay carries the traffic:
[DERP](https://tailscale.com/kb/1232/derp-servers) for Tailscale,
[iroh's relays](https://www.iroh.computer/docs/concepts/relay) for pai-sho, run
by [n0](https://n0.computer/). That whole layer comes from
[iroh](https://github.com/n0-computer/iroh). What Tailscale has and pai-sho does
not is a row above all that.

```
Tailscale
  box ------->  controlplane.tailscale.com  <------- laptop   membership
  box <~ ~ ~ ~  derp*.tailscale.com         ~ ~ ~ ~> laptop   negotiate, relay
  box <--------------------------------------------> laptop   direct

pai-sho
  box <~ ~ ~ ~  *.relay.iroh.network        ~ ~ ~ ~> laptop   negotiate, relay
  box <--------------------------------------------> laptop   direct
```

A Tailscale node registers with the
[coordination server](https://tailscale.com/blog/how-tailscale-works), which
decides membership and hands it a filtered list of the peers it may see. A
pai-sho box dials your laptop by public key, resolved by
[iroh's address lookup](https://www.iroh.computer/docs/concepts/discovery).
Nothing in that path can add a peer to your set, and there is nothing to sign up
for.

### Specific ports, not a whole machine

Tailscale gives a peer an IP, and everything listening on it is reachable unless
an [ACL](https://tailscale.com/kb/1018/acls) says otherwise. Default allow, then
narrow it. pai-sho grants one port at a time to one key, and a peer with no
grants sees nothing. Day to day the two feel much the same, since you type a
name and a port either way.

### Less to install

Without `--tun`, pai-sho binds loopback addresses. On Linux that needs no
network device and no privilege, because `127.0.0.0/8` already routes to `lo`.
Tailscale needs a tun device, or its
[userspace mode](https://tailscale.com/kb/1112/userspace-networking), which
gives you a proxy rather than real listeners. `--tun` puts pai-sho in the same
position, so this only holds on loopback.

### Tailscale's ops story is much nicer

One [policy file](https://tailscale.com/kb/1337/policy-syntax) for the whole
tailnet, so who-can-reach-what is a thing you read in a single place. pai-sho's
answer is "which command did you run on which machine." A web UI is the obvious
next step.

## More

[docs/scenarios.md](docs/scenarios.md) works two flows end to end: a shared build
box reached from a laptop, and a laptop booting a vibenv. Each says what has to
be true and what travels between the machines, and why the commands took the
shape they did. Its [invariants](docs/scenarios.md#invariants) are the shortest
statement of the model.

The [ADRs](docs/adr) record the decisions and how they moved: directed grants,
two passes at enrollment before invitations landed, peer surfaces, the owned
resolver, and the pure core.

Questions or ideas: come by the [Discord](https://discord.com/invite/YNbScHBHrh).
