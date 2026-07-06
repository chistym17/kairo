# Phase 3 — "Can it run many tasks?"

**One line:** wrap the existing agent loop in a benchmark harness that runs many tasks, logs every run, and produces pass rate + failure analysis.

Phase 1 and 2 are done. `loop_engine::run()` does not change shape. Phase 3 is a **caller** around it.

---

## What Phase 3 actually is

Right now the agent runs one task at a time from the CLI:

```bash
cargo run -- "fix the power function in calc.py"
```

Phase 3 adds a **benchmark runner** that:

1. Loads a list of tasks from a file
2. For each task: reset sandbox → run agent → record outcome
3. Parses `logs/run_*.jsonl` into metrics
4. Prints a summary table + failure breakdown

This is the portfolio differentiator — not more tools, but **measurable reliability across tasks**.

---

## What does NOT change

| Component | Phase 3 action |
|-----------|----------------|
| `loop_engine.rs` | **No changes** — same `run()` signature |
| `apply_patch`, tools | **No changes** |
| `LogEntry` schema | **No changes** — parser reads existing JSONL |
| `run_summary` line | **No changes** — parser uses `stop_reason` + `total_steps` |

Everything new lives in a benchmark layer + task definitions.

---

## Oracle (how we judge pass/fail)

**Oracle = the task's test suite**, run in the workspace after the agent finishes (or on each trial's final state).

| Check | How |
|-------|-----|
| Tests pass | `pytest` exits 0 in task workspace |
| Agent claimed success | `run_summary.stop_reason == TaskComplete` |
| **Benchmark pass** | Both: tests pass AND agent didn't hit `MaxStepsExceeded` / `ToolError` |

Watch for **gaming** — agent edits `test_*.py` instead of source. Mitigations:

- RULE in system prompt: never edit test files
- Log audit: flag runs where `ApplyPatch` / `WriteFile` touched a `test_` path
- Task design: tests live in separate files the prompt names as read-only

Phase 4 adds git worktree isolation per task. Phase 3 can copy/reset `test_repo/` per run.

---

## Task format

Tasks live in `benchmarks/tasks.json` (or one `.json` per task in `benchmarks/tasks/`).

```json
{
  "id": "calc-power-zero",
  "tier": 1,
  "workspace": "benchmarks/workspaces/calc-power-zero",
  "goal": "The test test_power in test_calc.py is failing because power() in calc.py has a bug. Read calc.py, identify the bug, fix it with apply_patch, verify with pytest.",
  "verify_cmd": "pytest test_calc.py",
  "files_readonly": ["test_calc.py"]
}
```

### Task tiers (target: 10–20 tasks)

| Tier | Difficulty | Example | Target pass rate |
|------|------------|---------|------------------|
| 1 | Single-line fix | wrong return, wrong operator | 80%+ |
| 2 | Subtle logic bug | edge case (`exp == 0`), wrong branch | 60%+ |
| 3 | Two-part fix | `//` + zero guard in `divide()` | 30%+ |
| 4 | Multi-function | bug in one fn, must not break others | varies |
| 5 | Multi-file (stretch) | fix spans 2 files | stretch goal |

Start with **5 tier-1/2 tasks** cloned from `calc.py` patterns. Expand before chasing SWE-bench scale.

SWE-bench-lite: useful for **task JSON shape** and verification ideas, not required as the task source.

---

## Metrics to track

Per run (from logs):

| Metric | Source |
|--------|--------|
| `task_id` | benchmark metadata (tag in run or separate index file) |
| `run_id` | log filename |
| `model_id` | log lines |
| `stop_reason` | `run_summary` line |
| `total_steps` | `run_summary` line |
| `passed` | oracle: `verify_cmd` exit 0 |
| `duration` | timestamps first/last log line |

Per benchmark sweep (aggregated):

| Metric | Notes |
|--------|-------|
| **Pass rate** | `passed / total_runs` |
| **Avg steps** | on successful runs only |
| **Failure categories** | see below |
| **Cost estimate** | steps × avg tokens (optional; needs token logging later) |

### Failure categories (tag during analysis)

| Category | Signal |
|----------|--------|
| `wrong_file` | patched `test_*.py` or unrelated file |
| `syntax_error` | pytest collection error after patch |
| `patch_failed` | `apply_patch` error in log |
| `max_steps` | `stop_reason: MaxStepsExceeded` |
| `gave_up` | `finish` with tests still failing |
| `api_error` | `stop_reason: ToolError` on step 0 |
| `false_success` | `TaskComplete` but oracle fails |
| `prompt_conflict` | `finish` on step 0–1 with no tools |

The write-up matters: *"60% pass rate, 40% fails — here's exactly why"* beats a higher number with no analysis.

---

## What you build, in order

```
1. benchmarks/tasks.json           ← task definitions (start with 3–5)
2. benchmarks/workspaces/<id>/     ← one broken repo per task
3. src/benchmark/task.rs           ← Task struct + loader
4. src/benchmark/reset.rs          ← copy/reset workspace before each run
5. src/benchmark/runner.rs         ← loop: for task in tasks { run agent }
6. src/benchmark/metrics.rs        ← parse logs/ → RunResult + aggregate
7. src/benchmark/mod.rs
8. src/bin/benchmark.rs            ← CLI: cargo run --bin benchmark
9. Extend src/config.rs            ← BENCHMARK_TASKS, TRIALS, OUTPUT_DIR
10. README benchmark section       ← methodology + results table (stub ok)
```

### Runner pseudocode

```rust
for task in load_tasks("benchmarks/tasks.json")? {
    for trial in 0..trials {
        reset_workspace(&task.workspace)?;
        let state = AgentState::new(task.goal, task.workspace, model, max_steps);
        let reason = loop_engine::run(&mut state, &llm, &system_prompt(), delay).await;
        let oracle_ok = run_verify(&task.verify_cmd, &task.workspace)?;
        record_result(task.id, state.run_id, reason, oracle_ok);
    }
}
print_summary();
```

`main.rs` / `cargo run --bin agent-v1` stays for single manual runs. Benchmark is a **second binary**.

---

## Config additions (.env)

| Variable | Default | Purpose |
|----------|---------|---------|
| `BENCHMARK_TASKS` | `benchmarks/tasks.json` | Task list path |
| `BENCHMARK_TRIALS` | `1` | Runs per task (for flaky model averaging) |
| `BENCHMARK_OUTPUT` | `benchmarks/results/` | Summary JSON + markdown report |
| `WORKSPACE_ROOT` | (per task) | Overridden by runner per task |

---

## Log ↔ task linking

Problem: `logs/run_{uuid}.jsonl` doesn't know which benchmark task produced it.

**Option A (recommended):** write `benchmarks/results/run_index.jsonl` at run start:

```json
{"run_id":"...","task_id":"calc-power-zero","trial":0,"timestamp":"..."}
```

**Option B:** add optional `task_id` field to `run_summary` line (touches log schema — avoid unless necessary).

Use Option A to keep `LogEntry` schema frozen.

---

## Phase 3 scope boundaries

### In scope

- [ ] Task file format + 5 starter tasks
- [ ] Workspace reset between runs
- [ ] `benchmark` binary wrapping `loop_engine::run()`
- [ ] Log parser + pass rate / steps / failure categories
- [ ] Summary output (terminal table + JSON)
- [ ] Gate analysis: detect test-file edits from logs

### Out of scope (Phase 4)

- [ ] Git worktree per task
- [ ] Docker isolation
- [ ] Parallel runs
- [ ] Token/cost tracking from API response
- [ ] `apply_patch_with_validation` (syntax check, rollback)
- [ ] SWE-bench full integration

### Nice to have (if time)

- [ ] Gate `finish` on passing oracle (hardening from Phase 1)
- [ ] Retry on `tool_use_failed`
- [ ] `reset_workspace.sh` helper

---

## Success criteria

Phase 3 is done when you can run:

```bash
cargo run --bin benchmark
```

…and get:

1. All tasks executed sequentially
2. Each run logged to `logs/run_{uuid}.jsonl`
3. Terminal summary: pass rate, avg steps, failure breakdown by category
4. `benchmarks/results/report.json` for README charts
5. At least one written failure analysis paragraph (even if pass rate is low)

---

## Suggested first 5 tasks

| ID | Tier | Bug |
|----|------|-----|
| `calc-add-abs` | 1 | `add()` wraps result in `abs()` |
| `calc-power-zero` | 2 | `power(5, 0)` returns 0 not 1 |
| `calc-cap-100` | 2 | `add()` caps at 100 |
| `calc-divide` | 3 | `/` instead of `//`, zero guard missing |
| `calc-multiply-zero` | 1 | `multiply(a, 0)` returns 1 not 0 |

Each gets its own `benchmarks/workspaces/<id>/` copy of calc.py + test_calc.py.

---

## After Phase 3

**Phase 4:** git worktree per task, README as mini-paper, polish. The benchmark numbers from Phase 3 feed directly into that README.

---

Ready to code Phase 3? Start with `benchmarks/tasks.json` + one workspace, then `src/benchmark/runner.rs`.
