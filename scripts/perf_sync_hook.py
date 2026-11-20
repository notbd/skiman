#!/usr/bin/env python3
"""Times `skiman sync --hook` with nothing to change (spec Testing Decisions,
"Performance"; Phase 1 experiment 6). Not a CI gate.

Builds a release binary and times two scenes, each in a scratch HOME with
all three agent tools installed and a canon holding 100 own skills (link
mode): the skills alone, and the skills plus 2 registered project roots,
each a git repo holding 10 adopted 8 KiB `AGENTS.md` files. After one sync
distributes and mirrors everything, it times 10 runs of `skiman sync --hook`
per scene, process start included, and prints each median. Target: under
100 ms. Exits 1 when a median misses the target or a run does not behave
as a no-change run.

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
ROOTS = 2
ITEMS = 10
ITEM_BYTES = 8192
TARGET_MS = 100.0
TOOL_DIRS = [".claude/skills", ".agents/skills", ".gemini/config/skills"]
PRESENCE_DIRS = [".claude", ".codex", ".gemini"]


def main() -> int:
    checkout = Path(__file__).resolve().parent.parent
    subprocess.run(
        ["cargo", "build", "--release", "--quiet"], cwd=checkout, check=True
    )
    binary = checkout / "target/release/skiman"

    ok = True
    for name, roots in [("skills only", 0), ("skills and project items", ROOTS)]:
        with tempfile.TemporaryDirectory(prefix="skiman-perf-") as tmp:
            result = scene(binary, Path(tmp).resolve(), roots)
        if result is None:
            return 1
        first_ms, times = result
        median = statistics.median(times)
        what = f"{SKILLS} link-mode skills x {len(TOOL_DIRS)} tool directories"
        if roots:
            what += f", {roots} project roots x {ITEMS} adopted items"
        print(f"{name}: skiman sync --hook, {what}, nothing to change")
        print(f"  runs: {', '.join(f'{t:.2f}' for t in times)} ms (first exec, not counted: {first_ms:.2f} ms)")
        print(f"  median of {RUNS}: {median:.2f} ms (min {min(times):.2f}, max {max(times):.2f}); target under {TARGET_MS:.0f} ms")
        ok = ok and median < TARGET_MS
    return 0 if ok else 1


def scene(binary: Path, root: Path, roots: int):
    """Sets up one scene under `root` and times it: the first exec and the
    counted runs, or None (reported) when the setup or a run goes wrong."""
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

    def setup(*args: str) -> bool:
        done = skiman(*args)
        if done.returncode != 0:
            sys.stderr.write(done.stderr.decode())
        return done.returncode == 0

    if not setup("init", "--session-hooks", "none", str(repo)):
        return None
    for i in range(SKILLS):
        name = f"skill-{i:03}"
        folder = repo / "skills" / name
        folder.mkdir(parents=True)
        (folder / "SKILL.md").write_text(
            f"---\nname: {name}\ndescription: Perf fixture {i}.\n---\n\n# {name}\n"
        )
    for r in range(roots):
        project = home / f"work{r}"
        for i in range(ITEMS):
            item = project / f"p{i:02}" / "AGENTS.md"
            item.parent.mkdir(parents=True)
            line = f"root {r} item {i}\n"
            item.write_text((line * (ITEM_BYTES // len(line) + 1))[:ITEM_BYTES])
            assert item.stat().st_size == ITEM_BYTES, item
        subprocess.run(["git", "init", "-q", str(project)], env=env, check=True)
        if not setup("project", "add", "--id", f"work{r}", str(project)):
            return None
    if roots and not setup("project", "adopt", "--yes"):
        return None
    if not setup("sync"):
        return None
    for rel in TOOL_DIRS:
        # 100 skills plus skiman-manual.
        assert len(list((home / rel).iterdir())) == SKILLS + 1, rel
    for r in range(roots):
        mirror = repo / "projects" / f"work{r}"
        assert len(list(mirror.rglob("AGENTS.md"))) == ITEMS, mirror

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
            return None
        times.append(ms)
    return first_ms, times


def timed(skiman) -> tuple:
    start = time.perf_counter()
    result = skiman("sync", "--hook")
    return (time.perf_counter() - start) * 1000, result


if __name__ == "__main__":
    sys.exit(main())
