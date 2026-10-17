#!/usr/bin/env python3
"""Stack frame of every function of the SBF build, as the compiler reports it (M11).

The SBF toolchain prints a message only when a frame is OVER 4096 bytes (scripts/build-test-so.sh fails on it); it does not print
the sizes. This builds the localnet program with `-Zemit-stack-sizes` (RUSTC_BOOTSTRAP=1 lets the stable platform-tools rustc
take it) into target/stack-sizes/, reads the `.stack_sizes` section of the linked program (one entry per function: its address in the
high half of an 8-byte word, then the frame size as a ULEB128), and names every entry with the symbol table.
Usage: scripts/stack-frames.py [top-N]      (default 20; exit status 1 when a frame is over 4096 bytes)
"""
import glob
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
top = int(sys.argv[1]) if len(sys.argv) > 1 else 20
tools = sorted(glob.glob(os.path.expanduser("~/.cache/solana/*/platform-tools/llvm/bin")))
if not tools:
    sys.exit("the Solana platform tools were not found under ~/.cache/solana")
bin_dir = tools[-1]
target = os.path.join(ROOT, "target", "stack-sizes")
env = dict(os.environ, RUSTC_BOOTSTRAP="1", RUSTFLAGS="-Zemit-stack-sizes")
subprocess.run(
    ["cargo", "build-sbf", "--features", "localnet", "--sbf-out-dir", os.path.join(target, "out"), "--", "--target-dir", os.path.join(target, "build")],
    cwd=os.path.join(ROOT, "programs", "setl8-lev-trading"), env=env, check=True, capture_output=True,
)
so = glob.glob(os.path.join(target, "build", "*", "release", "setl8_lev_trading.so"))[0]
data = open(so, "rb").read()

# section table: find .stack_sizes by name
shoff, shentsize, shnum, shstrndx = (int.from_bytes(data[o:o + n], "little") for o, n in ((0x28, 8), (0x3A, 2), (0x3C, 2), (0x3E, 2)))
def section(i):
    h = data[shoff + i * shentsize: shoff + (i + 1) * shentsize]
    return int.from_bytes(h[0:4], "little"), int.from_bytes(h[24:32], "little"), int.from_bytes(h[32:40], "little")
_, stroff, _ = section(shstrndx)
sec = None
for i in range(shnum):
    name_off, off, size = section(i)
    if data[stroff + name_off: data.index(b"\0", stroff + name_off)] == b".stack_sizes":
        sec = data[off: off + size]
assert sec is not None, ".stack_sizes not found"

def uleb(b, i):
    r = s = 0
    while True:
        x = b[i]; i += 1; r |= (x & 0x7F) << s; s += 7
        if x < 0x80:
            return r, i

entries, i = {}, 0
while i < len(sec):
    addr = int.from_bytes(sec[i:i + 8], "little") >> 32
    size, i = uleb(sec, i + 8)
    entries[addr] = size

# names from the symbol table of the same file
out = subprocess.run([os.path.join(bin_dir, "llvm-objdump"), "-t", "--demangle", so], capture_output=True, text=True).stdout
names = {}
for line in out.splitlines():
    m = re.match(r"^([0-9a-f]{16}) .* \.text\s+[0-9a-f]+ (.+)$", line)
    if m:
        name = re.sub(r"^\.hidden ", "", re.sub(r"::h[0-9a-f]{16}$", "", m.group(2))).strip()
        if name and not name.startswith(".L"):
            names[int(m.group(1), 16)] = name
rows = sorted(((size, names.get(addr, f"(local function at {addr:#x})")) for addr, size in entries.items()), reverse=True)
print(f"{len(rows)} functions with a frame in the localnet build; limit 4096 bytes per frame")
for size, name in rows[:top]:
    print(f"{size:6d}  {name[:150]}")
over = [r for r in rows if r[0] > 4096]
print(f"functions over 4096 bytes: {len(over)}; largest frame {rows[0][0]} bytes")
sys.exit(1 if over else 0)
