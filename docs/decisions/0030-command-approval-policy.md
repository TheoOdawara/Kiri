# ADR 0030 — Command approval policy: one table, two asymmetric postures

- Status: Accepted
- Date: 2026-07-25
- Supersedes: the `DEFAULT_PLAN_ALLOW` regex list and the `KIRI_PLAN_ALLOW` environment override, both
  removed. ADR 0007 and ADR 0020 mention `KIRI_PLAN_ALLOW` in their rationale; those references are
  historical from this date on — the decisions they record still stand.

## Context

`run_command` answered every approval question with a constant. `Tool::confirm_in_auto` returned `true`
unconditionally, so **auto mode confirmed every shell command** — `git diff`, `ls`, `cargo test`. Auto
mode was not auto; it was default mode with extra steps, and the user reported it as such (issue #23).

Plan mode had the opposite problem's mirror: a list of 33 regexes (`\bgit\b`, `\bcargo\b`, …) matched
against the leading program. It was consulted *only* in plan mode, so the names in it — "plan allow" —
described a surface nobody could relate to the friction they felt in auto. And the regexes matched too
loosely to say anything useful: `\bgit\b` admits `git push` exactly as it admits `git diff`.

The two modes want opposite defaults, and that is the whole difficulty:

- **Plan mode** should be **default-deny**. The model is investigating, often reading repo content it has
  never seen; an unknown program must not run.
- **Auto mode** should be **default-silent**. The user asked for an unattended turn. Interrupting for
  `git diff` trains them to approve without reading, which costs more safety than it buys.

A single allow-list cannot serve both: it either blocks `just build` in auto (friction) or admits an
unknown program while planning (unsafe).

## Decision

One table in `tools/domain/command_policy.rs`, read in two directions. A rule names a program and says
how it may be used:

```rust
pub struct CommandRule {
    pub program: &'static str,                        // matched on the lowercased file stem
    pub always_destructive: bool,                     // rm, sudo, dd…
    pub destructive_subcommands: &'static [&'static str], // git push, cargo install…
    pub inline_code_flags: &'static [&'static str],   // python3 -c, node -e
    pub plan_safety: PlanSafety,                      // No | Any | Subcommands(&[…])
}
```

**Absence from the table is itself the decision, and it is what makes one table serve two postures:** an
unlisted program is *not destructive* (auto runs it silently) and *not plan-safe* (plan refuses it).
`just build` is fluent in auto and refused while planning, with no entry required for either.

Three matching axes, because "which program" is not enough to classify a shell command:

1. **Program**, on `Path::file_stem()` lowercased — `/usr/bin/rm`, `rm.exe`, and `RM` are one program,
   and `python3.11` is `python3`.
2. **Subcommand** — the first non-flag token after the program. `git diff` and `git push` are different
   acts under one binary, and the old regex list could not tell them apart.
3. **Inline-code flag** — `python3 build.py` runs a file that is already in the repo and under review;
   `python3 -c "…"` runs a string the model just invented. The second is the one worth a prompt.

**The chain is scanned, not just the leading program.** A deny-list that judged only the first token
would fall to `echo ok && rm -rf .`. The line is split on `;`, `|`, `&&`, background `&`, and newline
(never on the fd-redirect forms `2>&1` / `>&2` / `&>`, so `cargo build 2>&1` stays one invocation), and
every segment is classified. Destructive programs are matched against **every token** of a segment, not
only its leading one, so a wrapper — `timeout 5 rm -rf x`, `env FOO=1 rm x` — cannot hide the program
that matters. A command substitution (`` ` ``, `$(`, `<(`, `>(`) is not split at all: its contents cannot
be attributed to a program, so the whole line is treated as unclassifiable, which means "confirm".

**Unclassifiable is dangerous.** A deny-list only holds if what it fails to read counts against it: an
empty command, unparseable tool arguments, and a substitution all confirm.

**Overrides only ever add.** `[commands] extra_destructive` and `extra_plan_safe` in the **global**
config (the untrusted project layer contributes only `effort` — ADR 0011/0020, so a hostile repo cannot
declare its own program plan-safe). `extra_destructive` accepts a bare program or a `"program subcommand"`
pair. Neither list can remove a built-in destructive entry, and `extra_plan_safe` grants plan-mode
fluency only — the destructive check runs first and independently, so listing `git` as plan-safe cannot
make `git push` silent.

**Plan mode still confirms every mutating call, on top of the allow-list.** Being admitted says the
program is an investigation tool; it does not say an unattended mutation is acceptable while planning.
The engine (`AgentLoop::plan_checked_run`) keys this on `!Tool::is_read_only`, so a future
mutating-but-plannable tool inherits the gate rather than having to remember it. This preserves SEC-01,
which making auto silent would otherwise have repealed as a side effect.

`Tool::confirm_in_auto` became per-call and now owns the whole decision. It previously shared it with
`Confirmation::default_accept`, which answers a different question — what Enter does *when* a prompt is
shown. Conflating them is why changing one could not fix auto mode without weakening the other:
`run_command` still default-declines every prompt it does show.

### The calibration

- *Always destructive*: `rm` `rmdir` `del` `erase` `mv` `move` `dd` `mkfs` `format` `chmod` `chown`
  `kill` `taskkill` `shutdown` `sudo` `doas` `runas`.
- *Destructive by subcommand*: `git push|reset|clean|rebase|filter-branch` · `cargo install|publish|yank|owner`
  · `npm|pnpm|yarn|bun publish|unpublish` · `docker rm|rmi|prune|kill|stop|system` · `pip install|uninstall`
  · `go install|clean` · `uv add|remove|publish` · `rustup self|uninstall` · `deno eval`.
- *Destructive by inline-code flag*: `python3 -c` · `node -e|--eval|-p|--print` · `bun -e` · `perl -e|-E`
  · `ruby -e`.
- *Plan-safe*: the read-only inspection set (`ls` `cat` `head` `tail` `wc` `echo` `pwd` `which` `env`
  `printenv` `tree` `stat` `file` `grep` `rg` `find` `fd` `rustc` `npx`), plus the non-mutating
  subcommands of `git`/`cargo`/`rustup`/`docker`/`npm`-family/`pip`, plus the interpreters and build
  tools pointed at a file (`python3` `node` `deno` `bun` `perl` `ruby` `make` `go` `uv`).

**`git commit` is deliberately not destructive.** It is local and reversible, so under this policy it
runs unattended in auto. Only `git push` leaves the machine. Anyone who wants the prompt adds
`extra_destructive = ["git commit"]` — the pair form exists for exactly this.

## Consequences

- Auto mode is usable: `git diff`, `cargo test`, `ls`, and an unrecognized project script run without
  interrupting. Deleting a path, moving one, and an irreversible command still prompt.
- Plan mode is stricter than it was. `\bgit\b` used to admit `git push`; now only the listed
  subcommands are admitted, and everything unlisted (`curl`, `docker run`, `just`) is refused rather
  than merely confirmed.
- **The deny-list runs without OS confinement on Windows until ADR 0031's restricted token ships.** On
  a Windows host, an unlisted program that is destructive in a way the table does not know about runs
  silently in auto with no sandbox underneath it. This is an accepted, time-boxed risk of shipping the
  policy on all three v1 platforms at once; macOS (Seatbelt) and Linux (bwrap) already have the
  underlying confinement.
- The splitter is quote-blind by design: `echo "a; rm -rf x"` splits and confirms. A false positive in
  the safe direction is the only kind a deny-list may produce, so this is not a defect to fix.
- It is a heuristic, not a shell parser, and is not sold as a boundary. The OS confinement layer
  (ADR 0009/0031) is the real control; this decides who gets interrupted.
- `DEFAULT_RW_DIRS` gained `~/.bun`, `~/.deno`, `~/.pnpm-store`, and `~/.local/share/uv` so a toolchain
  the policy admits can reach its own cache — an approved command failing under confinement reads as a
  harness bug, not as a policy decision.
