# Pai Sho on the App Store

What goes into App Store Connect for the iPhone and iPad app. Paste each
field as it is. Screenshots are in `screenshots/`: `iphone-6.9` for the
6.9-inch iPhone slot and `ipad-13` for the 13-inch iPad slot. Every other
size is scaled from those.

## App information

- **Name:** Pai Sho
- **Subtitle (30 max):** Your own ports, on your phone
- **Category:** Developer Tools. Secondary: Utilities.
- **Age rating:** answer "None" or "No" to every question. That gives 4+.
- **Copyright:** 2026 cross.stream
- **Support URL:** https://github.com/cablehead/pai-sho/issues
- **Marketing URL:** https://github.com/cablehead/pai-sho
- **Privacy Policy URL:** https://pai-sho.ndyg.cross.stream/privacy

## Promotional text (170 max)

Open web pages served on your own machines from your iPhone or iPad. Peer to
peer, with no account and no open ports.

## Description

Pai Sho opens web pages served on your own machines: a build dashboard, a
dev server, a home server's admin page.

The machine runs pai-sho, a command-line tool, and makes an invitation. Scan
its QR code with the app and give the machine a name. The ports it shares
are listed under that name. Tap one to open its page.

Neither side needs an account, a public IP, or an open inbound port. Devices
connect directly when they can, and through a relay when they can't. Traffic
is end-to-end encrypted with QUIC and TLS 1.3. A port is reachable only by
the devices it's granted to.

Pai Sho has no accounts and collects no data.

To set up a Mac or Linux machine:

    brew install cablehead/tap/pai-sho
    pai-sho daemon (leave it running)
    pai-sho invite --qr --expose 8080 (in another terminal)

Pai Sho is open source, under the MIT licence: github.com/cablehead/pai-sho.
It uses iroh, an open-source networking library by number 0 (n0.computer),
with thanks. Pai Sho is not an official iroh project.

## Keywords (100 max)

port forwarding,p2p,tunnel,localhost,dev server,self-hosted,remote,iroh,quic,homelab

## App Privacy

**Data Not Collected.** Nothing leaves the device for us. The app shares its
public key and network addresses with iroh's discovery servers so peers can
find it; that is the app's function and is not linked to the person.

## Export compliance (each build, by hand)

Asked when a build is submitted:

- Does your app use encryption? **Yes.**
- Does it qualify for an exemption? Choose the standard-encryption answer
  (encryption limited to standard algorithms, not Apple's).
- France stays off under Pricing and Availability until the ANSSI
  declaration is approved.

## Review notes

See `review-notes.md`. The demo machine is the pai-sho site at
https://pai-sho.ndyg.cross.stream (its source is the pai-sho-site repo).
