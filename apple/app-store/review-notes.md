# Notes for App Review

Paste into App Store Connect, under App Review Information, Notes. No
sign-in is needed: Pai Sho has no accounts.

---

Pai Sho connects a person's own devices peer to peer and opens web pages
they serve. It needs a second machine to connect to, so we run one for
review.

1. On a computer, open https://pai-sho.ndyg.cross.stream. It shows a QR
   code: an invitation to the demo machine.
2. In Pai Sho, tap the scan button at the top right and scan the code. Allow
   the camera when asked.
3. Type any name, such as "demo", and tap Accept.
4. The demo machine appears with port 47831. When it shows online, tap the
   port. Its page shows 127.0.0.1:47831.

An invitation works once, for 5 minutes. Reload the page for a new one. The
camera is only used to scan invitations. Local network access lets two
devices on the same network connect directly.

Pai Sho is open source, under the MIT licence:
https://github.com/cablehead/pai-sho

It uses iroh, an open-source networking library by number 0:
https://github.com/n0-computer/iroh. Pai Sho is not an official iroh
project. When two devices can't connect directly, their encrypted traffic
goes through number 0's public relays. Acknowledgements, at the bottom of
the app's peer list, lists each library in the app and its licence.
