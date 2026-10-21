#!/usr/bin/env python3
"""Mutation driver for the M12 audit (item 10).
Lists the cargo-mutants mutants of the program (JSON), builds each one in a scratch source copy (cargo build-sbf --features localnet),
and runs the prebuilt tests-rs test binaries against the mutated .so through SETL8_TEST_SO. KILLED = a tier-1 binary fails.
Tier 1 = the adversarial suite + trust matrix; survivors are re-run against tier 2 = every other tests-rs binary.
usage: mutdrv.py run   <tier1 bins comma> <tier2 bins comma> [files-glob]
       (state is kept in results.jsonl next to this script; finished mutants are skipped on restart)"""
import json, os, subprocess, sys, threading, time, queue, shutil

ROOT = '/Users/rovik/Documents/setl8-lev-trading'
SCR = '/private/tmp/claude-501/-Users-rovik-Documents-setl8-lev-trading/c0f6ebc5-9947-4de0-a16c-ee44ef606683/scratchpad/mut'
RES = os.environ.get('MUT_RES') or os.path.join(SCR, 'results.jsonl')
WORKERS = int(os.environ.get('MUT_WORKERS', '3'))
TESTS = os.path.join(ROOT, 'tests-rs')


def listing(glob):
    out = subprocess.run(['cargo', 'mutants', '--list', '--json', '-p', 'setl8-lev-trading', '-f', glob], cwd=os.path.join(SCR, 'n1'), capture_output=True, text=True)
    return json.loads(out.stdout)


def apply(src, span, repl):
    lines = src.split('\n')
    (sl, sc), (el, ec) = (span['start']['line'], span['start']['column']), (span['end']['line'], span['end']['column'])
    if sl == el:
        l = lines[sl - 1]
        lines[sl - 1] = l[:sc - 1] + repl + l[ec - 1:]
    else:
        first, last = lines[sl - 1], lines[el - 1]
        lines[sl - 1:el] = [first[:sc - 1] + repl + last[ec - 1:]]
    return '\n'.join(lines)


def run_bins(bins, so, timeout):
    env = dict(os.environ, SETL8_TEST_SO=so)
    for b in bins:
        try:
            p = subprocess.run([b, '--test-threads', '4'], cwd=TESTS, env=env, capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            return 'timeout', os.path.basename(b)
        if p.returncode != 0:
            failed = [l[5:-9] for l in p.stdout.splitlines() if l.startswith('test ') and l.endswith('FAILED')]
            return 'killed', os.path.basename(b).rsplit('-', 1)[0] + ': ' + '; '.join(failed[:3])
    return 'survived', ''


def worker(wid, q, tier1, tier2, done, lock):
    w = os.path.join(SCR, f'n{wid}')
    out = os.path.join(w, 'out')
    while True:
        try:
            m = q.get_nowait()
        except queue.Empty:
            return
        f = os.path.join(w, m['file'])
        orig = open(f).read()
        t0 = time.time()
        rec = {'name': m['name'], 'file': m['file'], 'genre': m['genre']}
        try:
            open(f, 'w').write(apply(orig, m['span'], m['replacement']))
            b = subprocess.run('cargo build-sbf --offline --features localnet --sbf-out-dir ../../out', shell=True, cwd=os.path.join(w, 'programs/setl8-lev-trading'), capture_output=True, text=True)
            if b.returncode != 0:
                rec.update(status='unviable', detail=(b.stdout + b.stderr)[-200:])
            else:
                so = os.path.join(out, 'setl8_lev_trading.so')
                st, d = run_bins(tier1, so, 900)
                rec.update(status=st, detail=d)
                if st == 'survived':
                    st2, d2 = run_bins(tier2, so, 1800)
                    rec.update(tier2=st2, tier2_detail=d2)
        finally:
            open(f, 'w').write(orig)
        rec['secs'] = round(time.time() - t0)
        with lock:
            with open(RES, 'a') as fh:
                fh.write(json.dumps(rec) + '\n')
            print(rec['status'], rec.get('tier2', ''), m['name'][:110], rec['secs'], 's', flush=True)


def main():
    tier1 = sys.argv[2].split(',')
    tier2 = [x for x in sys.argv[3].split(',') if x]
    glob = sys.argv[4] if len(sys.argv) > 4 else 'programs/setl8-lev-trading/src/instructions/*.rs'
    ms = listing(glob)
    if os.environ.get('MUT_ONLY'):
        import re
        rx = re.compile(os.environ['MUT_ONLY'])
        ms = [m for m in ms if rx.search(m['name'])]
    done = set()
    if os.path.exists(RES):
        done = {json.loads(l)['name'] for l in open(RES)}
    q = queue.Queue()
    for m in ms:
        if m['name'] not in done:
            q.put(m)
    print(f'{len(ms)} mutants, {q.qsize()} to run, {WORKERS} workers', flush=True)
    lock = threading.Lock()
    ts = [threading.Thread(target=worker, args=(i + 1, q, tier1, tier2, done, lock)) for i in range(WORKERS)]
    [t.start() for t in ts]
    [t.join() for t in ts]


main()
