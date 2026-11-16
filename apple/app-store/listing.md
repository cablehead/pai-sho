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
- **Privacy Policy URL:** https://github.com/cablehead/pai-sho/blob/main/docs/privacy.md

## Promotional text (170 max)

Open the web apps running on your own machines from your phone. No account,
no public IP, no open ports: just scan an invitation.

## Description

Pai Sho opens the services running on your own machines, such as a build
dashboard, a dev server or a home server's admin page, right on your iPhone
or iPad. It connects your devices to each other directly, wherever they are.

Neither side needs an account, a public IP address or an open inbound port.
The machine you want to reach runs pai-sho, the free command-line tool, and
makes an invitation. Scan its QR code with Pai Sho, give the machine a name,
and the ports it shares show up in the app. Tap one to open its page.

- Peer to peer: your devices connect directly when they can, and through an
  encrypting relay when they can't.
- Private by design: a port is reachable only by the devices it is granted
  to. A connection from any other device is refused.
- End-to-end encrypted with QUIC and TLS 1.3. Nobody in between can read
  your traffic, including us.
- No account and no tracking. Pai Sho collects nothing.

Install pai-sho on a Mac or Linux machine with `brew install
cablehead/tap/pai-sho` or `cargo install pai-sho`, then run `pai-sho invite
--qr --expose 8080` to share port 8080 with your phone.

Pai Sho is built on iroh, the open-source peer-to-peer networking library.

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
