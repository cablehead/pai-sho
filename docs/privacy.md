# Pai Sho privacy policy

Last updated: November 16, 2026

Pai Sho connects your own devices to each other and forwards ports between
them. It is made by cross.stream (Andy Gayton).

## What we collect

Nothing. Pai Sho has no account, no analytics, no tracking and no
advertising. We run no server that the app reports to, and we never see
what you send between your devices.

## What stays on your device

- **Your device's key.** Pai Sho makes a key pair the first time it starts
  and keeps it on the device. Your peers know your device by its public key.
- **Your peers.** The names you give your peers, their public keys and the
  ports they share with you are kept on the device.
- **Settings.** A few app settings are kept on the device.

Deleting the app deletes all of these.

## What goes over the network

Traffic between your devices is end-to-end encrypted (QUIC with TLS 1.3).
Only the devices you've admitted can read it.

Pai Sho is built on [iroh](https://github.com/n0-computer/iroh), and it
uses the public relay and discovery servers run by number 0, iroh's makers,
so your devices can find each other without a public IP address:

- **Discovery.** Your device publishes its public key and its network
  addresses, signed with its key, so your peers can find it.
- **Relays.** When two devices can't connect directly, a relay passes their
  traffic along. That traffic is already encrypted, so the relay can't read
  it, but it does see the devices' IP addresses.

number 0's handling of that data is described in its own privacy policy at
[n0.computer](https://n0.computer).

## Camera and local network

The camera is used only to scan an invitation's QR code. Nothing it sees
is kept or sent anywhere. Local network access lets your devices connect
directly when they're on the same network.

## Contact

Questions about this policy go to
[github.com/cablehead/pai-sho/issues](https://github.com/cablehead/pai-sho/issues).
