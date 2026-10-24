#!/usr/bin/env python3
"""Checks that the .rs files changed between two revisions differ ONLY in comments (and whitespace).

usage: scripts/rs-comments-only.py <old-rev> <new-rev> [path ...]

For every .rs file under the given paths (default: crates programs tests-rs tools) that differs between the revisions, strips line comments,
(nested) block comments and doc comments, keeps string, raw-string and char literals intact, collapses whitespace, and compares the
token streams. Prints one line per changed file and OK/FAIL; exit status 1 on any difference. A file added or removed is a FAIL.
"""
import re, subprocess, sys

def show(rev, path):
    r = subprocess.run(["git", "show", f"{rev}:{path}"], capture_output=True)
    return r.stdout.decode("utf-8") if r.returncode == 0 else None

def strip(src):
    out, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        two = src[i:i+2]
        if two == "//":
            while i < n and src[i] != "\n": i += 1
        elif two == "/*":
            depth = 1; i += 2
            while i < n and depth:
                if src[i:i+2] == "/*": depth += 1; i += 2
                elif src[i:i+2] == "*/": depth -= 1; i += 2
                else: i += 1
            out.append(" ")
        elif c == "r" and re.match(r'r#*"', src[i:i+10]) and (i == 0 or not (src[i-1].isalnum() or src[i-1] == "_")):
            m = re.match(r'r(#*)"', src[i:]); h = m.group(1); end = '"' + h
            j = src.find(end, i + len(m.group(0)))
            j = n if j < 0 else j + len(end)
            out.append(src[i:j]); i = j
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            out.append(src[i:j+1]); i = j + 1
        elif c == "'":
            m = re.match(r"'(\\.[^']*|[^\\'])'", src[i:])
            if m: out.append(m.group(0)); i += len(m.group(0))
            else: out.append(c); i += 1
        else:
            out.append(c); i += 1
    return " ".join("".join(out).split())

def main():
    old, new = sys.argv[1], sys.argv[2]
    paths = sys.argv[3:] or ["crates", "programs", "tests-rs", "tools"]
    names = subprocess.run(["git", "diff", "--name-only", old, new, "--"] + paths, capture_output=True, text=True).stdout.split()
    bad = 0
    for p in names:
        if not p.endswith(".rs"):
            print(f"  non-rs file changed: {p}"); bad += 1; continue
        a, b = show(old, p), show(new, p)
        if a is None or b is None:
            print(f"  FAIL {p}: added or removed"); bad += 1
        elif strip(a) == strip(b):
            print(f"  comment-only {p}")
        else:
            print(f"  FAIL {p}: token streams differ"); bad += 1
    print("TOKEN-STREAM CHECK:", "OK" if bad == 0 else "FAIL", f"({len(names)} files changed)")
    sys.exit(1 if bad else 0)

main()
