#!/usr/bin/env python3
"""Offline helper: makes a Cargo `directory` source out of the already EXTRACTED crates in ~/.cargo/registry/src.

Cargo's automatic cache cleaning deletes old `.crate` files from ~/.cargo/registry/cache while the extracted sources stay; `cargo metadata`
(which `cargo build-sbf` runs) then refuses to work offline ("failed to download"). This script copies, for every crates.io package of the
given Cargo.lock, the extracted source directory to <dest>/<name>-<version> and writes `.cargo-checksum.json` with the lock file's own
package checksum (so cargo still ties the source to the exact crate the lock names). No network. Prints the config to put in .cargo/config.toml.

usage: vendor-from-registry-src.py <Cargo.lock> <dest-dir>
"""
import glob, json, os, re, shutil, sys

lock, dest = sys.argv[1], sys.argv[2]
roots = glob.glob(os.path.expanduser("~/.cargo/registry/src/index.crates.io-*"))
if not roots:
    sys.exit("no extracted registry sources found")
text = open(lock).read()
pkgs = re.findall(r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"\n(?:source = "([^"]+)"\n)?(?:checksum = "([0-9a-f]+)"\n)?', text)
os.makedirs(dest, exist_ok=True)
n = missing = 0
for name, ver, source, checksum in pkgs:
    if not source or not source.startswith("registry+"):
        continue
    src = next((os.path.join(r, f"{name}-{ver}") for r in roots if os.path.isdir(os.path.join(r, f"{name}-{ver}"))), None)
    if src is None:
        print(f"MISSING extracted source: {name}-{ver}", file=sys.stderr)
        missing += 1
        continue
    d = os.path.join(dest, f"{name}-{ver}")
    if os.path.exists(d):
        shutil.rmtree(d)
    shutil.copytree(src, d)
    for junk in (".cargo-ok",):
        p = os.path.join(d, junk)
        if os.path.exists(p):
            os.remove(p)
    json.dump({"files": {}, "package": checksum}, open(os.path.join(d, ".cargo-checksum.json"), "w"))
    n += 1
print(f"vendored {n} crates into {dest}, {missing} missing", file=sys.stderr)
sys.exit(1 if missing else 0)
