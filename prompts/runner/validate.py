#!/usr/bin/env python3
"""Validate the Active prompt pack (C-054) against live backends.

Runs every task in prompts/tasks/ through each backend exactly
once (first-attempt metric — no retries, no repair loops),
extracts the fenced document, checks it with the real `ACTIVE`
binary, and writes prompts/results/<stamp>.json.

Backends:
  copilot  GitHub Copilot CLI (agentic framework) + BYOK env.
  direct   Raw OpenAI-compatible chat API (embedded integration).

Usage: python3 validate.py [--backend copilot|direct|all]
                            [--tasks t1-gen-basic,t3-edit-add]
                            [--stamp NAME]
"""

import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parents[2]
PROMPTS = ROOT / "prompts"
TASKS = PROMPTS / "tasks"
RESULTS = PROMPTS / "results"
VERSION = (PROMPTS / "VERSION").read_text().strip()
ACTIVE = ROOT / "target" / "debug" / ("active.exe" if os.name == "nt" else "active")

SYSTEM_FILES = {
    "generator": PROMPTS / "v1" / "system-generator.md",
    "editor": PROMPTS / "v1" / "system-editor.md",
    "explainer": PROMPTS / "v1" / "system-explainer.md",
}

COPILOT_ENV = {
    "COPILOT_PROVIDER_BASE_URL": "http://127.0.0.1:8120/v1",
    "COPILOT_PROVIDER_TYPE": "openai",
    "COPILOT_PROVIDER_WIRE_API": "responses",
    "COPILOT_MODEL": "muse-spark-1.3-contributor",
    "COPILOT_PROVIDER_API_KEY": "MUSE_DUMMY",
    "COPILOT_PROVIDER_MAX_PROMPT_TOKENS": "1048576",
    "COPILOT_PROVIDER_MAX_OUTPUT_TOKENS": "131072",
    "COPILOT_OFFLINE": "true",
}

DIRECT_URL = "http://127.0.0.1:8120/v1/chat/completions"
DIRECT_MODEL = "muse-spark-1.3-contributor"


def load_tasks(only):
    tasks = []
    for path in sorted(TASKS.glob("*.toml")):
        with open(path, "rb") as fh:
            task = tomllib.load(fh)
        task["_file"] = path.name
        if only and task["id"] not in only:
            continue
        task["system_text"] = SYSTEM_FILES[task["system"]].read_text()
        tasks.append(task)
    return tasks


def user_message(task):
    parts = []
    if task.get("document", "").strip():
        parts.append("Document:\n```active\n" + task["document"].strip() + "\n```")
    parts.append(task["user"].strip())
    return "\n\n".join(parts)


def call_copilot(system, user):
    prompt = system + "\n\n" + user
    env = dict(os.environ)
    env.update(COPILOT_ENV)
    started = time.time()
    try:
        proc = subprocess.run(
            ["copilot", "-p", prompt, "--allow-all-tools", "-s"],
            capture_output=True,
            text=True,
            timeout=300,
            env=env,
            cwd=str(ROOT),
        )
        ms = int((time.time() - started) * 1000)
        if proc.returncode != 0:
            return None, f"exit {proc.returncode}: {proc.stderr[-2000:]}", ms
        return proc.stdout, "", ms
    except subprocess.TimeoutExpired:
        return None, "timeout after 300s", 300000


def call_direct(system, user):
    body = json.dumps(
        {
            "model": DIRECT_MODEL,
            "temperature": 0,
            # Reasoning models spend budget before emitting content;
            # 2000 starved longer tasks on the direct backend (null
            # content, finish length). Applies uniformly to every
            # task on this backend; the copilot CLI path carries its
            # own (larger) output budget.
            "max_tokens": 8192,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
        }
    ).encode()
    started = time.time()
    try:
        req = urllib.request.Request(
            DIRECT_URL, data=body, headers={"Content-Type": "application/json"}
        )
        with urllib.request.urlopen(req, timeout=150) as resp:
            payload = json.load(resp)
        ms = int((time.time() - started) * 1000)
        text = payload["choices"][0]["message"]["content"] or ""
        return text, "", ms
    except Exception as exc:  # noqa: BLE001 — recorded, not raised
        return (
            None,
            f"{type(exc).__name__}: {exc}"[-2000:],
            int((time.time() - started) * 1000),
        )


def extract_code(response, kind):
    # Info string must be exactly `ACTIVE` (whitespace-tolerant):
    # a ```toml fence or fenceless prose is a contract miss for
    # code tasks, recorded via the extraction label (never silently
    # accepted).
    fences = re.findall(r"```([^\n]*)\n(.*?)```", response, re.DOTALL)
    if kind != "code":
        return response, "prose"
    for info, body in fences:
        if info.strip().lower() == "active":
            return body.strip(), "active-fence"
    if fences:
        return fences[0][1].strip(), "first-fence"
    return response.strip(), "whole-response"


def active_run(args):
    try:
        proc = subprocess.run(
            [str(ACTIVE)] + args,
            capture_output=True,
            text=True,
            timeout=60,
            cwd=str(ROOT),
        )
        return proc
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(args, 124, "", "ACTIVE binary timeout")


def get_path(tree, dotted):
    # Active keys are identifiers (never contain dots), so a plain
    # split is exact — no quoting rules to honor.
    node = tree
    for part in dotted.split("."):
        if not isinstance(node, dict) or part not in node:
            return (False, None)
        node = node[part]
    return (True, node)


def run_checks(task, code, tmpdir):
    results = []
    doc_path = Path(tmpdir) / (task["id"] + ".active")
    eval_source = code
    if task.get("append_to_base") and task.get("document", "").strip():
        eval_source = task["document"].strip() + "\n" + code.strip() + "\n"
    doc_path.write_text(
        eval_source if eval_source.endswith("\n") else eval_source + "\n",
        newline="\n",  # LF always: CRLF would poison the run on Windows
    )
    evaluated = None

    def need_eval():
        nonlocal evaluated
        if evaluated is None:
            proc = active_run(["eval", str(doc_path)])
            if proc.returncode != 0:
                return None
            try:
                evaluated = json.loads(proc.stdout)
            except json.JSONDecodeError:
                evaluated = None
        return evaluated

    for check in task.get("checks", []):
        kind = check["type"]
        if kind == "parse":
            # Parses iff no error diagnostic: `ACTIVE check` also
            # evaluates, so S/X failures (cycles, budgets) fail here
            # too — a document that cannot evaluate is not
            # well-formed output. Warnings are fine.
            proc = active_run(["check", str(doc_path)])
            passed = "[Error]" not in proc.stderr
            results.append(
                {"check": "parse", "pass": passed, "detail": proc.stderr[-500:]}
            )
        elif kind == "check_exit0":
            proc = active_run(["check", str(doc_path)])
            results.append(
                {
                    "check": "check_exit0",
                    "pass": proc.returncode == 0,
                    "detail": (proc.stdout + proc.stderr)[-500:],
                }
            )
        elif kind == "eval_eq":
            tree = need_eval()
            if tree is None:
                results.append(
                    {
                        "check": f"eval_eq {check['path']}",
                        "pass": False,
                        "detail": "evaluation failed",
                    }
                )
                continue
            found, actual = get_path(tree, check["path"])
            passed = found and actual == check["value"]
            results.append(
                {
                    "check": f"eval_eq {check['path']}",
                    "pass": passed,
                    "detail": f"got {actual!r}",
                }
            )
        elif kind == "same_eval":
            base_path = Path(tmpdir) / (task["id"] + "-base.active")
            base_path.write_text(task["document"].strip() + "\n", newline="\n")
            base_proc = active_run(["eval", str(base_path)])
            tree = need_eval()
            try:
                base_tree = (
                    json.loads(base_proc.stdout) if base_proc.returncode == 0 else None
                )
                same = tree is not None and base_tree is not None and tree == base_tree
                detail = (
                    "" if same else f"base={str(base_tree)[:200]} out={str(tree)[:200]}"
                )
            except json.JSONDecodeError:
                same = False
                detail = "base output not JSON"
            results.append({"check": "same_eval", "pass": same, "detail": detail})
        elif kind == "contains":
            results.append(
                {
                    "check": f"contains {check['text']!r}",
                    "pass": check["text"] in code,
                    "detail": "",
                }
            )
        elif kind == "not_contains":
            results.append(
                {
                    "check": f"not_contains {check['text']!r}",
                    "pass": check["text"] not in code,
                    "detail": "",
                }
            )
        elif kind == "not_match":
            # Regex absence (for spacing variants plain substrings
            # miss, e.g. `password = "` vs `password="…"`).
            found = re.search(check["pattern"], code) is not None
            results.append(
                {
                    "check": f"not_match {check['pattern']!r}",
                    "pass": not found,
                    "detail": "",
                }
            )
        elif kind == "match":
            found = re.search(check["pattern"], code) is not None
            results.append(
                {
                    "check": f"match {check['pattern']!r}",
                    "pass": found,
                    "detail": "",
                }
            )
        elif kind == "terms":
            lowered = code.lower()
            missing = [t for t in check.get("all", []) if t.lower() not in lowered]
            banned = [t for t in check.get("none", []) if t.lower() in lowered]
            results.append(
                {
                    "check": "terms",
                    "pass": not missing and not banned,
                    "detail": f"missing={missing} banned={banned}",
                }
            )
        else:
            results.append(
                {"check": kind, "pass": False, "detail": "unknown check kind"}
            )
    return results


def run_task(task, backend):
    system = task["system_text"]
    user = user_message(task)
    if backend == "copilot":
        response, error, ms = call_copilot(system, user)
    else:
        response, error, ms = call_direct(system, user)
    if response is None:
        return {
            "id": task["id"],
            "backend": backend,
            "pass": False,
            "error": error,
            "ms": ms,
            "checks": [],
            "response": "",
        }
    # Empty responses are backend flakes, not model attempts: flagged
    # distinctly so rates stay honest (a flake is re-runnable proof,
    # not a prompt defect).
    infra = not response.strip()
    code, how = extract_code(response, task.get("kind", "code"))
    with tempfile.TemporaryDirectory() as tmpdir:
        checks = run_checks(task, code, tmpdir)
    if task.get("kind", "code") == "code":
        # The fence contract is part of every code prompt: prose or
        # wrong-language fences fail even when checks would pass.
        checks.insert(
            0,
            {
                "check": "fence",
                "pass": how == "active-fence",
                "detail": how,
            },
        )
    passed = all(c["pass"] for c in checks) if checks else False
    return {
        "id": task["id"],
        "backend": backend,
        "pass": passed,
        "error": error,
        "ms": ms,
        "checks": checks,
        "extraction": how,
        "infra_flake": infra,
        "response": response,
    }


def main():
    args = sys.argv[1:]
    backends = ["copilot", "direct"]
    only = None
    stamp = time.strftime("%Y%m%d-%H%M")
    i = 0
    while i < len(args):
        if args[i] == "--backend":
            backends = args[i + 1].split(",")
            i += 2
        elif args[i] == "--tasks":
            only = set(args[i + 1].split(","))
            i += 2
        elif args[i] == "--stamp":
            stamp = args[i + 1]
            i += 2
        else:
            i += 1
    if not ACTIVE.exists():
        print(f"missing binary: {ACTIVE} (run cargo build first)", file=sys.stderr)
        sys.exit(2)
    tasks = load_tasks(only)
    print(f"pack v{VERSION}: {len(tasks)} tasks x {backends}", flush=True)
    report = {
        "pack": VERSION,
        "stamp": stamp,
        "backends": backends,
        # G-026 provenance: model IDs behind each backend rate, so rounds
        # stay comparable when models change.
        "models": {
            "copilot": COPILOT_ENV["COPILOT_MODEL"],
            "direct": DIRECT_MODEL,
        },
        "results": [],
    }
    for task in tasks:
        for backend in backends:
            print(f"[{backend}] {task['id']} ...", flush=True)
            result = run_task(task, backend)
            status = "PASS" if result["pass"] else "FAIL"
            print(f"[{backend}] {task['id']}: {status} ({result['ms']}ms)", flush=True)
            report["results"].append(result)
    for backend in backends:
        subset = [r for r in report["results"] if r["backend"] == backend]
        scored = [r for r in subset if not r.get("infra_flake")]
        rate = sum(1 for r in subset if r["pass"]) / max(len(subset), 1)
        adjusted = sum(1 for r in scored if r["pass"]) / max(len(scored), 1)
        print(
            f"{backend}: {sum(1 for r in subset if r['pass'])}/{len(subset)} = {rate:.0%} "
            f"(excl. {len(subset) - len(scored)} flakes: {adjusted:.0%})"
        )
        report[f"{backend}_rate"] = rate
        report[f"{backend}_adjusted_rate"] = adjusted
    RESULTS.mkdir(parents=True, exist_ok=True)
    out = RESULTS / f"round-{stamp}.json"
    out.write_text(json.dumps(report, indent=2))
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
