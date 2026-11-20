#!/usr/bin/env python3
"""Times `skiman sync --hook` with nothing to change (spec Testing Decisions,
"Performance"; Phase 1 experiment 6). Not a CI gate.

Builds a release binary, sets up a scratch HOME with all three agent tools
installed and a canon holding 100 own skills (link mode), runs one sync
to distribute them, then times 10 runs of `skiman sync --hook`, process start
included, and prints the median. Target: under 100 ms. Exits 1 when the
median misses the target or a run does not behave as a no-change run.

Usage: python3 scripts/perf_sync_hook.py   (from anywhere inside the checkout)
"""

import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

SKILLS = 100
RUNS = 10
TARGET_MS = 100.0
TOOL_DIRS = [".claude/skills", ".agents/skills", ".gemini/config/skills"]
PRESENCE_DIRS = [".claude", ".codex", ".gemini"]


def main() -> int:
    checkout = Path(__file__).resolve().parent.parent
    subprocess.run(
        ["cargo", "build", "--release", "--quiet"], cwd=checkout, check=True
    )
    binary = checkout / "target/release/skiman"

    with tempfile.TemporaryDirectory(prefix="skiman-perf-") as tmp:
        root = Path(tmp).resolve()
        home = root / "home"
        for rel in PRESENCE_DIRS + TOOL_DIRS:
            (home / rel).mkdir(parents=True, exist_ok=True)
        git_config = root / "gitconfig"
        git_config.write_text("")
        env = {
            "PATH": os.environ["PATH"],
            "HOME": str(home),
            "XDG_CONFIG_HOME": str(root / "config"),
            "XDG_STATE_HOME": str(root / "state"),
            "GIT_CONFIG_GLOBAL": str(git_config),
            "GIT_CONFIG_NOSYSTEM": "1",
            "LC_ALL": "C",
        }
        repo = root / "canon"

        def skiman(*args: str) -> subprocess.CompletedProcess:
            return subprocess.run(
                [str(binary), *args], env=env, cwd=home, capture_output=True
            )

        setup = skiman("init", "--session-hooks", "none", str(repo))
        if setup.returncode != 0:
            sys.stderr.write(setup.stderr.decode())
            return 1
        for i in range(SKILLS):
            name = f"skill-{i:03}"
            folder = repo / "skills" / name
            folder.mkdir(parents=True)
            (folder / "SKILL.md").write_text(
                f"---\nname: {name}\ndescription: Perf fixture {i}.\n---\n\n# {name}\n"
            )
        setup = skiman("sync")
        if setup.returncode != 0:
            sys.stderr.write(setup.stderr.decode())
            return 1
        for rel in TOOL_DIRS:
            # 100 skills plus skiman-manual.
            assert len(list((home / rel).iterdir())) == SKILLS + 1, rel

        # The first exec of a freshly built binary includes a one-time macOS
        # check (about 60 ms); it is run and reported, not counted.
        first_ms, _ = timed(skiman)
        times = []
        for _ in range(RUNS):
            ms, result = timed(skiman)
            if result.returncode != 0 or result.stdout or result.stderr:
                sys.stderr.write(
                    f"sync --hook was not a silent no-change run: exit {result.returncode}\n"
                    f"stdout: {result.stdout.decode()}\nstderr: {result.stderr.decode()}\n"
                )
                return 1
            times.append(ms)

    median = statistics.median(times)
    print(
        f"skiman sync --hook, {SKILLS} link-mode skills x {len(TOOL_DIRS)} tool directories, nothing to change"
    )
    print(f"runs: {', '.join(f'{t:.2f}' for t in times)} ms (first exec, not counted: {first_ms:.2f} ms)")
    print(f"median of {RUNS}: {median:.2f} ms (min {min(times):.2f}, max {max(times):.2f}); target under {TARGET_MS:.0f} ms")
    return 0 if median < TARGET_MS else 1


def timed(skiman) -> tuple:
    start = time.perf_counter()
    result = skiman("sync", "--hook")
    return (time.perf_counter() - start) * 1000, result


if __name__ == "__main__":
    sys.exit(main())
