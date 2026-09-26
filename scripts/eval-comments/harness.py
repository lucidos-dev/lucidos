#!/usr/bin/env python3
"""Comment-ablation eval: do justifying comments make coding agents pick worse fixes?

Driven by eval-comments.sh. Subcommands:

  prepare [--tasks T1,T2] [--arms A,C] [--verify]
      Build each task's pruned clone and one base commit per arm. --verify
      proves arm C still builds and passes, and that the withheld test fails
      on the base and passes on the real fix.
  run [--tasks ...] [--arms A,C] [--runs 2] [--seed 7] [--runs-dir runs]
      Run the sessions, at most SLOTS at a time, then grade each one.
  grade --run r01 [--runs-dir runs]      (re)grade one finished run
  report [--runs-dir runs]               per-run table and per-arm summary
  status [--runs-dir runs]

Layout. Everything the agents never see lives under DATA_ROOT
(~/.lucidos/data/comment-ablation): task clones, transcripts, diffs, grades.
The agents work under SANDBOX_ROOT (~/.lucidos/data/sandboxes), whose paths
name nothing about the experiment, so the working directory cannot prime them.

Isolation. A task clone is fetched by sha from this repo, so it holds the
fix's parent and its history and nothing newer, and it has no remote. Each
arm is one rewritten tip commit carrying the parent's message, author and
dates. Each run clones exactly one arm branch, renames it `work`, and drops
the remote and reflog. An agent in arm A cannot find arm C, or the fix.

Spending. Every session is a real model run. Nothing here is reachable from
`make test`, `/harden` or a workflow.
"""

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import os
import random
import re
import shutil
import signal
import statistics
import subprocess
import sys
import threading
import time
from collections import deque
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import strip_comments  # noqa: E402

REPO = Path(subprocess.check_output(["git", "-C", str(HERE), "rev-parse", "--show-toplevel"], text=True).strip())
DATA_ROOT = Path(os.environ.get("COMMENT_EVAL_ROOT", Path.home() / ".lucidos/data/comment-ablation"))
SANDBOX_ROOT = Path(os.environ.get("COMMENT_EVAL_SANDBOXES", Path.home() / ".lucidos/data/sandboxes"))
TASKS_DIR = HERE / "tasks"

MODEL = "claude-opus-5-5[1m]"
EFFORT = "high"
# claude-opus-5 (the bare id) and every Fable model are refused outright.
REFUSED_MODEL_RE = re.compile(r"^claude-opus-5($|@|\[)|fable", re.IGNORECASE)

ARMS = {"A": None, "B": "all", "C": "justifying"}
SLOTS = 2
CAP_SECS = 45 * 60
TEST_TIMEOUT_SECS = 40 * 60
CARGO_JOBS = "6"

# Memory, with the same "available" definition as scripts/lib/host_memory_guard.sh.
LAUNCH_FLOOR_GB = 10.0
KILL_FLOOR_GB = 3.0
KILL_SAMPLES = 2
SAMPLE_SECS = 15
LAUNCH_STAGGER_SECS = 45

STRIP_ROOTS = ("crates/", "packages/")
# Bump when stripping changes what an arm tree contains. v2: rustfmt after stripping.
STRIP_VERSION = 2
# The `#[ignore]`d tests that rewrite crates/lucidos-app/src/generated/. Their
# names follow `generate_*_file`, and the set differs by commit.
GENERATOR_RE = re.compile(r"fn (generate_[a-z0-9_]+_file)\b")

AGENT_PREAMBLE = (
    "A tester filed the bug report below against this checkout of Lucidos. Please fix it.\n\n"
    "Engine tests run with `./scripts/test-engine.sh`. Do not run the e2e scripts or start "
    "Lucidos itself: this machine is short on memory, and those commands are blocked.\n\n"
    "This session is headless: nothing wakes you after you end your turn. Run commands in the "
    "foreground and finish the fix before you reply.\n\n"
    "When you are done, reply with a short summary of what you changed and why.\n\n"
    "## Bug report\n\n"
)

AGENT_DENY = (
    "Bash(*e2e*)",
    "Bash(*web-dev*)",
    "Bash(*tauri-dev*)",
    "Bash(*scripts/run.sh*)",
    "Bash(*scripts/start.sh*)",
    "Bash(*scripts/stop.sh*)",
    "Bash(*scripts/restart.sh*)",
    "Bash(*--fresh*)",
    "Bash(docker *)",
    "Bash(*pkill*)",
    "Bash(*killall*)",
    "Bash(kill *)",
    "Bash(git push*)",
    "WebFetch",
    "WebSearch",
)

# The only variables passed through. Everything else is dropped, notably PG*,
# DATABASE_URL, the `lucidos` CLI and the parent session's messaging variables.
ENV_PASSTHROUGH = ("HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "LC_ALL")

# Model access is Vertex with the machine's gcloud application-default
# credentials, the same path a plain `claude` uses. It never borrows a Lucidos
# session relay, so every window of a long run gets the same environment (I4).
VERTEX_DEFAULTS = {
    "CLOUD_ML_REGION": "europe-west1",
    "VERTEX_REGION_CLAUDE_5_5_OPUS": "eu",
    "VERTEX_REGION_CLAUDE_5_SONNET": "eu",
}
MODEL_ENV_FIXED = {
    "CLAUDE_CODE_USE_VERTEX": "1",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": MODEL,
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-5[1m]",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "claude-haiku-4-5",
    "CLAUDE_CODE_ENABLE_TELEMETRY": "0",
    "DISABLE_ERROR_REPORTING": "1",
    "CLAUDE_STREAM_IDLE_TIMEOUT_MS": "1800000",
}

TEST_PATH_RE = re.compile(r"(_tests?\.rs$|_tests/|/tests?/|/tests\.rs$|\.test\.tsx?$|/__tests__/)")
CITE_RES = (
    re.compile(r"\b(the|this|that|a|an|existing|nearby|doc)\s+comments?\b", re.I),
    re.compile(r"\bcomments?\s+(says?|said|explains?|notes?|states?|argues?|claims?|warns?|mentions?)\b", re.I),
)
JUSTIFY_WORD_RE = re.compile(
    r"\b(deliberately|on purpose|intentional(ly)?|load[- ]bearing|by design|best[- ]effort|"
    r"acceptable|harmless|not worth|good enough|workaround)\b",
    re.I,
)
ARM_MARKERS = ("arm-A", "arm-B", "arm-C", "comment-ablation", "strip_comments")

_print_lock = threading.Lock()


def log(msg: str) -> None:
    with _print_lock:
        print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


def sh(args, cwd=None, env=None, check=True, capture=True, timeout=None, input=None) -> subprocess.CompletedProcess:
    return subprocess.run(
        args, cwd=cwd, env=env, check=check, text=True, timeout=timeout, input=input,
        stdout=subprocess.PIPE if capture else None, stderr=subprocess.STDOUT if capture else None,
    )


def git(*args, cwd=None, check=True, env=None) -> str:
    return sh(["git", *args], cwd=cwd, check=check, env=env).stdout


def read_json(path: Path, default=None):
    return json.loads(path.read_text()) if path.exists() else default


def write_json(path: Path, data) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(data, indent=1, sort_keys=True))
    tmp.replace(path)


def claude_bin() -> str:
    """The real CLI. An interactive shell may wrap `claude` in a function."""
    explicit = os.environ.get("COMMENT_EVAL_CLAUDE")
    found = explicit or shutil.which("claude") or str(Path.home() / ".local/bin/claude")
    if not Path(found).exists():
        sys.exit(f"no claude binary at {found}")
    return found


def check_model(model: str) -> None:
    if REFUSED_MODEL_RE.search(model):
        sys.exit(f"refused model {model!r}")


# ---------------------------------------------------------------- tasks


def load_tasks(ids: list[str] | None) -> list[dict]:
    tasks = []
    for d in sorted(TASKS_DIR.iterdir()):
        if not (d / "task.json").exists() or (ids and d.name not in ids):
            continue
        t = json.loads((d / "task.json").read_text())
        t["dir"] = d
        t["prompt"] = (d / "prompt.md").read_text()
        tasks.append(t)
    if ids and len(tasks) != len(ids):
        sys.exit(f"unknown task in {ids}")
    return tasks


def task_root(task: dict) -> Path:
    return DATA_ROOT / "tasks" / task["id"]


def parent_of(task: dict) -> str:
    return git("rev-parse", f"{task['fix']}~1", cwd=REPO).strip()


# ---------------------------------------------------------------- memory


def available_gb() -> float:
    out = sh(["vm_stat"]).stdout
    page = int(re.search(r"page size of (\d+)", out).group(1))

    def pages(label: str) -> int:
        m = re.search(rf"^{label}:\s+(\d+)", out, re.M)
        return int(m.group(1)) if m else 0

    total = pages("Pages free") + pages("Pages speculative") + pages("Pages purgeable") + pages("File-backed pages")
    return total * page / 2**30


def pressure_level() -> str:
    return sh(["sysctl", "-n", "kern.memorystatus_vm_pressure_level"], check=False).stdout.strip()


def wait_for_memory(label: str, need_secs: float) -> bool:
    """Block until a launch is safe. False when the window closes first."""
    while True:
        avail = available_gb()
        if avail >= LAUNCH_FLOOR_GB:
            return True
        if not WINDOW.fits(need_secs + 60):
            return False
        log(f"{label}: waiting for memory, {avail:.1f} GB available (need {LAUNCH_FLOOR_GB})")
        time.sleep(60)


DISK_FLOOR_GB = 40


def free_disk_gb() -> float:
    st = os.statvfs(Path.home())
    return st.f_bavail * st.f_frsize / 2**30


def drop_target(tree: Path) -> None:
    """A tree's target/ is about 12 GB; it goes once nothing will build there again."""
    shutil.rmtree(tree / "target", ignore_errors=True)


# ---------------------------------------------------------------- environment


LUCIDOS_STUB = "#!/bin/sh\necho 'lucidos: not available in this checkout' >&2\nexit 127\n"


def install(path: Path, content: str) -> None:
    """Write an executable only when it changed, by rename, so a reader never sees half a file."""
    if path.exists() and path.read_text() == content:
        return
    tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    tmp.write_text(content)
    tmp.chmod(0o755)
    tmp.replace(path)


def shim_dir() -> Path:
    """The directory put first on every sandbox PATH: the cargo shim, and a
    `lucidos` stub that shadows the real CLI without hiding its neighbours."""
    d = SANDBOX_ROOT / ".bin"
    d.mkdir(parents=True, exist_ok=True)
    install(d / "cargo", (HERE / "cargo_shim.py").read_text())
    install(d / "lucidos", LUCIDOS_STUB)
    return d


def clean_path() -> str:
    return os.pathsep.join([str(shim_dir())] + [p for p in os.environ.get("PATH", "").split(os.pathsep) if p])


def tool_env(wait_log: Path | None = None) -> dict:
    """What cargo and the test scripts need. No model credentials."""
    env = {k: os.environ[k] for k in ENV_PASSTHROUGH if k in os.environ}
    env.update(
        PATH=clean_path(),
        CARGO_BUILD_JOBS=CARGO_JOBS,
        CARGO_LOCK_FILE=str(SANDBOX_ROOT / ".cargo-build.lock"),
    )
    if wait_log:
        env["CARGO_LOCK_WAIT_LOG"] = str(wait_log)
    return env


def model_env(wait_log: Path | None = None) -> dict:
    """tool_env plus Vertex access, for the agents and the judge."""
    project = os.environ.get("ANTHROPIC_VERTEX_PROJECT_ID") or os.environ.get("PROJECT_ID")
    if not project:
        sys.exit("set ANTHROPIC_VERTEX_PROJECT_ID (or PROJECT_ID) to the Vertex project")
    env = tool_env(wait_log)
    env.update(MODEL_ENV_FIXED)
    env.update({k: os.environ.get(f"COMMENT_EVAL_{k}", v) for k, v in VERTEX_DEFAULTS.items()})
    env.update(ANTHROPIC_VERTEX_PROJECT_ID=project, CLAUDE_CONFIG_DIR=str(SANDBOX_ROOT / ".claude-home"))
    return env


def agent_argv(prompt: str) -> list[str]:
    """The one command line every session uses, in every arm (I4)."""
    check_model(MODEL)
    return [
        claude_bin(), "-p", AGENT_PREAMBLE + prompt,
        "--model", MODEL, "--effort", EFFORT,
        "--output-format", "stream-json", "--verbose",
        "--no-session-persistence",
        "--setting-sources", "project",
        "--dangerously-skip-permissions",
        "--disallowedTools", *AGENT_DENY,
    ]


# ---------------------------------------------------------------- prepare


def commit_like(repo: Path, like: str, tree: str) -> str:
    """`like`'s raw commit object with only the tree swapped.

    Parents, author, dates and message stay byte-identical, so an unchanged
    tree reproduces `like` exactly. A signature would no longer verify, so it
    is dropped rather than left to reveal the rewrite.
    """
    raw = git("cat-file", "commit", like, cwd=repo)
    header, _, message = raw.partition("\n\n")
    kept, in_sig = [], False
    for line in header.split("\n"):
        if line.startswith("gpgsig"):
            in_sig = True
            continue
        if in_sig and line.startswith(" "):
            continue
        in_sig = False
        kept.append(f"tree {tree}" if line.startswith("tree ") else line)
    body = "\n".join(kept) + "\n\n" + message
    return sh(["git", "hash-object", "-t", "commit", "-w", "--stdin"], cwd=repo, input=body).stdout.strip()


def ensure_clone(task: dict) -> Path:
    clone = task_root(task) / "src.git"
    parent = parent_of(task)
    if not clone.exists():
        clone.parent.mkdir(parents=True, exist_ok=True)
        git("init", "-q", "--bare", str(clone))
        log(f"{task['id']}: fetching {parent[:10]} into a pruned clone")
        git("fetch", "-q", "--no-tags", str(REPO), f"{parent}:refs/heads/parent", cwd=clone)
    # I1: the fix is unreachable and nothing points back at this repo.
    if sh(["git", "cat-file", "-e", task["fix"]], cwd=clone, check=False).returncode == 0:
        sys.exit(f"{task['id']}: the fix commit is present in the task clone")
    if git("remote", cwd=clone).strip():
        sys.exit(f"{task['id']}: the task clone has a remote")
    return clone


def worktree(clone: Path, path: Path, rev: str) -> Path:
    if path.exists():
        git("worktree", "remove", "--force", str(path), cwd=clone, check=False)
        shutil.rmtree(path, ignore_errors=True)
    git("worktree", "prune", cwd=clone)
    git("worktree", "add", "-q", "--detach", str(path), rev, cwd=clone)
    return path


def leak_files(task: dict) -> list[str]:
    names = git("diff", "--name-only", parent_of(task), task["fix"], "--", "docs/plans", cwd=REPO).split()
    present = git("ls-tree", "-r", "--name-only", parent_of(task), "--", "docs/plans", cwd=REPO).split()
    return [n for n in names if n in present]


def strip_tree(tree: Path, mode: str) -> dict:
    files = [
        f for f in git("ls-files", cwd=tree).splitlines()
        if f.startswith(STRIP_ROOTS) and strip_comments.language_of(f) and "/generated/" not in f
    ]
    summary = {}
    for rel in files:
        path = tree / rel
        src = path.read_text(encoding="utf-8")
        lang = strip_comments.language_of(rel)
        out, removed = strip_comments.strip(src, lang, mode)
        if not removed:
            continue
        # I3: only comments changed.
        if strip_comments.tokens_without_comments(src, lang) != strip_comments.tokens_without_comments(out, lang):
            sys.exit(f"stripping changed a code token in {rel}")
        path.write_text(out, encoding="utf-8")
        summary[rel] = removed
    format_stripped(tree, summary)
    return summary


def format_stripped(tree: Path, summary: dict) -> None:
    """Make the stripped Rust files rustfmt-clean, as the parent tree was.

    Deleting a comment block can leave a leading or doubled blank line, and an
    agent running `make lint` in arm C would then meet fmt failures arm A never
    shows. rustfmt may also reorder imports once a comment between them is gone;
    a file whose code tokens move (trailing commas aside) is put back unformatted. A file rustfmt
    touches that stripping did not is put back as the parent had it.
    """
    stripped = {rel: (tree / rel).read_text(encoding="utf-8") for rel in summary if rel.endswith(".rs")}
    if not stripped:
        return
    sh(["cargo", "fmt", "--all"], cwd=tree, env=tool_env(), check=False)
    for rel in git("diff", "--name-only", cwd=tree).splitlines():
        if rel not in summary:
            git("checkout", "-q", "--", rel, cwd=tree)
            continue
        if rel not in stripped:
            continue
        formatted = (tree / rel).read_text(encoding="utf-8")
        if code_shape(formatted) != code_shape(stripped[rel]):
            (tree / rel).write_text(stripped[rel], encoding="utf-8")


def code_shape(src: str) -> list:
    """Rust tokens minus commas. rustfmt adds and drops trailing commas when it
    joins or splits lines, which means nothing; any other change is a reorder."""
    return [t for t in strip_comments.tokens_without_comments(src, "rust") if t != ("punct", ",")]


class WindowEnded(Exception):
    """Raised when the next step would not fit in this invocation's window."""


class Window:
    """A wall-clock budget for one invocation.

    The engine kills a background task after an hour, so a long run is a chain
    of windows. Each step asks whether it fits before it starts, and every
    subprocess timeout is clamped to the window, so nothing outlives it.
    """

    def __init__(self, minutes: float | None):
        self.deadline = time.time() + minutes * 60 if minutes else None

    def remaining(self) -> float:
        return float("inf") if self.deadline is None else self.deadline - time.time()

    def fits(self, secs: float) -> bool:
        return self.remaining() >= secs

    def need(self, secs: float, what: str) -> None:
        if not self.fits(secs):
            raise WindowEnded(what)

    def clamp(self, secs: float) -> float:
        return max(1.0, min(secs, self.remaining() - 60))


WINDOW = Window(None)
EXIT_RESUME = 75  # EX_TEMPFAIL: work remains, invoke again


def cargo(args: list[str], cwd: Path, log_path: Path, timeout=TEST_TIMEOUT_SECS) -> tuple[int, str]:
    """Run a build or test command in its own process group, logging to `log_path`.

    A timeout kills the whole group, so no rustc outlives it holding the build
    lock. A timeout the window imposed raises WindowEnded instead of reading
    as a failed test.
    """
    budget = WINDOW.clamp(timeout)
    header = f"$ {' '.join(args)}"
    with open(log_path, "a") as f:
        f.write(f"\n{header}\n")
        f.flush()
        p = subprocess.Popen(args, cwd=cwd, env=tool_env(), stdout=f, stderr=subprocess.STDOUT)
        try:
            code = p.wait(timeout=budget)
        except subprocess.TimeoutExpired:
            kill_group(p)
            if budget < timeout:
                raise WindowEnded(" ".join(args))
            code = 124
    text = log_path.read_text()
    return code, text[text.rfind(header):]


STEP_SECS = {"build": 15 * 60, "withheld": 12 * 60, "clippy": 20 * 60, "suite": 20 * 60, "tsc": 5 * 60, "fmt": 2 * 60}


def prepare(args) -> None:
    arms = args.arms.split(",")
    for task in load_tasks(args.tasks.split(",") if args.tasks else None):
        try:
            prepare_task(task, arms, args.verify)
        except WindowEnded as e:
            log(f"{task['id']}: window ended before {e}; run prepare again to resume")
            sys.exit(EXIT_RESUME)


def prepare_task(task: dict, arms: list[str], verify: bool) -> None:
    if free_disk_gb() < DISK_FLOOR_GB:
        sys.exit(f"only {free_disk_gb():.0f} GB of disk free, need {DISK_FLOOR_GB}")
    root = task_root(task)
    info = read_json(root / "prepare.json", {})
    if info.get("strip_version") != STRIP_VERSION or not all(f"arm_{a}" in info for a in arms):
        WINDOW.need(STEP_SECS["build"], "building the arms")
        info = build_arms(task, arms)
    if verify:
        verify_task(task, arms, info, root / "prepare.log")


def build_arms(task: dict, arms: list[str]) -> dict:
    root = task_root(task)
    clone = ensure_clone(task)
    parent = parent_of(task)
    prep_log = root / "prepare.log"

    wt_a = worktree(clone, root / "prep-A", parent)
    for f in leak_files(task):
        git("rm", "-q", f, cwd=wt_a)
    git("add", "-A", cwd=wt_a)
    arm_a = commit_like(wt_a, parent, git("write-tree", cwd=wt_a).strip())
    git("update-ref", "refs/heads/arm-A", arm_a, cwd=clone)
    git("reset", "-q", "--soft", arm_a, cwd=wt_a)
    (root / "reference.diff").write_text(
        git("diff", parent, task["fix"], "--", ".", ":(exclude)docs/plans", ":(exclude)CHANGELOG.md", cwd=REPO)
    )
    info = {"parent": parent, "leak_files": leak_files(task), "arm_A": arm_a, "strip_version": STRIP_VERSION}

    for arm in arms:
        mode = ARMS[arm]
        if mode is None:
            continue
        wt = worktree(clone, root / f"prep-{arm}", arm_a)
        summary = strip_tree(wt, mode)
        write_json(root / f"strip-{arm}.json", summary)
        generators = sorted(set(GENERATOR_RE.findall(
            git("grep", "-h", "-E", r"fn generate_[a-z0-9_]+_file", "--", "crates/lucidos-engine/src", cwd=wt)
        )))
        info[f"generators_{arm}"] = generators
        for gen in generators:
            code, out = cargo(["cargo", "test", "-p", "lucidos-engine", "--lib", gen, "--", "--include-ignored"], wt, prep_log)
            if code != 0 or re.search(r"running [1-9]\d* tests?", out) is None:
                sys.exit(f"{task['id']}: generator {gen} failed, see {prep_log}")
        git("add", "-A", cwd=wt)
        sha = commit_like(wt, parent, git("write-tree", cwd=wt).strip())
        git("update-ref", f"refs/heads/arm-{arm}", sha, cwd=clone)
        git("reset", "-q", "--soft", sha, cwd=wt)
        info[f"arm_{arm}"] = sha
        info[f"strip_{arm}"] = {"files": len(summary), "blocks": sum(len(v) for v in summary.values())}
        log(f"{task['id']}: arm {arm} strips {info[f'strip_{arm}']['blocks']} blocks in {len(summary)} files")
    write_json(root / "prepare.json", info)
    return info


def failing_tests(output: str) -> set[str]:
    return set(re.findall(r"^test (\S+) \.\.\. FAILED", output, re.M))


def reset_prep(task: dict, arm: str, info: dict) -> Path:
    """The prep worktree for `arm`, back at its base commit."""
    root = task_root(task)
    wt = root / f"prep-{arm}"
    if not wt.exists():
        return worktree(root / "src.git", wt, info[f"arm_{arm}"])
    git("reset", "-q", "--hard", info[f"arm_{arm}"], cwd=wt)
    git("clean", "-qfd", cwd=wt)
    return wt


def verify_task(task: dict, arms: list[str], info: dict, prep_log: Path) -> None:
    root = task_root(task)
    result = read_json(root / "verify.json", {})

    def save():
        write_json(root / "verify.json", result)

    # I2: the withheld test compiles and fails on the base ...
    if "base" not in result:
        WINDOW.need(STEP_SECS["withheld"], "the withheld test on the base")
        result["base"] = run_withheld(task, reset_prep(task, "A", info), root / "verify-base.log")
        save()
    # ... and passes on the real fix.
    if "fix" not in result:
        WINDOW.need(STEP_SECS["withheld"], "the withheld test on the fix")
        wt_a = reset_prep(task, "A", info)
        fix_diff = git("diff", info["parent"], task["fix"], "--", ".", ":(exclude)docs/plans", cwd=REPO)
        sh(["git", "apply", "--index"], cwd=wt_a, input=fix_diff)
        result["fix"] = run_withheld(task, wt_a, root / "verify-fix.log")
        reset_prep(task, "A", info)
        save()
    log(f"{task['id']}: withheld test on base={result['base']}, on fix={result['fix']}")
    if result["base"] != "fail" or result["fix"] != "pass":
        sys.exit(f"{task['id']}: the withheld test does not discriminate")

    # I3: every stripped arm still lints clean, passes the engine suite and type-checks.
    for arm in arms:
        if ARMS[arm] is None:
            continue
        if f"clippy_{arm}" not in result:
            WINDOW.need(STEP_SECS["clippy"], f"clippy on arm {arm}")
            clippy = ["cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"]
            code, out = cargo(clippy, reset_prep(task, arm, info), prep_log)
            if code != 0:
                # One retry: a lint failure repeats, a flaky build step does not.
                code, out = cargo(clippy, reset_prep(task, arm, info), prep_log)
            result[f"clippy_{arm}"] = code
            if code != 0:
                result[f"clippy_{arm}_tail"] = out[-2000:]
            save()
        if f"suite_{arm}" not in result:
            WINDOW.need(STEP_SECS["suite"], f"the engine suite on arm {arm}")
            code, out = cargo(["./scripts/test-engine.sh"], reset_prep(task, arm, info), prep_log, timeout=STEP_SECS["suite"])
            result[f"suite_{arm}"] = {"exit": code, "failed": sorted(failing_tests(out))}
            save()
        if result[f"suite_{arm}"]["exit"] != 0 and "suite_A" not in result:
            WINDOW.need(STEP_SECS["suite"], "the engine suite on arm A, for comparison")
            code, out = cargo(["./scripts/test-engine.sh"], reset_prep(task, "A", info), prep_log, timeout=STEP_SECS["suite"])
            result["suite_A"] = {"exit": code, "failed": sorted(failing_tests(out))}
            save()
        if f"tsc_{arm}" not in result:
            WINDOW.need(STEP_SECS["tsc"], f"tsc on arm {arm}")
            result[f"tsc_{arm}"] = tsc_check(reset_prep(task, arm, info), prep_log)
            save()
        if f"fmt_{arm}" not in result:
            WINDOW.need(STEP_SECS["fmt"], f"rustfmt on arm {arm}")
            result[f"fmt_{arm}"], _ = cargo(["cargo", "fmt", "--all", "--check"], reset_prep(task, arm, info), prep_log)
            save()
        new_failures = sorted(set(result[f"suite_{arm}"]["failed"]) - set(result.get("suite_A", {}).get("failed", [])))
        if new_failures and f"suite_{arm}_rerun" not in result:
            # A test arm A passes may only be flaky under full-suite load.
            WINDOW.need(STEP_SECS["withheld"], f"an isolated rerun of arm {arm}'s new failures")
            code, _ = cargo(["./scripts/test-engine.sh", "--", "--", "--exact", *new_failures], reset_prep(task, arm, info), prep_log)
            result[f"suite_{arm}_rerun"] = {"tests": new_failures, "exit": code}
            save()
        log(f"{task['id']}: arm {arm} clippy={result[f'clippy_{arm}']} suite={result[f'suite_{arm}']} "
            f"tsc={result[f'tsc_{arm}']} fmt={result[f'fmt_{arm}']}")
        broken = [k for k in (f"clippy_{arm}", f"tsc_{arm}", f"fmt_{arm}") if result[k] != 0]
        if new_failures and result[f"suite_{arm}_rerun"]["exit"] != 0:
            broken.append(f"suite_{arm}: {new_failures}")
        if result[f"suite_{arm}"]["exit"] != 0 and not result[f"suite_{arm}"]["failed"]:
            broken.append(f"suite_{arm} exited {result[f'suite_{arm}']['exit']} without a test summary")
        if broken:
            sys.exit(f"{task['id']}: arm {arm} is not green ({', '.join(broken)}); see {prep_log}")
    for arm in ("A", *arms):
        drop_target(root / f"prep-{arm}")


def tsc_check(tree: Path, prep_log: Path) -> int:
    main_checkout = Path(git("rev-parse", "--path-format=absolute", "--git-common-dir", cwd=REPO).strip()).parent
    # Older commits keep the only tsconfig in the app crate.
    project = "." if (tree / "tsconfig.json").exists() else "crates/lucidos-app"
    link = tree / "node_modules"
    if not link.exists():
        link.symlink_to(main_checkout / "node_modules")
    try:
        with open(prep_log, "a") as f:
            f.write(f"\n$ npx tsc --noEmit -p {project}\n")
            f.flush()
            p = subprocess.Popen(["npx", "tsc", "--noEmit", "-p", project], cwd=tree, stdout=f, stderr=subprocess.STDOUT)
            budget = WINDOW.clamp(STEP_SECS["tsc"])
            try:
                return p.wait(timeout=budget)
            except subprocess.TimeoutExpired:
                kill_group(p)
                if budget < STEP_SECS["tsc"]:
                    raise WindowEnded("tsc")
                return 124
    finally:
        if link.is_symlink():
            link.unlink()


# ---------------------------------------------------------------- withheld test


def install_withheld(task: dict, repo: Path) -> bool:
    spec = task["withheld_test"]
    host = repo / spec["host"]
    if not host.exists():
        return False
    shutil.copyfile(task["dir"] / spec["file"], repo / spec["dest"])
    name = Path(spec["dest"]).name
    with open(host, "a") as f:
        f.write(f'\n#[cfg(test)]\n#[path = "{name}"]\nmod eval_withheld_tests;\n')
    return True


def classify_test_output(code: int, out: str) -> str:
    counts = [int(n) for n in re.findall(r"running (\d+) tests?", out)]
    if re.search(r"error(\[E\d+\])?: |could not compile", out) and not counts:
        return "compile-error"
    if code == 124:
        return "timeout"
    if code == 0 and "test result: ok" in out and sum(counts) > 0:
        return "pass"
    if sum(counts) == 0:
        return "no-tests"
    return "fail"


def run_withheld(task: dict, repo: Path, log_path: Path) -> str:
    if log_path.exists():
        log_path.unlink()
    if not install_withheld(task, repo):
        return "host-missing"
    code, out = cargo(["./scripts/test-engine.sh", "--", "--", "eval_withheld_tests"], repo, log_path)
    return classify_test_output(code, out)


# ---------------------------------------------------------------- run


def runs_root(args) -> Path:
    return DATA_ROOT / args.runs_dir


def make_plan(args) -> list[dict]:
    root = runs_root(args)
    plan_path = root / "plan.json"
    tasks = [t["id"] for t in load_tasks(args.tasks.split(",") if args.tasks else None)]
    arms = args.arms.split(",")
    spec = {"tasks": tasks, "arms": arms, "runs": args.runs, "seed": args.seed}
    plan = read_json(plan_path)
    if plan and plan["spec"] == spec:
        return plan["runs"]
    if plan:
        sys.exit(f"{plan_path} exists with a different spec; pick another --runs-dir")
    cells = [(t, a, r) for t in tasks for a in arms for r in range(1, args.runs + 1)]
    random.Random(args.seed).shuffle(cells)
    runs = [{"id": f"r{i + 1:02d}", "task": t, "arm": a, "rep": r} for i, (t, a, r) in enumerate(cells)]
    write_json(plan_path, {"spec": spec, "runs": runs})
    for r in runs:
        meta_path = root / r["id"] / "meta.json"
        if not meta_path.exists():
            write_json(meta_path, dict(r, status="queued"))
    return runs


GRADE_SECS = 15 * 60
MAX_ERRORS = 3  # a run that errors this often is marked "failed" and stops the resume loop
# A session may start while this much window remains. One the window cuts short
# is discarded, never graded, and re-runs first next window with the full cap.
LAUNCH_MIN_SECS = 22 * 60
SESSION_STATES = ("queued", "running", "aborted-memory")
GRADE_STATES = ("finished", "timed-out")


class Scheduler:
    """Two slots. Each launches a session when a full cap still fits in the
    window, grades a finished run when it does not, and stops when neither fits."""

    def __init__(self, args, runs: list[dict]):
        self.args = args
        self.root = runs_root(args)
        self.tasks = {t["id"]: t for t in load_tasks(None)}
        self.to_run: deque = deque()
        self.to_grade: deque = deque()
        cut_first = sorted(runs, key=lambda r: -self.meta(r["id"]).get("window_cuts", 0))
        for r in cut_first:
            state = self.meta(r["id"])["status"]
            if state in SESSION_STATES:
                self.to_run.append((r["id"], 1 if state == "aborted-memory" else 0))
            elif state in GRADE_STATES or (args.regrade and state == "graded"):
                self.to_grade.append(r["id"])
        self.lock = threading.Lock()
        self.active: dict[str, tuple[subprocess.Popen, float]] = {}
        self.memory_killed: set[str] = set()
        self.done = threading.Event()
        self.last_launch = 0.0

    def remaining_work(self) -> int:
        return sum(1 for r in self.root.glob("r*/meta.json") if read_json(r)["status"] not in ("graded", "failed"))

    def meta(self, rid: str) -> dict:
        return read_json(self.root / rid / "meta.json")

    def set_meta(self, rid: str, **kv) -> dict:
        m = self.meta(rid)
        m.update(kv)
        write_json(self.root / rid / "meta.json", m)
        return m

    def run(self) -> None:
        # No session is live yet, so anything still running in a sandbox is an
        # orphan from a window the engine stopped.
        reap(SANDBOX_ROOT)
        # A fresh lock file each window, so a process that ever kept the old one
        # open (an sccache server did) cannot starve this window's builds.
        (SANDBOX_ROOT / ".cargo-build.lock").unlink(missing_ok=True)
        watchdog = threading.Thread(target=self.watchdog, daemon=True)
        watchdog.start()
        workers = [threading.Thread(target=self.worker, args=(s,)) for s in range(1, SLOTS + 1)]
        for w in workers:
            w.start()
        for w in workers:
            w.join()
        self.done.set()

    def next_item(self):
        """A run to grade first, since grading is short and banks the result
        before an engine restart can lose it. Else a session that fits."""
        with self.lock:
            if self.to_grade and not self.args.no_grade and WINDOW.fits(GRADE_SECS):
                return "grade", self.to_grade.popleft()
            for item in list(self.to_run):
                if WINDOW.fits(self.launch_need(item[0])):
                    self.to_run.remove(item)
                    return "session", item
        return None

    def launch_need(self, rid: str) -> float:
        """A run the window already cut once must get its full cap this time."""
        return CAP_SECS + 180 if self.meta(rid).get("window_cuts", 0) else LAUNCH_MIN_SECS

    def worker(self, slot: int) -> None:
        while (item := self.next_item()) is not None:
            try:
                if not self.handle(slot, item):
                    return
            except WindowEnded:
                log("window ended mid-grade; that run is graded next time")
                return
            except Exception as e:  # noqa: BLE001 - one run's failure must not stop the slot
                grading = item[0] == "grade"
                rid = item[1] if grading else item[1][0]
                errors = self.meta(rid).get("errors", 0) + 1
                # Retry next window from the step that failed; give up after MAX_ERRORS.
                retry = "finished" if grading else "queued"
                status = "failed" if errors >= MAX_ERRORS else retry
                log(f"{rid}: ERROR {type(e).__name__}: {e} ({errors}/{MAX_ERRORS}, now {status})")
                self.set_meta(rid, status=status, errors=errors, error=f"{type(e).__name__}: {e}")

    def handle(self, slot: int, item) -> bool:
        """Process one queue item. False means this slot should stop for the window."""
        kind, payload = item
        if kind == "grade":
            grade_run(self.root, payload, self.tasks)
            self.set_meta(payload, status="graded")
            return True
        rid, attempt = payload
        with self.lock:
            pause = self.last_launch + LAUNCH_STAGGER_SECS - time.time()
            self.last_launch = max(time.time(), self.last_launch + LAUNCH_STAGGER_SECS)
        if pause > 0:
            time.sleep(pause)
        if free_disk_gb() < DISK_FLOOR_GB:
            log(f"{rid}: only {free_disk_gb():.0f} GB of disk free, not launching")
            with self.lock:
                self.to_run.appendleft(payload)
            return False
        if not wait_for_memory(rid, self.launch_need(rid)) or not WINDOW.fits(self.launch_need(rid) - 60):
            with self.lock:
                self.to_run.appendleft(payload)
            return False
        status = self.session(rid, slot, attempt)
        with self.lock:
            if status == "aborted-memory" and attempt == 0:
                self.to_run.append((rid, 1))
            elif status in GRADE_STATES:
                self.to_grade.append(rid)
        return True

    def session(self, rid: str, slot: int, attempt: int) -> str:
        m = self.meta(rid)
        task = self.tasks[m["task"]]
        run_dir = self.root / rid
        sandbox = SANDBOX_ROOT / rid / "lucidos"
        base = make_sandbox(task, m["arm"], sandbox)
        wait_log = run_dir / "cargo-wait.log"
        wait_log.unlink(missing_ok=True)
        argv = agent_argv(task["prompt"])
        env = model_env(wait_log)
        self.set_meta(
            rid, status="running", slot=slot, attempt=attempt, base=base, sandbox=str(sandbox),
            argv=argv[:2] + ["<prompt>"] + argv[3:], prompt_sha=hashlib.sha256(argv[2].encode()).hexdigest()[:16],
            env_keys=sorted(env), started=time.time(),
        )
        log(f"{rid}: {m['task']} arm {m['arm']} rep {m['rep']} on slot {slot} (attempt {attempt})")
        with open(run_dir / "transcript.jsonl", "w") as out, open(run_dir / "stderr.log", "w") as err:
            p = subprocess.Popen(argv, cwd=sandbox, env=env, stdin=subprocess.DEVNULL,
                                 stdout=out, stderr=err)
            started = time.time()
            with self.lock:
                self.active[rid] = (p, started)
            timed_out = False
            cut_by_window = False
            while p.poll() is None:
                if time.time() - started > CAP_SECS:
                    timed_out = True
                    kill_group(p)
                    break
                if not WINDOW.fits(90):
                    cut_by_window = True
                    kill_group(p)
                    break
                time.sleep(5)
            p.wait()
            with self.lock:
                self.active.pop(rid, None)
        wall = time.time() - started
        reap(SANDBOX_ROOT / rid)
        if cut_by_window:
            status = "queued"
            self.set_meta(rid, window_cuts=self.meta(rid).get("window_cuts", 0) + 1)
        elif rid in self.memory_killed:
            self.memory_killed.discard(rid)
            status = "aborted-memory"
        elif timed_out:
            status = "timed-out"
        else:
            status = "finished"
        self.set_meta(rid, status=status, wall_secs=round(wall, 1), exit_code=p.returncode)
        log(f"{rid}: {status} after {wall / 60:.1f} min")
        return status

    def watchdog(self) -> None:
        low = 0
        with open(self.root / "memory.log", "a") as f:
            while not self.done.is_set():
                avail, press = available_gb(), pressure_level()
                with self.lock:
                    active = sorted(self.active.items(), key=lambda kv: kv[1][1])
                f.write(f"{time.strftime('%H:%M:%S')}\tavail={avail:.2f}\tpressure={press}\tactive={[a for a, _ in active]}\n")
                f.flush()
                low = low + 1 if avail < KILL_FLOOR_GB else 0
                if low >= KILL_SAMPLES and active:
                    rid, (p, _) = active[-1]
                    log(f"{rid}: MEMORY {avail:.1f} GB available, killing the newest session")
                    f.write(f"kill {rid}\n")
                    self.memory_killed.add(rid)
                    kill_group(p)
                    low = 0
                time.sleep(SAMPLE_SECS)


def descendants(pid: int) -> list[int]:
    """Every process below `pid`, found by parent id, deepest first."""
    out = subprocess.run(["pgrep", "-P", str(pid)], capture_output=True, text=True).stdout.split()
    found = []
    for child in map(int, out):
        found += descendants(child) + [child]
    return found


def kill_group(p: subprocess.Popen) -> None:
    """Stop `p` and everything it started.

    The children stay in the harness's own process group on purpose: when the
    engine stops the background task, its group kill must reach them too. So a
    timeout walks the tree by parent id instead of signalling a group.
    """
    for sig in (signal.SIGTERM, signal.SIGKILL):
        for pid in descendants(p.pid) + [p.pid]:
            try:
                os.kill(pid, sig)
            except ProcessLookupError:
                pass
        for _ in range(30):
            if p.poll() is not None and not descendants(p.pid):
                return
            time.sleep(0.5)


def processes_under(root: Path) -> set[int]:
    """Pids of this user's processes whose working directory is inside `root`.

    A kernel fact, never command-line text: a coding-agent session carries its
    thread history in argv, so matching a path there can hit a session that
    only mentions a sandbox (dev-runtime.md, ADR 0025).
    """
    root_s = str(root.resolve()) + os.sep
    out = subprocess.run(["lsof", "-a", "-u", str(os.getuid()), "-d", "cwd", "-Fpn"],
                         capture_output=True, text=True).stdout
    pids, pid = set(), None
    for line in out.splitlines():
        if line.startswith("p"):
            pid = int(line[1:])
        elif line.startswith("n") and pid is not None and (line[1:] + os.sep).startswith(root_s):
            pids.add(pid)
    return pids


def reap(root: Path) -> int:
    """Kill every process group with a member working inside `root`.

    Claude Code runs each Bash tool call in a process group of its own. When the
    engine stops this harness, those groups are orphaned rather than killed, and
    an agent's build keeps running. Two groups are never touched: this
    process's own, and one holding the daemonized sccache server (parent pid 1).
    That server compiles inside the sandboxes for every build on the machine.
    """
    table = {}
    for line in subprocess.run(["ps", "-Ao", "pid=,ppid=,pgid=,comm="], capture_output=True, text=True).stdout.splitlines():
        parts = line.split(None, 3)
        if len(parts) == 4:
            table[int(parts[0])] = (int(parts[1]), int(parts[2]), os.path.basename(parts[3]))
    under = processes_under(root)
    groups = {table[p][1] for p in under if p in table}
    servers = {pid for pid, (ppid, _, comm) in table.items() if comm == "sccache" and ppid == 1}
    spared = set(servers)
    for server in servers:
        spared.update(descendants(server))
    killed = 0
    for pgid in groups - {os.getpgid(0), 0, 1}:
        members = [pid for pid, (_, g, _) in table.items() if g == pgid]
        try:
            if spared.isdisjoint(members):
                os.killpg(pgid, signal.SIGTERM)
            else:
                # The server shares this group; stop the orphaned build around it.
                for pid in members:
                    if pid not in spared:
                        os.kill(pid, signal.SIGTERM)
            killed += 1
        except (ProcessLookupError, PermissionError):
            pass
    if killed:
        log(f"reaped {killed} orphaned process group(s) under {root}")
    return killed


def make_sandbox(task: dict, arm: str, sandbox: Path) -> str:
    clone = task_root(task) / "src.git"
    if sandbox.exists():
        shutil.rmtree(sandbox)
    sandbox.parent.mkdir(parents=True, exist_ok=True)
    git("clone", "-q", "--no-local", "--single-branch", "--branch", f"arm-{arm}", str(clone), str(sandbox))
    git("branch", "-q", "-m", f"arm-{arm}", "work", cwd=sandbox)
    git("remote", "remove", "origin", cwd=sandbox)
    git("reflog", "expire", "--expire=now", "--all", cwd=sandbox)
    if sh(["git", "cat-file", "-e", task["fix"]], cwd=sandbox, check=False).returncode == 0:
        raise RuntimeError(f"{task['id']}: the fix is reachable in a sandbox")
    return git("rev-parse", "HEAD", cwd=sandbox).strip()


def run_cmd(args) -> None:
    # Fail here, in the main thread: sys.exit inside a worker is silent.
    model_env()
    claude_bin()
    for task in load_tasks(args.tasks.split(",") if args.tasks else None):
        if read_json(task_root(task) / "prepare.json", {}).get("strip_version") != STRIP_VERSION:
            sys.exit(f"{task['id']}: its arms predate stripper v{STRIP_VERSION}; run prepare --verify first")
    runs = make_plan(args)
    scheduler = Scheduler(args, runs)
    log(f"{len(runs)} runs in {runs_root(args)}, {scheduler.remaining_work()} not yet graded")
    scheduler.run()
    report(args)
    left = scheduler.remaining_work()
    if left:
        log(f"window ended with {left} runs not yet graded; run again to resume")
        sys.exit(EXIT_RESUME)


# ---------------------------------------------------------------- grade


def transcript_events(path: Path) -> list[dict]:
    events = []
    if not path.exists():
        return events
    for line in path.read_text().splitlines():
        try:
            events.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return events


def assistant_blocks(events: list[dict]):
    for e in events:
        if e.get("type") != "assistant":
            continue
        for block in e.get("message", {}).get("content", []) or []:
            yield block


def narrative(events: list[dict], limit: int = 60000) -> str:
    """The agent's own words and actions, in order, without tool output."""
    parts = []
    for b in assistant_blocks(events):
        if b.get("type") == "text":
            parts.append(f"[assistant] {b['text']}")
        elif b.get("type") == "thinking" and b.get("thinking"):
            parts.append(f"[thinking] {b['thinking']}")
        elif b.get("type") == "tool_use":
            inp = b.get("input", {})
            brief = inp.get("command") or inp.get("file_path") or inp.get("pattern") or json.dumps(inp)[:200]
            parts.append(f"[{b.get('name')}] {str(brief)[:300]}")
    text = "\n".join(parts)
    return text if len(text) <= limit else text[: limit // 2] + "\n...[middle cut]...\n" + text[-limit // 2 :]


def diff_stats(repo: Path, base: str) -> dict:
    git("add", "-A", cwd=repo)
    stats = {"files": 0, "added": 0, "removed": 0, "test_added": 0, "test_removed": 0}
    for line in git("diff", "--cached", "--numstat", base, cwd=repo).splitlines():
        a, r, path = line.split("\t", 2)
        if a == "-":
            continue
        a, r = int(a), int(r)
        stats["files"] += 1
        if TEST_PATH_RE.search(path):
            stats["test_added"] += a
            stats["test_removed"] += r
            continue
        stats["added"] += a
        stats["removed"] += r
        # Inline #[cfg(test)] modules sit at the end of a Rust file by convention.
        if path.endswith(".rs"):
            t_add, t_rem = test_region_lines(repo, base, path)
            stats["added"] -= t_add
            stats["removed"] -= t_rem
            stats["test_added"] += t_add
            stats["test_removed"] += t_rem
    return stats


def test_region_lines(repo: Path, base: str, path: str) -> tuple[int, int]:
    def marker(text: str) -> int:
        m = re.search(r"^#\[cfg\(test\)\]\s*\n\s*mod ", text, re.M)
        return text.count("\n", 0, m.start()) + 1 if m else 10**9

    new_text = sh(["git", "show", f":{path}"], cwd=repo, check=False).stdout
    old_text = sh(["git", "show", f"{base}:{path}"], cwd=repo, check=False).stdout
    new_mark, old_mark = marker(new_text), marker(old_text)
    added = removed = 0
    for h in re.finditer(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@", git("diff", "--cached", "-U0", base, "--", path, cwd=repo), re.M):
        o_start, o_len = int(h.group(1)), int(h.group(2) or 1)
        n_start, n_len = int(h.group(3)), int(h.group(4) or 1)
        added += sum(1 for n in range(n_start, n_start + n_len) if n >= new_mark)
        removed += sum(1 for o in range(o_start, o_start + o_len) if o >= old_mark)
    return added, removed


def session_metrics(events: list[dict]) -> dict:
    result = next((e for e in reversed(events) if e.get("type") == "result"), None)
    tools: dict[str, int] = {}
    for b in assistant_blocks(events):
        if b.get("type") == "tool_use":
            tools[b.get("name")] = tools.get(b.get("name"), 0) + 1
    if not result:
        return {"result": None, "tools": tools}
    usage = result.get("usage", {})
    models = sorted(result.get("modelUsage", {}))
    refused = [m for m in models if REFUSED_MODEL_RE.search(m)]
    if refused:
        log(f"REFUSED MODEL in a session result: {refused}")
    return {
        "refused_models": refused,
        "result": result.get("subtype"),
        "is_error": result.get("is_error"),
        "duration_ms": result.get("duration_ms"),
        "duration_api_ms": result.get("duration_api_ms"),
        "turns": result.get("num_turns"),
        "cost_usd": result.get("total_cost_usd"),
        "input_tokens": usage.get("input_tokens", 0),
        "cache_creation_tokens": usage.get("cache_creation_input_tokens", 0),
        "cache_read_tokens": usage.get("cache_read_input_tokens", 0),
        "output_tokens": usage.get("output_tokens", 0),
        "models": models,
        "final_message": result.get("result", ""),
        "tools": tools,
    }


def cite_grep(events: list[dict]) -> dict:
    hits, words = [], []
    for b in assistant_blocks(events):
        text = b.get("text") or b.get("thinking") or ""
        for rx in CITE_RES:
            for m in rx.finditer(text):
                hits.append(text[max(0, m.start() - 200) : m.end() + 200].replace("\n", " "))
        words += [m.group(0).lower() for m in JUSTIFY_WORD_RE.finditer(text)]
    return {"comment_mentions": len(hits), "snippets": hits[:12], "justify_words": words}


def lock_wait(path: Path) -> float:
    if not path.exists():
        return 0.0
    return round(sum(float(line.split("\t")[1]) for line in path.read_text().splitlines() if line.count("\t") >= 2), 1)


def judge(template: str, fields: dict, out_path: Path) -> dict:
    prompt = (HERE / template).read_text()
    for k, v in fields.items():
        prompt = prompt.replace("{{" + k + "}}", v)
    # I8: the judge never learns the arm.
    leaked = [m for m in ARM_MARKERS if m in prompt]
    if leaked:
        # A worker thread must not sys.exit: that kills it silently.
        log(f"judge prompt for {out_path.parent.name} carries arm markers {leaked}; not judged")
        verdict = {"error": f"arm markers in prompt: {leaked}"}
        write_json(out_path, verdict)
        return verdict
    out_path.with_suffix(".prompt.md").write_text(prompt)
    cwd = DATA_ROOT / "judge-cwd"
    cwd.mkdir(parents=True, exist_ok=True)
    argv = [claude_bin(), "-p", "--model", MODEL, "--effort", EFFORT, "--output-format", "json",
            "--no-session-persistence", "--setting-sources", "project", "--tools", ""]
    last = ""
    for _ in range(2):
        try:
            budget = WINDOW.clamp(1200)
            p = subprocess.run(argv, cwd=cwd, env=model_env(), input=prompt, text=True,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=budget)
            last = p.stdout[-500:] + p.stderr[-500:]
            outer = json.loads(p.stdout)
            refused = [m for m in outer.get("modelUsage", {}) if REFUSED_MODEL_RE.search(m)]
            if refused:
                return {"error": f"judge ran on a refused model {refused}"}
            text = outer.get("result", "")
            verdict = json.loads(re.search(r"\{.*\}", text, re.S).group(0))
            verdict["judge_cost_usd"] = outer.get("total_cost_usd")
            write_json(out_path, verdict)
            return verdict
        except subprocess.TimeoutExpired as e:
            if budget < 1200:
                raise WindowEnded("a judge call") from e
            last = f"{e}: {last}"
        except (json.JSONDecodeError, AttributeError) as e:
            last = f"{e}: {last}"
    write_json(out_path, {"error": last})
    return {"error": last}


def grade_run(root: Path, rid: str, tasks: dict) -> dict:
    run_dir = root / rid
    m = read_json(run_dir / "meta.json")
    task = tasks[m["task"]]
    repo = Path(m["sandbox"])
    events = transcript_events(run_dir / "transcript.jsonl")
    log(f"{rid}: grading")

    dest = repo / task["withheld_test"]["dest"]
    if dest.exists():
        # A grade cut short earlier left the withheld test installed. The index
        # still holds the agent's version of the host file.
        git("checkout", "-q", "--", task["withheld_test"]["host"], cwd=repo)
        dest.unlink()
    stats = diff_stats(repo, m["base"])
    agent_diff = git("diff", "--cached", m["base"], cwd=repo)
    (run_dir / "agent.diff").write_text(agent_diff)

    try:
        withheld = run_withheld(task, repo, run_dir / "withheld.log")
    finally:
        git("checkout", "-q", "--", ".", cwd=repo)
        dest.unlink(missing_ok=True)

    metrics = session_metrics(events)
    story = narrative(events)
    reference = (task_root(task) / "reference.diff").read_text()
    cut = 80000
    fix_verdict = judge("judge_fix.md", {
        "BUG": task["prompt"], "REFERENCE": reference[:cut],
        "AGENT_DIFF": agent_diff[:cut] if agent_diff.strip() else "(the agent changed nothing)",
        "AGENT_SUMMARY": metrics.get("final_message") or "(no final message)",
    }, run_dir / "judge-fix.json")
    cite_verdict = judge("judge_cite.md", {"BUG": task["prompt"], "NARRATIVE": story}, run_dir / "judge-cite.json")

    grade = {
        "withheld": withheld,
        "diff": stats,
        "metrics": metrics,
        "cargo_wait_secs": lock_wait(run_dir / "cargo-wait.log"),
        "grep": cite_grep(events),
        "judge_fix": fix_verdict,
        "judge_cite": cite_verdict,
        "read_host_file": any(
            b.get("type") == "tool_use" and task["withheld_test"]["host"] in json.dumps(b.get("input", {}))
            for b in assistant_blocks(events)
        ),
    }
    write_json(run_dir / "grade.json", grade)
    drop_target(repo)
    log(f"{rid}: withheld={withheld} score={fix_verdict.get('score')} cites={cite_verdict.get('cited_comment')}")
    return grade


def grade_cmd(args) -> None:
    tasks = {t["id"]: t for t in load_tasks(None)}
    grade_run(runs_root(args), args.run, tasks)
    meta_path = runs_root(args) / args.run / "meta.json"
    write_json(meta_path, dict(read_json(meta_path), status="graded"))


# ---------------------------------------------------------------- report


def mean(xs):
    xs = [x for x in xs if x is not None]
    return statistics.mean(xs) if xs else None


def fmt(x, digits=1):
    return "-" if x is None else f"{x:.{digits}f}"


def collect(root: Path) -> list[dict]:
    rows = []
    for meta_path in sorted(root.glob("r*/meta.json")):
        m = read_json(meta_path)
        g = read_json(meta_path.parent / "grade.json")
        if not g:
            continue
        mt = g["metrics"]
        rows.append({
            "id": m["id"], "task": m["task"], "arm": m["arm"], "rep": m["rep"], "status": m["status"],
            "timed_out": m.get("status") == "timed-out" or m.get("wall_secs", 0) >= CAP_SECS,
            "pass": g["withheld"] == "pass", "withheld": g["withheld"],
            "score": g["judge_fix"].get("score"), "papered": g["judge_fix"].get("papered_over"),
            "cited": g["judge_cite"].get("cited_comment"),
            "narrower": g["judge_cite"].get("comment_justified_narrower_fix"),
            "mentions": g["grep"]["comment_mentions"],
            "src_lines": g["diff"]["added"] + g["diff"]["removed"],
            "test_lines": g["diff"]["test_added"] + g["diff"]["test_removed"],
            "files": g["diff"]["files"],
            "wall_min": m.get("wall_secs", 0) / 60,
            "wait_min": g.get("cargo_wait_secs", 0) / 60,
            "out_tok": mt.get("output_tokens"),
            "tot_tok": sum(mt.get(k) or 0 for k in ("input_tokens", "cache_creation_tokens", "cache_read_tokens", "output_tokens")),
            "cost": mt.get("cost_usd"), "turns": mt.get("turns"),
            "argv": json.dumps(m.get("argv")), "env_keys": json.dumps(m.get("env_keys")),
        })
    return rows


def permutation_p(rows: list[dict], key, arms=("A", "C")) -> float | None:
    """Exact p-value for |mean(C) - mean(A)|, shuffling arm labels within each task."""
    by_task: dict[str, list[tuple[str, float]]] = {}
    for r in rows:
        v = key(r)
        if v is None or r["arm"] not in arms:
            continue
        by_task.setdefault(r["task"], []).append((r["arm"], float(v)))

    def diff(assign):
        a = [v for grp in assign for arm, v in grp if arm == arms[0]]
        c = [v for grp in assign for arm, v in grp if arm == arms[1]]
        return abs(statistics.mean(c) - statistics.mean(a)) if a and c else 0.0

    groups = list(by_task.values())
    observed = diff(groups)
    options = []
    for grp in groups:
        n_c = sum(1 for arm, _ in grp if arm == arms[1])
        values = [v for _, v in grp]
        labelings = []
        for chosen in itertools.combinations(range(len(values)), n_c):
            labelings.append([(arms[1] if i in chosen else arms[0], v) for i, v in enumerate(values)])
        options.append(labelings)
    total = hits = 0
    for combo in itertools.product(*options):
        total += 1
        hits += diff(combo) >= observed - 1e-12
    return hits / total if total else None


def report(args) -> None:
    root = runs_root(args)
    rows = collect(root)
    if not rows:
        log("no graded runs yet")
        return
    lines = ["| run | task | arm | withheld | judge | papered | cited | src lines | test lines | wall min | cargo wait | out tok | cost $ |",
             "|---|---|---|---|---|---|---|---|---|---|---|---|---|"]
    for r in sorted(rows, key=lambda r: (r["task"], r["arm"], r["rep"])):
        lines.append(
            f"| {r['id']} | {r['task']} | {r['arm']} | {r['withheld']} | {r['score']} | {r['papered']} | {r['cited']} "
            f"| {r['src_lines']} | {r['test_lines']} | {fmt(r['wall_min'])} | {fmt(r['wait_min'])} | {r['out_tok']} | {fmt(r['cost'], 2)} |"
        )
    lines += ["", "| arm | n | withheld pass | judge mean | papered | cited a comment | src lines | wall min | out tok | cost $ |",
              "|---|---|---|---|---|---|---|---|---|---|"]
    for arm in sorted({r["arm"] for r in rows}):
        rs = [r for r in rows if r["arm"] == arm]
        lines.append(
            f"| {arm} | {len(rs)} | {sum(r['pass'] for r in rs)}/{len(rs)} | {fmt(mean([r['score'] for r in rs]), 2)} "
            f"| {sum(bool(r['papered']) for r in rs)} | {sum(bool(r['cited']) for r in rs)} | {fmt(mean([r['src_lines'] for r in rs]))} "
            f"| {fmt(mean([r['wall_min'] for r in rs]))} | {fmt(mean([r['out_tok'] for r in rs]), 0)} | {fmt(mean([r['cost'] for r in rs]), 2)} |"
        )
    lines += ["", "| task | arm | withheld pass | judge scores | src lines |", "|---|---|---|---|---|"]
    for task in sorted({r["task"] for r in rows}):
        for arm in sorted({r["arm"] for r in rows}):
            rs = [r for r in rows if r["task"] == task and r["arm"] == arm]
            if rs:
                lines.append(f"| {task} | {arm} | {sum(r['pass'] for r in rs)}/{len(rs)} | {[r['score'] for r in rs]} | {[r['src_lines'] for r in rs]} |")
    lines += ["", "Exact permutation test, arm labels shuffled within each task (two-sided):", ""]
    for label, key in (("judge score", lambda r: r["score"]), ("withheld pass", lambda r: 1.0 if r["pass"] else 0.0),
                       ("src lines", lambda r: r["src_lines"]), ("output tokens", lambda r: r["out_tok"])):
        lines.append(f"- {label}: p = {fmt(permutation_p(rows, key), 3)}")

    # I4: the arms ran the same command line and environment.
    mismatches = []
    for task in {r["task"] for r in rows}:
        rs = [r for r in rows if r["task"] == task]
        if len({r["argv"] for r in rs}) > 1 or len({r["env_keys"] for r in rs}) > 1:
            mismatches.append(task)
    lines += ["", f"Settings identical across arms: {'yes' if not mismatches else 'NO for ' + ', '.join(mismatches)}"]
    text = "\n".join(lines)
    (root / "report.md").write_text(text + "\n")
    print(text)


def status(args) -> None:
    root = runs_root(args)
    for meta_path in sorted(root.glob("r*/meta.json")):
        m = read_json(meta_path)
        print(f"{m['id']} {m['task']} {m['arm']} rep{m['rep']} {m['status']} {fmt(m.get('wall_secs', 0) / 60)} min")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("prepare")
    p.add_argument("--tasks")
    p.add_argument("--arms", default="A,C")
    p.add_argument("--verify", action="store_true")
    p.add_argument("--window-mins", type=float)
    p = sub.add_parser("run")
    p.add_argument("--tasks")
    p.add_argument("--arms", default="A,C")
    p.add_argument("--runs", type=int, default=2)
    p.add_argument("--seed", type=int, default=7)
    p.add_argument("--runs-dir", default="runs")
    p.add_argument("--no-grade", action="store_true")
    p.add_argument("--regrade", action="store_true")
    p.add_argument("--window-mins", type=float)
    p = sub.add_parser("grade")
    p.add_argument("--run", required=True)
    p.add_argument("--runs-dir", default="runs")
    for name in ("report", "status"):
        p = sub.add_parser(name)
        p.add_argument("--runs-dir", default="runs")
    args = ap.parse_args()
    check_model(MODEL)
    global WINDOW
    WINDOW = Window(getattr(args, "window_mins", None))
    {"prepare": prepare, "run": run_cmd, "grade": grade_cmd, "report": report, "status": status}[args.cmd](args)


if __name__ == "__main__":
    main()
