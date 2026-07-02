# Task: pai-sho enrollment for vibenvs

Own the **pai-sho side** of this goal:

> Boot a workload VM → it phones home to my laptop → I can reach its terminal
> (and a filedrop) with no per-VM step — and no stranger or sibling VM can.

The rest (filedrop, cred delivery, launcher) is built separately, against the
**contract** below. Design from [ADR 0001](../adr/0001-directed-grants.md); land
in slices (see PR #1's checklist). Branch `adr/authorized-mesh` — pull first.

## What you own

1. **Stable operator identity.** The laptop daemon persists its key, so its ticket
   `kL` is constant across restarts.
2. **Enrollment by token.** `pai-sho grant-token --label <name>` mints a one-time,
   5-minute secret. A workload presents it on first connect; the laptop pins the
   workload's key under that label and consumes the token. Unclaimed → expires.
3. **Serve only the operator.** A workload exposes its ports to `kL` and no one
   else. A stranger or sibling VM that dials in gets nothing — refused, no
   announcement, no tunnel. This is the whole point: the VMs are untrusted.
4. **Phone-home.** A workload boots dialing the laptop (`-a kL`), enrolls with its
   seeded token, and exposes its ports to the operator.

## The contract (what the vibenv side codes against)

```
# laptop, once
pai-sho ticket                        → kL      (stable across restarts)
pai-sho grant-token --label rustdev   → TOKEN   (one-time, 5 min)

# workload at boot (seeded with kL + TOKEN by the launcher)
pai-sho daemon -a kL -e 42000,7777 --enroll TOKEN

# laptop, after it connects
pai-sho list   → workload shows up (name "rustdev") with a local binding per
                 exposed port; reachable by me alone.
```

Keep this shape stable: the launcher seeds `kL` + `TOKEN`; the operator reaches
each workload at its `list` binding. Exact flag names are your call — keep `list`
JSON with the current field names.

## Done when

- A workload boots, phones home with a token, and appears in the operator's `list`
  under its label — no manual key exchange.
- The operator can reach its exposed ports; a second workload cannot see the
  first; an un-enrolled peer gets nothing.
- Reused or expired tokens fail. The operator key survives restart (ticket
  unchanged).
- `cargo check` clean.

## House rules

- JSON control plane; keep the nice field names.
- Commit per stable change (`cargo check` passing), push to `adr/authorized-mesh`.
- No `Co-Authored-By` trailer — the repo's `.claude/settings.json` handles it.
- Docs reader-first, example-led, no wall of words.

## Status: shipped, verified end-to-end (2026-07-02)

All four "done when" points hold, tested on real hardware (Hetzner host + a
dedicated operator pai-sho daemon on the laptop, its own socket + key so it
never touches an unrelated personal daemon):

- Rebuilt the guest-side pai-sho binary from this branch, baked it into
  `vibenv-base.img`.
- `init2` dials home (`-a $OPERATOR_TICKET --enroll $ENROLL_TOKEN`) and persists
  its own key to `/session/vibenv/pai-sho.key`.
- `ch-launch [slug] [toolchain] [token]` seeds both into the session env.
- `vibenv-launch.sh` (new, laptop repo) mints the token, calls `ch-launch` over
  SSH, polls until enrolled — the actual entry point now.
- Test: `./vibenv-launch.sh phonehome-test rust` → vibenv booted, dialed home,
  showed up in the operator's `list` under its label, port auto-bound, confirmed
  live via a real HTTP request. Zero manual `add-peer` / ticket copying.

Contract held exactly as written — no changes needed on the pai-sho side to
consume it. Full build/verification log: `vibenv-layers.md` in the infra repo.
