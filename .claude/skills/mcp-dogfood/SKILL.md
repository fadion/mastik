---
name: mcp-dogfood
description: Mastik's MCP self-improvement loop — a headless Opus tester designs a realistic page through the MCP of a fresh build, reports where the tools got in its way, and this session validates each finding, fixes what holds, rebuilds and runs again. Takes the number of runs ("/mcp-dogfood 5 runs"; default 3, at most 10) and stops early after a run with no meaningful findings. Use when the user asks to dogfood, exercise, stress or improve the MCP tools by using them, or to "run the MCP loop". Not for reviewing MCP code without using it (that's /code-review) or for one tool's bug the user already named.
---

# The MCP dogfood loop

Each run: **build → serve → tester → validate → fix → gates → commit**, then the next run on the
new build.

The tester is a separate `claude -p` process, not an `Agent` subagent. A subagent shares this
session's MCP connection, and with it a tool list read from an older build. A fresh process reads
the tools of the build it was started on.

`scripts/run-tester.ps1` does one run's mechanics. This session does the judging.

## Arguments

- **Runs:** the number written with "runs" ("5 runs"), else a bare number outside quotes. With
  none, 3; never more than 10.
- **"no commits":** leave every fix in the tree instead of committing it.
- **"all tools":** the tester's proxy lists every tool (`--tools all`), not the core set. By
  default it sees what a person's agent sees.
- **A brief** in quotes is used for every run instead of the pool. Write it to a file in the
  scratchpad and pass `-BriefFile`.

Without "no commits", invoking this skill is the ask to commit. Each validated fix is committed
on its own through the `commit` skill, so a later run never tests a half-finished change.

## Phase 0: the ground (once)

1. **Check the tree with `git status --short`.** It must be clean apart from this skill's own
   folder. Other uncommitted work means someone has something in flight: stop and ask.
2. **List the hosts** with `run-tester.ps1 -Check`. If a Mastik window is listed, ask the person
   to close it, and never close it yourself. The proxy prefers the window, so the tester would
   edit their open window, running whatever build the window was started from.
3. **Don't call this session's own `mastik` MCP tools while the loop runs.** That proxy is a
   stale copy in `%LOCALAPPDATA%\Mastik-mcp`.
4. **Read `rejected.md`.** Its findings were judged and declined. A run that reports one again
   does not count it as new.
5. **Pick a start offset**, a random number from 0 to 7 kept for this invocation. Run *n* uses
   brief `offset + n - 1`, so runs rotate and a second invocation does not replay the first.

## Phase 1: one run

Start the run in the background with `run_in_background`, and wait for the notification:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .claude/skills/mcp-dogfood/scripts/run-tester.ps1 -Run <n> -BriefIndex <offset + n - 1> -Out <scratchpad>\dogfood\run<n> [-AllTools] [-BriefFile <path>] [-Photos <folder>]
```

**What the script does.** It:
- stops the serves from the fresh build or from the MCP copy. It refuses with exit 4 if a serve
  from any other install is running, because that is someone's work;
- builds `target\release`;
- seeds `page.mastik` with an empty Luau script through `mastik run`;
- copies in photos from `-Photos`, else `$env:MASTIK_DOGFOOD_PHOTOS`, and names them in the
  prompt. With none, pass 2 has nothing to place, and the script warns;
- starts a `serve` from the fresh build for the tester's proxy to find;
- runs the tester on `tester.md`, from the run folder, with:
  - no user or project settings and no skills;
  - only the MCP tools and ToolSearch;
  - the model alias `opus`;
  - a $20 budget (`-BudgetUsd`).

**Exit codes:**

| Code | Meaning | Do |
| --- | --- | --- |
| 0 | A report was collected. | Validate it. |
| 3 | A window is running. | Ask the person to close it. |
| 4 | A serve from another build is running. | Ask the person about it. |
| 5 | A window or another host appeared during the run (`invalid.txt`). | Discard the run and run it again. |
| 6 | The tester ended without a report: a crash, the budget, or no JSON block. | Read `answer.json`, then fix the cause or run it again. It is **not** a clean run. |

A run past an hour is hung: stop it and treat it as exit 6.

**The run folder holds:**
- `prompt.md`;
- `answer.json`, `claude`'s JSON result;
- `report.md` and `findings.json`;
- `transcript.jsonl`;
- `calls.jsonl`: one line per tool call, holding its number, tool, arguments, whether it failed,
  and the start of its answer;
- `page.mastik` and `page.png`.

**Look at `page.png`.** A page that is not a realistic, finished design means the run did not
test what it should. Say why, from the transcript.

## Phase 2: validate

Validate every finding in `findings.json`:

1. **Find its call in `calls.jsonl`.** The tester's account can be wrong, so read the exact
   arguments and answer. If no call matches, the finding is rejected.
2. **Reproduce it on the fresh build.** The normal route is a small headless run, with
   `-BriefFile` holding two or three lines that make only that call. This session's MCP is
   stale, and the transcript's ids are short ids (`n48c_36`), which only the proxy's connection
   reads.

   For a batch write there is an offline route: `target\release\mastik.exe run <steps.json> --in
   <copy of page.mastik> -o out.mastik`. It needs full ids from `mastik inspect <file> outline`.
   It also starts from the finished page, not from the state the call saw.
3. **Judge it.**

   A finding **holds** when a typical agent would make the same mistake, or would need the
   missing piece for real work.

   It is **rejected** when any of these is true:
   - the tester misread a description that is clear;
   - it asks for something `docs/non-goals.md` declines;
   - it contradicts the design (ask `design-oracle` when unsure);
   - it is one sentence of wording with no failure behind it.

   Before agreeing to a merge or a rename, read `docs/design/mcp.md` *Core*: a rename breaks
   every agent's memory of the old name.

   An error that comes from the client and not from Mastik (such as "No such tool available")
   holds only when Mastik's own descriptions or instructions could have prevented it.
4. **Record a rejection** in `rejected.md` as one line. Name the capability, not the tool, so
   the line survives a rename, and say why the finding does not hold. A rejection that is a
   design decision goes to `docs/non-goals.md` or `docs/decisions.md` instead, as `AGENTS.md`
   says.

**A finding is meaningful** when it holds and is none of these: cosmetic wording, a `Keep:`
note, or a repeat of `rejected.md`.

**A run is clean** only when all three are true: the script exited 0, `page.png` shows a
finished page, and no finding is meaningful.

## Phase 3: fix

For each finding that holds, most damaging first:

1. **Fix it where the design puts it:**
   - the verb or its parameters: `mastik-core::verbs`, `mastik-mcp::schema`;
   - the handler: `mastik-app::mcp`;
   - the instructions every agent reads: `mastik_mcp::wire::INSTRUCTIONS`.

   Follow `AGENTS.md`: exact-match edits, a test per behaviour with a flip-check, no history in
   comments.
2. **Update the design docs** when a fix changes a rule that `docs/design/mcp.md` states. Brief
   `arch-scribe` with what changed and why.
3. **Run the gates** with `rust-verify`.
4. **Re-test the fix** with the Phase 2 reproduction, on a build that has it.
   `run-tester.ps1 -NoBuild` skips the build when you have just built.
5. **Commit it on its own** with the `commit` skill, as `fix(mcp): …` or `feat(mcp): …`, unless
   "no commits" was given. A `rejected.md` change goes in that run's last commit, or in a
   `chore(claude): …` commit of its own if nothing else was committed.

A finding too large for the loop goes to `docs/roadmap.md` as open work, not into a rushed fix.

## Phase 4: next or stop

- **Stop** after a clean run, or after the last run.
- **Otherwise**, go back to Phase 1 with *n + 1*. The script rebuilds, so the next tester sees
  this run's fixes.

At the end:
- report each run: its brief, its findings (held, fixed, rejected, deferred), and its commits;
- say what stopped the loop;
- show the last run's `page.png` with SendUserFile where that tool exists; otherwise give its
  path;
- confirm with `run-tester.ps1 -Check` that no serve is left running.

## Keeping the skill current

- **The tester prompt names no tool.** It asks for capabilities, so tools can be merged and
  renamed without the skill going stale.
- **When a brief stops reaching an area,** change `briefs.md`. The script reads its
  `## Pool` and `## The passes` lists, so keep those headings.
- **When the CLI used for reproducing changes,** edit Phase 2.
