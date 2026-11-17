# Notes for App Review

Paste into App Store Connect, under App Review Information, Notes. No
sign-in is needed: Pai Sho has no accounts.

---

Pai Sho connects a person's own devices peer to peer, and opens the web
pages a connected machine shares. It needs a second machine to connect to,
so we run a demo machine for review.

1. On a computer, open https://pai-sho.ndyg.cross.stream. It shows a QR
   code with a fresh invitation to the demo machine.
2. In Pai Sho on the iPhone or iPad, tap the scan button at the top and
   scan the code. Allow the camera when asked.
3. Enter any name, such as "demo", and tap Accept.
4. The demo machine appears with port 47831. Once it shows as online, tap
   the port to open its page, which shows 127.0.0.1:47831.

Each invitation works once and expires after 5 minutes. Reload the page
for a new one. The camera is used only to scan invitations, and local
network access lets two devices on the same network connect directly.

Pai Sho is open source, under the MIT licence:
https://github.com/cablehead/pai-sho. It is built on iroh, the open-source
peer-to-peer networking library made by number 0:
https://github.com/n0-computer/iroh and https://n0.computer. When two
devices can't reach each other directly, the encrypted connection goes
through number 0's public relay servers. The app credits both under the
peer list, with Acknowledgements listing every open-source library it
includes and its licence.
