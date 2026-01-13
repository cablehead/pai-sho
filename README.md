# pai-sho

P2P TCP port forwarding over [iroh](https://github.com/n0-computer/iroh).

## Usage

```
pai-sho [--socket <path>] <command>
```

### Global Options

| Option | Default | Description |
|--------|---------|-------------|
| `--socket` | `/tmp/pai-sho.sock` | Unix socket path |

### Commands

```
daemon [--host <ip>]    Start the daemon
ticket                  Print daemon's ticket
add-peer <ticket>       Connect to a peer
remove-peer <ticket>    Disconnect from a peer
expose <port>           Expose a local port to peers
unexpose <port>         Stop exposing a port
list                    Show peers, exposed ports, bindings
```

### Daemon Options

| Option | Default | Description |
|--------|---------|-------------|
| `--host` | `127.0.0.1` | Address to forward exposed ports to |

## Example

```sh
# Machine A
pai-sho daemon
# prints ticket: abc123...

pai-sho expose 8080

# Machine B
pai-sho daemon
pai-sho add-peer abc123...

# Now B can reach A's port 8080 at 127.0.0.1:8080
curl http://127.0.0.1:8080
```

## Build

```sh
cargo build --release
```
