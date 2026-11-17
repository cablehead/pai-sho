"""Writes apple/PaiSho/Acknowledgements.txt: every crate linked into the iOS
app, with its licence and the licence texts its source ships.

Run from the repo root after a dependency changes:
    python3 apple/tools/acknowledgements.py
"""
import glob, json, os, subprocess, sys

TARGET = "aarch64-apple-ios"
OUT = "apple/PaiSho/Acknowledgements.txt"

meta = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET]))
packages = {p["id"]: p for p in meta["packages"]}
nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
root = meta["resolve"]["root"]

# Crates reached from pai-sho through normal (not build or dev) dependencies:
# the code that ends up in the app.
seen, todo = set(), [root]
while todo:
    pid = todo.pop()
    if pid in seen:
        continue
    seen.add(pid)
    for dep in nodes[pid]["deps"]:
        if any(k["kind"] is None for k in dep["dep_kinds"]):
            todo.append(dep["pkg"])

def licence_files(pkg):
    d = os.path.dirname(pkg["manifest_path"])
    names = []
    for pat in ("LICEN[CS]E*", "COPYING*", "NOTICE*", "licen[cs]e*"):
        names += glob.glob(os.path.join(d, pat))
    if pkg.get("license_file"):
        names.append(os.path.join(d, pkg["license_file"]))
    files = sorted({n for n in names if os.path.isfile(n)})
    return [(os.path.basename(f), open(f, encoding="utf-8", errors="replace").read().strip()) for f in files]

crates = sorted((packages[p] for p in seen if p != root), key=lambda p: (p["name"], p["version"]))
# The intro is shown as one paragraph per line, so it isn't wrapped here.
out = ["Pai Sho is open source, under the MIT licence: https://github.com/cablehead/pai-sho",
       "",
       "It uses iroh, an open-source networking library by number 0 (https://n0.computer), with thanks: https://github.com/n0-computer/iroh. Pai Sho is not an official iroh project.",
       "",
       "Below is each Rust crate in the app, with its licence.",
       ""]
missing = []
shown = {}  # licence text -> the crate whose entry shows it in full
for p in crates:
    out.append("=" * 72)
    out.append(f"{p['name']} {p['version']}")
    out.append(f"Licence: {p.get('license') or 'see below'}")
    if p.get("repository"):
        out.append(p["repository"])
    files = licence_files(p)
    if not files:
        missing.append(p["name"])
    for name, text in files:
        out.append("")
        if text in shown:
            out.append(f"--- {name}: the same text as under {shown[text]}")
            continue
        shown[text] = f"{p['name']} {p['version']}"
        out.append(f"--- {name}")
        out.append(text)
    if not files:
        out.append("This crate ships no licence file. Its repository has the licence.")
    out.append("")
open(OUT, "w").write("\n".join(out) + "\n")
print(f"{len(crates)} crates, {os.path.getsize(OUT)} bytes, no licence file: {', '.join(missing) or 'none'}", file=sys.stderr)
