<p align="center">
  <img alt="A forest spirit placing a tile on a pai sho board" width="380" src="docs/assets/pai-sho.png">
</p>

<h1 align="center">pai-sho</h1>

<p align="center">
  Forward ports between your own machines, peer to peer over <a href="https://github.com/n0-computer/iroh">iroh</a>.<br>
  Neither side needs an account, a public IP, or an open inbound port.<br>
  A port is reachable only by the peers you grant it to.
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
# 5hc4bjqfp6...7fd25613dd...   one-time, valid 5 minutes
```

Paste the invitation on your laptop, and give the box a name:

```sh
# laptop
pai-sho accept 5hc4bjqfp6...7fd25613dd... --as buildbox
curl http://buildbox.pai-sho:8080
```

### A laptop boots a VM

You boot a dedicated VM per task, a [vibenv](https://github.com/cablehead/vibenv.dag),
with no inbound ports. This time the laptop does the inviting, and names the VM
before it exists:

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

As soon as it connects, the VM gets an address on your laptop's private network,
and its two ports are bound there under the name you chose. Only your laptop is
admitted; a dial from any other key is refused.

```sh
curl http://vibenv-ndyg.pai-sho:3001
open http://vibenv-ndyg.pai-sho:7331
```

If you close the laptop and reopen it, the connection comes back and the ports
rebind.

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

Homebrew also ships a launchd service that creates the private network and
points the system at the `.pai-sho` resolver. It does not start on its own; see
[Setting up the network](#setting-up-the-network).

## Setting up the network

`--tun` puts each peer on a private `10.99.0.0/16` network. The daemon sits at
`10.99.0.1`, peers land on `10.99.1.x`, and the daemon's resolver answers
`*.pai-sho` at `10.99.0.53`.

### macOS

Homebrew ships a supervised launch. Trust the tap as your user (once), then start
the service:

```sh
brew trust cablehead/tap
sudo --preserve-env=XDG_CONFIG_HOME brew services start pai-sho
```

`brew trust` records trust for your user. `sudo brew services` is the one root
use Homebrew allows; it loads a launchd service and runs no build scripts.
`sudo brew trust` and `sudo brew install` are refused.
`--preserve-env=XDG_CONFIG_HOME` matters only if you set `XDG_CONFIG_HOME`:
plain `sudo` strips it, so brew looks for your trust file under
`$HOME/.homebrew` and refuses the tap.

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

Without `--tun`, peers are bound at loopback addresses (`127.0.1.x`) and you
serve the resolver yourself with `--resolver <addr>`. Names still resolve, to
those addresses.

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
| `--tun` | | Put surfaces on a private TUN network (`utun` on macOS, a pre-created device like `ps0` on Linux); the resolver answers at `10.99.0.53:53` |
| `--resolver` | | Loopback mode, an alternative to `--tun`: serve the `*.pai-sho` resolver on this UDP address (e.g. `127.0.0.1:5353`) |
| `--name` | | This node's own name. The resolver answers `<name>.pai-sho` with `--host`, so a service reached locally and from a peer has one origin, which is what CORS needs |
| `--socket-owner` | | Username to own the control socket, chowned right after bind. Lets the CLI skip sudo when the daemon runs as root |
| `--socket-mode` | | Octal mode for the control socket, e.g. `660` |

## How it works

**Identity.** Each daemon has a stable key: an iroh endpoint ID, backed by a
keypair stored at `--key`. Because it never changes, whatever boots your VMs can
hand each one your laptop's key ahead of time.

**Grants.** A port is reachable only by the peers a grant names, and is served
to them alone. A grant names a key, and iroh proves the connecting peer holds
that key, so a peer cannot pass its access on to another machine
([ADR 0001](docs/adr/0001-directed-grants.md)).

**Invitations.** A connection from an unknown key is refused unless it carries a
code from `invite`. The code is spent on use, and the peer it admitted is
remembered across restarts. An invitation is
`<key>.<code>`: the key says who to dial, the code admits you. When you already
know a peer's key, `invite <key>` authorizes it with no secret created at all
([ADR 0006](docs/adr/0006-invitations.md),
[ADR 0003](docs/adr/0003-host-attested-enrollment.md)).

**Admission.** `list` reports how each peer got in, because the three routes
carry different weight when auditing: a code was held by whoever used it, a key
was vouched for ahead of time.

| `admission` | How the peer arrived |
|-------------|----------------------|
| `code` | It dialed in presenting a one-time `invite` code, which was spent on use. |
| `added` | Its key was taken up on this side with `accept`, or `daemon --accept`. This daemon dials it. |
| `key` | Its key was authorized ahead of time with `invite <key>`, no secret created. It dials in; this daemon never dials it. |

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
`10.99.1.x`, and the resolver answers at `10.99.0.53:53`. On Linux the
interface is created ahead of time and owned by the daemon's user, so the daemon
needs no elevated capability. On macOS the daemon creates a utun itself, which
needs root.

**Surfaces.** A peer's ports all live at one local address, under the name you
gave it, or the first eight characters of its key if you gave none. That address
with its ports is the peer's surface. It comes up by itself the first time the
peer announces a granted port. Each peer has its own address, so two peers can
both serve `8080`. `project` overrides the defaults (pin an address with `--ip`,
rename with `--as`), `unproject` takes a surface down, and projections survive a
restart ([ADR 0004](docs/adr/0004-peer-surfaces.md)).

**Resolver.** The daemon answers `<name>.pai-sho` from the live surface table, so
`vibenv-ndyg.pai-sho` reaches that peer's ports and stops resolving when the peer goes
away. It is authoritative for the one suffix and never touches the rest of your
DNS. Point the OS at it for `.pai-sho` only: `/etc/resolver/pai-sho` on macOS, a
dnsmasq `server=/pai-sho/10.99.0.53` forward on Linux
([ADR 0005](docs/adr/0005-auto-project-and-owned-resolver.md)).

**Reconnection.** If the connection drops, both sides retry with exponential
backoff. Projected surfaces stay put and rebind when the link returns.

**Structure.** The decisions (who may connect, which grants exist, whether a
tunnel is allowed) live in `src/core/`, which does no IO and is unit tested
without a network. `peer.rs` feeds it events and carries out the actions it
returns ([ADR 0007](docs/adr/0007-pure-core.md)).

The [invariants](docs/scenarios.md#invariants) in `docs/scenarios.md` are the
five things that hold no matter which commands you run.

## See also

[ngrok](https://ngrok.com) and [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/)
are great when you need a public URL anyone can reach. pai-sho is for connecting
your own machines, or sending an invitation to a friend so they can see
something you're working on.

[SSH tunnels](https://www.ssh.com/academy/ssh/tunneling) need inbound access on at
least one side. pai-sho works when neither machine has open inbound ports.

[WireGuard](https://www.wireguard.com/) only goes direct: a peer entry in the
[config file](https://www.wireguard.com/quickstart/) needs an `Endpoint` with a
routable address, and there is no hole punching or relay to fall back to. If
both machines are behind NAT you need a bounce host. Tailscale adds that
machinery around WireGuard; pai-sho gets it from iroh.

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
[iroh](https://github.com/n0-computer/iroh). Tailscale has one more layer above
it, the top row here:

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
Address lookup only answers where a key is reachable; it cannot add a peer to
your set, and it needs no sign-up.

### One port at a time

Tailscale gives a peer an IP, and everything listening on it is reachable unless
an [ACL](https://tailscale.com/kb/1018/acls) says otherwise. pai-sho starts with
nothing reachable, and you grant one port at a time to one key. Day to day the
two feel much the same, since you type a name and a port either way.

### Less to install

Without `--tun`, pai-sho binds loopback addresses. On Linux that needs no
network device and no privilege, because `127.0.0.0/8` already routes to `lo`.
Tailscale needs a tun device, or its
[userspace mode](https://tailscale.com/kb/1112/userspace-networking), which
gives you a proxy rather than real listeners. `--tun` puts pai-sho in the same
position, so this only holds on loopback.

### Tailscale's ops story is much nicer

Tailscale has one [policy file](https://tailscale.com/kb/1337/policy-syntax) for
the whole tailnet, so you can read who can reach what in one place. In pai-sho
that information is spread across whichever `invite` and `expose` commands ran
on which machine, and `list` shows one daemon's view of it; nothing yet shows
the whole picture.

## More

[docs/scenarios.md](docs/scenarios.md) works two flows end to end: a shared build
box reached from a laptop, and a laptop booting a vibenv. Each says what has to
be true and what travels between the machines, and why the commands took the
shape they did.

The [ADRs](docs/adr) record the decisions and which ones superseded which:
directed grants, two passes at enrollment before invitations landed, peer
surfaces, the owned resolver, and the pure core.

Questions or ideas: come by the [Discord](https://discord.com/invite/YNbScHBHrh).
