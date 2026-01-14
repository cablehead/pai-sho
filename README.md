# pai-sho

P2P TCP port forwarding over [iroh](https://github.com/n0-computer/iroh).

## Status

early sketch. currently vibe coded. seems to work.

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
daemon [options]        Start the daemon
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
| `-a, --add` | | Add peer on startup (repeatable) |
| `-e, --expose` | | Expose port on startup (repeatable) |

## Example

```sh
# Machine A - expose port 8080
pai-sho daemon -e 8080
# prints ticket: abc123...

# Machine B - connect to A
pai-sho daemon -a abc123...

# Now B can reach A's port 8080 at 127.0.0.1:8080
curl http://127.0.0.1:8080
```
