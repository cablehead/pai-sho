# Host-Attested Enrollment

## Context

[ADR 0002](0002-token-enrollment.md) has the operator trust a workload by
**token**: a one-time secret, minted on the laptop and shipped into the VM,
which the workload presents on first connect. The token's whole job is to
bootstrap one moment: the operator's first "this key is `rustdev`".

Two problems pushed us to revisit it:

- **The token leaks by construction.** As we move a workload's boot config off
  a shared filesystem and onto the kernel cmdline, the token would sit in
  `/proc/cmdline`, readable by every process in the guest, and in the host's
  `ps`, journal, and shell history. A bearer secret is exactly the thing you
  don't want in those places.
- **A name is not a credential.** "First peer to claim the label wins" is a
  race: the label is public, so nothing gates it.

Prior art points one way. SPIRE, Kubernetes kubelet bootstrap, and cloud
instance identity all bind trust to a **key the node generates itself**, vouched
for by something already trusted, never a secret shipped into the node. The
token is the leakable option those systems warn against; key-vouching is the
one they reach for.

## Decision

**The host vouches for the workload's key.**

1. The workload generates its keypair at boot and persists it (`--key`).
2. It reports its **public** key to the host over vsock.
3. The launcher relays that exact key to the operator over the link it already
   holds (ssh to the host, the operator's local socket), saying: *pin this key
   as `rustdev`*.
4. The operator accepts that one key. Anything else dialing in is refused.

No secret exists to leak. The slug rides the cmdline in the open. It's a label,
not a credential. There is no race: the operator only ever accepts the key the
host named, so an attacker's key is simply not the pinned one.

The worked flow:

```
# workload at boot: nothing secret on the cmdline
pai-sho daemon -a <operator-ticket> -e 42000 --slug rustdev --key /var/lib/vibenv/key
        (operator-ticket is a public node id; slug is a public label)

# guest -> host, over vsock
"my key is kW"

# launcher -> operator, over its trusted link
pai-sho pin kW --slug rustdev
```

This supersedes the token half of ADR 0002. The other half, the workload
trusting the operator by its stable ticket (`-a <operator-ticket>`), stands
unchanged.

## Tradeoffs

- **Needs a trusted host-to-operator channel.** We have one: the launcher runs on
  the laptop, reaches the host over ssh, and reaches the operator over its local
  socket. The binding travels that path, not through the guest.
- **If that channel were ever untrusted for integrity**, the fallback is a PAKE
  (SPAKE2, as in Magic Wormhole / Matter pairing): workload and operator derive
  agreement from a value the launcher seeds into both, with no bearer token and
  no offline attack. It's strictly more machinery, worth it only without a
  trusted host link, so not what we build now.
- **Needs vsock** (guest to host): one round trip at boot. Cloud Hypervisor
  provides it and the guest kernel already has `CONFIG_VIRTIO_VSOCKETS`.
- **The host is trusted.** The threat model already assumes a compromised host
  can read guest RAM, so no channel hides a secret from a bad host regardless.
  This design does not try to; it removes the leakable bearer secret and closes
  the network enrollment race.

## Note on names

This ADR was written when the commands were `pin <key> --slug <name>` and
`daemon --slug`. What shipped is `invite <key> --as <name>`, and the workload
side is `daemon --accept <key>`. The decision is unchanged: the host vouches for
a key the workload generated, and nothing secret travels into the guest. See
docs/scenarios.md for the current flow.
