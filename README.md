<h1>
<p align="center">
  pai-sho
</h1>
  <p align="center">
    What happens when you want <a href="https://github.com/n0-computer/dumbpipe">dumbpipe</a> to stay running, handle a few ports at once, and reconnect when your laptop wakes up.
  </p>
</p>

<p align="center">
  <a href="https://github.com/cablehead/pai-sho/actions/workflows/ci.yml">
    <img src="https://github.com/cablehead/pai-sho/actions/workflows/ci.yml/badge.svg" alt="CI">
  </a>
  <a href="https://crates.io/crates/pai-sho">
    <img src="https://img.shields.io/crates/v/pai-sho.svg" alt="Crates">
  </a>
</p>

My workflow is generally a dedicated VM per task -- a [vibenv](https://github.com/cablehead/vibenv.dag). These environments tend to have no open inbound ports. iroh's [dumbpipe](https://github.com/n0-computer/dumbpipe) worked nicely for reaching a single port, but when you need 3-4 ports you have to run 3-4 dumbpipes on each side -- so 6-8 processes just to share a few ports. pai-sho gives you a single long-lived daemon that manages multiple ports. You can expose and unexpose them on the fly, and if the connection drops it comes back on its own.

## Example

Say I have a VM running an [http-nu](https://github.com/cablehead/http-nu) app on `:3001` and [stellar](https://github.com/cablehead/stellar) on `:7331` for live CSS editing. On my laptop, the daemon is already running; its ticket is stable across restarts. I mint a one-time token for the VM:

```sh
pai-sho ticket
# 5hc4bjqfp6booceusm3jrfebbegyfi6aiqwbgx4xxqmpvg5usoyq
pai-sho grant-token --label vm
# 7fd25613dd5e17cb...   (one-time, valid 5 minutes)
```

On the VM, I start the daemon pointing home, with both ports exposed:

```sh
pai-sho daemon -a 5hc4bjqfp6booceusm3jrfebbegyfi6aiqwbgx4xxqmpvg5usoyq \
    -e 3001,7331 --enroll 7fd25613dd5e17cb...
```

The VM enrolls under the label `vm`, and `localhost:3001` and `localhost:7331` on my laptop reach it. Those ports are exposed to my laptop and to no one else -- anyone else who dials either daemon is refused. Close the laptop, reopen it -- the connection restores on its own, no new token needed.

Later, I spin up something new on the VM:

```sh
http-nu :3002 -c '{|req| "hello from a new experiment"}'
pai-sho expose 3002
```

It's immediately there at `http://localhost:3002` in my local browser. Done with it? `pai-sho unexpose 3002`.

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
ticket                     Print daemon's ticket
grant-token --label <l>    Mint a one-time enrollment token (valid 5 min)
add-peer <ticket>          Connect to a peer
remove-peer <ticket>       Disconnect from a peer (and drop its pin)
expose <port> [--to <key>] Grant a local port to peers (default: all known)
unexpose <port> [--to <k>] Revoke grants for a port (or one peer's grant)
list                       Show peers, grants, bindings (JSON)
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

## How it works

Each daemon has a stable ticket (an iroh endpoint ID; the key persists at `--key`). When you add a peer by ticket, iroh handles discovery and NAT traversal -- connecting directly when possible, falling back through relay servers when needed.

Access is default deny. A port is exposed by a grant -- `(port) -> peer key` -- and served only to peers named in one ([ADR 0001](docs/adr/0001-directed-grants.md)). An incoming connection from an unknown key is refused unless it presents a one-time enrollment token minted by `grant-token`; a valid claim pins the peer's key under the token's label, and the pin survives restarts ([ADR 0002](docs/adr/0002-token-enrollment.md)).

Each peer is announced the ports granted to it. When a peer grants you port 3001, a local TCP listener binds `127.0.0.1:3001` on your side. Traffic goes over an encrypted QUIC connection. It goes both ways -- if you have something running locally on `:4001`, `pai-sho expose 4001` makes it available in the remote session too.

If the connection drops, both sides reconnect with exponential backoff. Existing port bindings stay active and resume when the connection comes back.

## See also

[ngrok](https://ngrok.com) and [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/) are great when you need a public URL anyone can reach. pai-sho is more for connecting your own machines or sharing a ticket with a friend so they can see something you're working on.

[SSH tunnels](https://www.ssh.com/academy/ssh/tunneling) need inbound access on at least one side. pai-sho works when neither machine has open inbound ports.

[WireGuard](https://www.wireguard.com/), [Tailscale](https://tailscale.com), [NetBird](https://netbird.io/) -- mesh VPNs that give every machine an IP on a virtual network. pai-sho is narrower -- you expose specific ports, not your whole machine. It can be easier to reason about what you're exposing when you go port by port.

[dumbpipe](https://github.com/n0-computer/dumbpipe) is the direct inspiration.
