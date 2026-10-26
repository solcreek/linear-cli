#!/usr/bin/env python3
"""Summarize one bench/run.sh result, or compare two as a before/after table.

    bench/compare.py bench/results/go-0.2.0
    bench/compare.py bench/results/go-0.2.0 bench/results/rust-0.3.0
"""
import json
import pathlib
import statistics
import sys


def load(d):
    d = pathlib.Path(d)
    env = dict(line.split(": ", 1) for line in (d / "env.txt").read_text().splitlines())
    rss = dict(line.split() for line in (d / "rss.txt").read_text().splitlines())
    rows = {}
    for f in sorted(d.glob("*.json")):
        r = json.loads(f.read_text())["results"][0]
        t = r["times"]
        rows[f.stem] = {
            "mean": r["mean"] * 1000, "median": statistics.median(t) * 1000, "min": min(t) * 1000,
            "sd": r["stddev"] * 1000, "rss": int(rss.get(f.stem, 0)) / 2**20,
        }
    return env, rows


ORDER = ["version", "help", "issue-list-help", "describe", "issue-list", "issue-show", "demo-issue-list", "serve-100"]


def order(rows):
    return [n for n in ORDER if n in rows] + sorted(n for n in rows if n not in ORDER)


def one(d):
    env, rows = load(d)
    print(f"{env['label']} — {env['version']} at `{env['git']}`, {env['host']}, load {env['load']}, "
          f"{env['runs']} runs each, {env['date']}\n")
    print(f"binary: {int(env['size_bytes']) / 2**20:.2f} MiB\n")
    print("| scenario | median | mean ± σ | min | peak RSS |")
    print("|---|---:|---:|---:|---:|")
    for n in order(rows):
        r = rows[n]
        print(f"| {n} | {r['median']:.2f} ms | {r['mean']:.2f} ± {r['sd']:.2f} ms | {r['min']:.2f} ms | {r['rss']:.1f} MiB |")


def two(a, b):
    ea, ra = load(a)
    eb, rb = load(b)
    for e in (ea, eb):
        print(f"- **{e['label']}**: {e['version']} at `{e['git']}`, {e['host']}, load {e['load']}, {e['runs']} runs, {e['date']}")
    sa, sb = int(ea["size_bytes"]) / 2**20, int(eb["size_bytes"]) / 2**20
    print(f"\n| scenario | {ea['label']} median | {eb['label']} median | speed-up | {ea['label']} RSS | {eb['label']} RSS |")
    print("|---|---:|---:|---:|---:|---:|")
    for n in order(ra):
        if n not in rb:
            continue
        x, y = ra[n], rb[n]
        print(f"| {n} | {x['median']:.2f} ms | {y['median']:.2f} ms | {x['median'] / y['median']:.2f}× "
              f"| {x['rss']:.1f} MiB | {y['rss']:.1f} MiB |")
    print(f"| binary size | {sa:.2f} MiB | {sb:.2f} MiB | {sa / sb:.2f}× smaller | | |")


if __name__ == "__main__":
    {2: lambda: one(sys.argv[1]), 3: lambda: two(sys.argv[1], sys.argv[2])}.get(len(sys.argv), lambda: sys.exit(__doc__))()
