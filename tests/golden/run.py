#!/usr/bin/env python3
"""Golden outputs: run linearctl on every case and compare with what the Go
implementation printed at 0.2.0, byte for byte.

    tests/golden/run.py check  ./target/release/linearctl   # exit 1 on any difference
    tests/golden/run.py record ./linearctl                  # rewrite expected/

Each line of cases.jsonl is one invocation: {"args": [...]} with optional
"stdin" (text), "env" ({NAME: value}), "tty" (stdout is a terminal) and
"setup" (files to create first, {relative path: text}). Every case runs offline:
in an empty working directory and LINEARCTL_HOME, with no API key and an API
URL nothing answers, so only help, usage errors, local state and demo mode
(--demo) produce output. Paths of the temporary directories, times and dates
are replaced with <TMP>, <TIME> and <DATE>, since demo times move with now,
and the binary's own version with <VERSION>, so a release needs no re-record.
"""
import json
import os
import pathlib
import pty
import re
import select
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
EXPECTED = HERE / "expected"


def slug(args):
    s = re.sub(r"[^A-Za-z0-9]+", "-", " ".join(args) or "root").strip("-")
    return s[:80] or "root"


def version_of(binary):
    out = subprocess.run([binary, "--version"], capture_output=True, text=True).stdout
    return out.split()[-1]


def run_case(binary, case, version):
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="lc-golden-"))
    try:
        cwd, home = tmp / "cwd", tmp / "home"
        cwd.mkdir()
        for rel, text in case.get("setup", {}).items():
            p = tmp / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(text)
        env = {k: v for k, v in os.environ.items() if not k.startswith(("LINEAR", "XDG_"))}
        env.update({"NO_COLOR": "1", "LINEARCTL_HOME": str(home), "LINEARCTL_API_URL": "http://127.0.0.1:9/graphql",
                    "HOME": str(tmp / "user")})
        env.update(case.get("env", {}))
        argv = [binary] + case["args"]
        stdin = case.get("stdin", "").encode()
        if case.get("tty"):
            out, err, code = run_tty(argv, cwd, env, stdin)
        else:
            p = subprocess.run(argv, cwd=cwd, env=env, input=stdin, capture_output=True, timeout=20)
            out, err, code = p.stdout, p.stderr, p.returncode
        text = f"exit: {code}\n--- stdout\n{out.decode(errors='replace')}\n--- stderr\n{err.decode(errors='replace')}"
        for d in {str(tmp), os.path.realpath(tmp)}:
            text = text.replace(d, "<TMP>")
        text = re.sub(r"(linearctl |\"version\":\s?\")" + re.escape(version) + r"\b", r"\g<1><VERSION>", text)
        text = re.sub(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?Z", "<TIME>", text)
        text = re.sub(r"\b\d{4}-\d\d-\d\d\b", "<DATE>", text)
        return text
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def run_tty(argv, cwd, env, stdin):
    """stdout on a pseudo-terminal, stdin and stderr on pipes."""
    master, slave = pty.openpty()
    p = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.PIPE, stdout=slave, stderr=subprocess.PIPE)
    os.close(slave)
    p.stdin.write(stdin)
    p.stdin.close()
    out = b""
    # Read until the process has exited and the terminal is drained: macOS
    # can report EIO on the master before the last bytes are read.
    while True:
        r, _, _ = select.select([master], [], [], 0.1)
        chunk = b""
        if r:
            try:
                chunk = os.read(master, 65536)
            except OSError:
                chunk = b""
        out += chunk
        if not chunk and p.poll() is not None:
            break
    err = p.stderr.read()
    code = p.wait(timeout=20)
    os.close(master)
    # The terminal turns \n into \r\n, and under load sometimes \r\r\n; linearctl
    # never prints a bare \r itself (printable() replaces it).
    return out.replace(b"\r", b""), err, code


def main():
    if len(sys.argv) != 3 or sys.argv[1] not in ("check", "record"):
        sys.exit(__doc__)
    mode, binary = sys.argv[1], os.path.abspath(sys.argv[2])
    cases = [json.loads(line) for line in (HERE / "cases.jsonl").read_text().splitlines() if line.strip()]
    version = version_of(binary)
    names = []
    for i, case in enumerate(cases):
        name = f"{i:04d}-{slug(case['args'])}"
        names.append(name)
    if mode == "record":
        shutil.rmtree(EXPECTED, ignore_errors=True)
        EXPECTED.mkdir()
        for name, case in zip(names, cases):
            with open(EXPECTED / f"{name}.txt", "w", newline="") as f:
                f.write(run_case(binary, case, version))
        print(f"recorded {len(cases)} cases")
        return
    failed = 0
    for name, case in zip(names, cases):
        with open(EXPECTED / f"{name}.txt", newline="") as f:
            want = f.read()
        got = run_case(binary, case, version)
        if got != want:
            failed += 1
            if failed <= 5:
                import difflib
                diff = difflib.unified_diff(want.splitlines(), got.splitlines(), "expected", "got", lineterm="", n=2)
                print(f"--- {name}: {json.dumps(case)}\n" + "\n".join(list(diff)[:60]))
    print(f"{len(cases) - failed}/{len(cases)} cases match")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
