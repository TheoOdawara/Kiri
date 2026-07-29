//! Approval rules for shell commands. Two postures from one table (ADR 0030): auto mode is a
//! **deny-list** — an unlisted program runs silently, only a destructive one asks; plan mode is an
//! **allow-list** — an unlisted program is refused. Not to be confused with
//! `application::command_sandbox::SandboxPolicy`, which is OS confinement, not approval.

use std::path::Path;

/// How a program may be used in plan mode.
#[derive(Debug, Clone, Copy)]
pub enum PlanSafety {
    /// Never admitted while planning.
    No,
    /// Admitted for any invocation that is not destructive.
    Any,
    /// Admitted only for these subcommands — `git diff` investigates, `git commit` changes the repo.
    Subcommands(&'static [&'static str]),
}

/// One program's approval rule. Absence from [`RULES`] is itself a decision: not destructive (so auto
/// runs it silently) and not plan-safe (so plan refuses it).
#[derive(Debug)]
pub struct CommandRule {
    /// Matched against the invoked program's lowercased file stem, so `/usr/bin/rm`, `rm.exe`, and
    /// `python3.11` reduce to `rm`, `rm`, and `python3`.
    pub program: &'static str,
    /// Destructive however it is invoked.
    pub always_destructive: bool,
    /// Destructive only for these subcommands.
    pub destructive_subcommands: &'static [&'static str],
    /// Destructive only when one of these flags is present: the flag means "execute this string" rather
    /// than "execute a file that is already in the project", and the string came from the model.
    pub inline_code_flags: &'static [&'static str],
    pub plan_safety: PlanSafety,
}

/// Irreversible however invoked: never unattended, never while planning.
const fn destructive(program: &'static str) -> CommandRule {
    CommandRule {
        program,
        always_destructive: true,
        destructive_subcommands: &[],
        inline_code_flags: &[],
        plan_safety: PlanSafety::No,
    }
}

/// Reads or reports, never writes: the fluent core of plan mode.
const fn inspect(program: &'static str) -> CommandRule {
    CommandRule {
        program,
        always_destructive: false,
        destructive_subcommands: &[],
        inline_code_flags: &[],
        plan_safety: PlanSafety::Any,
    }
}

/// Runs project code — a build tool or an interpreter pointed at a file in the repo. Plan-safe (the same
/// trust `cargo test` gets), with the mutating subcommands and inline-code flags called out per entry.
const fn runs_project_code(
    program: &'static str,
    destructive_subcommands: &'static [&'static str],
    inline_code_flags: &'static [&'static str],
) -> CommandRule {
    CommandRule {
        program,
        always_destructive: false,
        destructive_subcommands,
        inline_code_flags,
        plan_safety: PlanSafety::Any,
    }
}

/// The built-in table. Only what a coding session actually reaches for: everything else falls to the
/// unlisted default, which is the safe combination in both directions.
const RULES: &[CommandRule] = &[
    destructive("rm"),
    destructive("rmdir"),
    destructive("del"),
    destructive("erase"),
    destructive("mv"),
    destructive("move"),
    destructive("dd"),
    destructive("mkfs"),
    destructive("format"),
    destructive("chmod"),
    destructive("chown"),
    destructive("kill"),
    destructive("taskkill"),
    destructive("shutdown"),
    // Privilege escalation is destructive in itself: whatever follows runs outside every guarantee the
    // harness makes, so the wrapper must be approved even though the wrapped program may look benign.
    destructive("sudo"),
    destructive("doas"),
    destructive("runas"),
    inspect("ls"),
    inspect("dir"),
    inspect("cat"),
    inspect("head"),
    inspect("tail"),
    inspect("wc"),
    inspect("echo"),
    inspect("pwd"),
    inspect("which"),
    inspect("env"),
    inspect("printenv"),
    inspect("tree"),
    inspect("stat"),
    inspect("file"),
    inspect("grep"),
    inspect("rg"),
    inspect("find"),
    inspect("fd"),
    CommandRule {
        program: "git",
        always_destructive: false,
        // Only what cannot be undone from the reflog or a local checkout. `commit` is absent on purpose:
        // it is local and reversible, so under a deny-list it runs unattended.
        destructive_subcommands: &["push", "reset", "clean", "rebase", "filter-branch"],
        inline_code_flags: &[],
        plan_safety: PlanSafety::Subcommands(&[
            "diff",
            "status",
            "log",
            "show",
            "branch",
            "remote",
            "rev-parse",
            "blame",
            "ls-files",
        ]),
    },
    CommandRule {
        program: "cargo",
        always_destructive: false,
        destructive_subcommands: &["install", "publish", "yank", "owner"],
        inline_code_flags: &[],
        plan_safety: PlanSafety::Subcommands(&[
            "check", "build", "test", "clippy", "fmt", "tree", "metadata",
        ]),
    },
    CommandRule {
        program: "rustup",
        always_destructive: false,
        destructive_subcommands: &["self", "uninstall"],
        inline_code_flags: &[],
        plan_safety: PlanSafety::Subcommands(&["show", "which", "toolchain", "component"]),
    },
    inspect("rustc"),
    CommandRule {
        program: "docker",
        always_destructive: false,
        destructive_subcommands: &["rm", "rmi", "prune", "kill", "stop", "system"],
        inline_code_flags: &[],
        plan_safety: PlanSafety::Subcommands(&["ps", "images", "logs", "inspect"]),
    },
    // Package managers: publishing is the irreversible act; the rest is ordinary build/test traffic.
    npm_like("npm"),
    npm_like("pnpm"),
    npm_like("yarn"),
    inspect("npx"),
    CommandRule {
        program: "pip",
        always_destructive: false,
        destructive_subcommands: &["install", "uninstall"],
        inline_code_flags: &[],
        plan_safety: PlanSafety::Subcommands(&["list", "show", "freeze"]),
    },
    CommandRule {
        program: "pip3",
        always_destructive: false,
        destructive_subcommands: &["install", "uninstall"],
        inline_code_flags: &[],
        plan_safety: PlanSafety::Subcommands(&["list", "show", "freeze"]),
    },
    runs_project_code("uv", &["add", "remove", "publish"], &[]),
    runs_project_code("python", &[], &["-c"]),
    runs_project_code("python3", &[], &["-c"]),
    runs_project_code("node", &[], &["-e", "--eval", "-p", "--print"]),
    runs_project_code("deno", &["eval"], &[]),
    runs_project_code("bun", &["publish", "unpublish"], &["-e"]),
    runs_project_code("perl", &[], &["-e", "-E"]),
    runs_project_code("ruby", &[], &["-e"]),
    runs_project_code("make", &[], &[]),
    runs_project_code("go", &["install", "clean"], &[]),
];

/// `publish` is the one act that cannot be taken back; `run`/`test` are how a JS project is built.
const fn npm_like(program: &'static str) -> CommandRule {
    CommandRule {
        program,
        always_destructive: false,
        destructive_subcommands: &["publish", "unpublish"],
        inline_code_flags: &[],
        plan_safety: PlanSafety::Subcommands(&["run", "test", "ls", "list", "outdated", "why"]),
    }
}

fn rule_for(program: &str) -> Option<&'static CommandRule> {
    RULES.iter().find(|rule| rule.program == program)
}

/// The invoked program's lowercased file stem, so a path, a `.exe`, and a version suffix all reduce to
/// the name the table lists.
fn stem(token: &str) -> String {
    Path::new(token)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(token)
        .to_ascii_lowercase()
}

/// A `NAME=value` prefix, which is shell setup rather than the invoked program.
fn is_env_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Whether the line contains a substitution construct whose contents cannot be attributed to a program.
/// Unclassifiable is treated as dangerous, so this is what keeps `echo $(rm -rf x)` out of silent auto.
fn has_substitution(command: &str) -> bool {
    command.contains('`')
        || command.contains("$(")
        || command.contains("<(")
        || command.contains(">(")
}

/// Split a command line on the separators that introduce another program (`;`, `|`, `&&`, background
/// `&`, newline), dropping blank pieces. A `&` is a separator unless the next byte is an ASCII digit or
/// `>` — the fd-redirect forms (`2>&1`, `>&2`, `&>`), which must not split so `cargo build 2>&1` stays
/// one invocation.
///
/// Deliberately quote-blind: `echo "a; rm -rf x"` splits and the `rm` piece forces a confirmation. That
/// is a false positive in the safe direction, which is the only direction a splitter backing a deny-list
/// may err in. Separators are all ASCII, so every slice lands on a char boundary.
fn segments(command: &str) -> Vec<&str> {
    let bytes = command.as_bytes();
    let mut pieces = Vec::new();
    let mut start = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        let separates = match byte {
            b';' | b'|' | b'\n' | b'\r' => true,
            b'&' => match bytes.get(index + 1) {
                None => true,
                Some(next) => !next.is_ascii_digit() && *next != b'>',
            },
            _ => false,
        };
        if separates {
            pieces.push(&command[start..index]);
            start = index + 1;
        }
    }
    pieces.push(&command[start..]);
    pieces
        .into_iter()
        .filter(|piece| !piece.trim().is_empty())
        .collect()
}

/// One program invocation from a command line: the program, its subcommand, and every token (kept
/// because a wrapper hides the real program in a later one).
struct Invocation<'a> {
    program: String,
    subcommand: Option<String>,
    tokens: Vec<&'a str>,
}

impl<'a> Invocation<'a> {
    /// `None` when the segment names no program (blank, or only env assignments).
    fn parse(segment: &'a str) -> Option<Self> {
        let tokens: Vec<&str> = segment.split_whitespace().collect();
        let leading = tokens.iter().position(|token| !is_env_assignment(token))?;
        let subcommand = tokens[leading + 1..]
            .iter()
            .find(|token| !token.starts_with('-'))
            .map(|token| token.to_ascii_lowercase());
        Some(Self {
            program: stem(tokens[leading]),
            subcommand,
            tokens,
        })
    }
}

/// The built-in [`RULES`] plus the trusted global config's additive overrides. Overrides only ever add:
/// `extra_plan_safe` cannot make a destructive command silent, and nothing can remove an entry from the
/// built-in destructive set.
#[derive(Debug, Default)]
pub struct CommandPolicy {
    /// A bare program name, or `"program subcommand"` to harden one subcommand only.
    extra_destructive: Vec<String>,
    /// Program names admitted in plan mode as if they were [`PlanSafety::Any`].
    extra_plan_safe: Vec<String>,
}

fn normalize(entries: &[String]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| entry.trim().to_ascii_lowercase())
        .filter(|entry| !entry.is_empty())
        .collect()
}

impl CommandPolicy {
    pub fn new(extra_destructive: &[String], extra_plan_safe: &[String]) -> Self {
        Self {
            extra_destructive: normalize(extra_destructive),
            extra_plan_safe: normalize(extra_plan_safe),
        }
    }

    /// Whether this command line must be confirmed even in auto mode. `true` for anything it cannot
    /// classify: a deny-list only holds if what it fails to read counts as dangerous.
    #[cfg(test)]
    pub fn needs_confirmation(&self, command: &str) -> bool {
        if has_substitution(command) {
            return true;
        }
        let invocations: Vec<Invocation> = segments(command)
            .into_iter()
            .filter_map(Invocation::parse)
            .collect();
        if invocations.is_empty() {
            return true;
        }
        invocations
            .iter()
            .any(|invocation| self.is_destructive(invocation))
    }

    /// Why plan mode refuses this command, or `None` when it may run. Allow-list semantics: an unlisted
    /// program is refused, and so is a chain — an admitted program must characterize the whole line.
    pub fn plan_refusal(&self, command: &str) -> Option<String> {
        if has_substitution(command) {
            return Some(plan_message(
                "it expands a command substitution, so the program it runs cannot be checked",
            ));
        }
        let pieces = segments(command);
        if pieces.len() > 1 {
            return Some(plan_message(
                "it chains more than one program, so an allowed prefix cannot vouch for the rest",
            ));
        }
        let Some(invocation) = pieces.first().and_then(|piece| Invocation::parse(piece)) else {
            return Some(plan_message("it names no program to run"));
        };
        if self.is_destructive(&invocation) {
            return Some(plan_message(&format!(
                "'{}' is destructive here; plan mode investigates only",
                invocation.program
            )));
        }
        if self.plan_admits(&invocation) {
            None
        } else {
            Some(plan_message(&format!(
                "'{}' is not in the plan-mode allow-list (read-only investigation and build/test \
                 commands only)",
                invocation.program
            )))
        }
    }

    fn is_destructive(&self, invocation: &Invocation) -> bool {
        // Every token, not only the leading program: a wrapper (`timeout 5 rm -rf x`, `xargs rm`) would
        // otherwise hide the destructive program behind a benign one.
        if invocation
            .tokens
            .iter()
            .any(|token| self.is_always_destructive(&stem(token)))
        {
            return true;
        }
        let rule = rule_for(&invocation.program);
        if let Some(subcommand) = &invocation.subcommand
            && (rule
                .is_some_and(|rule| rule.destructive_subcommands.contains(&subcommand.as_str()))
                || self.extra_hardens(&invocation.program, subcommand))
        {
            return true;
        }
        rule.is_some_and(|rule| {
            invocation
                .tokens
                .iter()
                .any(|token| rule.inline_code_flags.contains(token))
        })
    }

    fn is_always_destructive(&self, program: &str) -> bool {
        rule_for(program).is_some_and(|rule| rule.always_destructive)
            || self.extra_destructive.iter().any(|entry| entry == program)
    }

    /// Whether an override hardens exactly this `program subcommand` pair.
    fn extra_hardens(&self, program: &str, subcommand: &str) -> bool {
        self.extra_destructive.iter().any(|entry| {
            entry
                .split_once(' ')
                .is_some_and(|(prog, sub)| prog == program && sub.trim() == subcommand)
        })
    }

    fn plan_admits(&self, invocation: &Invocation) -> bool {
        if self
            .extra_plan_safe
            .iter()
            .any(|entry| entry == &invocation.program)
        {
            return true;
        }
        match rule_for(&invocation.program).map(|rule| rule.plan_safety) {
            Some(PlanSafety::Any) => true,
            Some(PlanSafety::Subcommands(allowed)) => invocation
                .subcommand
                .as_deref()
                .is_some_and(|subcommand| allowed.contains(&subcommand)),
            Some(PlanSafety::No) | None => false,
        }
    }
}

fn plan_message(reason: &str) -> String {
    format!("blocked in plan mode: {reason}. Run it outside plan mode.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> CommandPolicy {
        CommandPolicy::default()
    }

    #[test]
    fn ordinary_work_runs_unattended_in_auto() {
        // Issue #23: auto mode used to confirm every shell command, `git diff` included, which made the
        // mode useless. The deny-list default is silence.
        let policy = policy();
        for command in [
            "git diff",
            "git status --short",
            "git commit -F msg.txt",
            "ls -la src",
            "cargo test",
            "cargo build 2>&1",
            "rg needle src",
            "just build",
            "./scripts/deploy.sh",
            "echo hi > out.txt",
        ] {
            assert!(
                !policy.needs_confirmation(command),
                "must run silently in auto: {command}"
            );
        }
    }

    #[test]
    fn irreversible_work_still_asks_in_auto() {
        let policy = policy();
        for command in [
            "rm -rf build",
            "git push origin main",
            "git reset --hard HEAD~1",
            "cargo publish",
            "npm publish",
            "docker rm kiri",
            "sudo systemctl restart nginx",
            "chmod 777 /etc",
        ] {
            assert!(
                policy.needs_confirmation(command),
                "must ask even in auto: {command}"
            );
        }
    }

    #[test]
    fn a_chain_is_judged_by_every_link() {
        // The deny-list would be worthless if `echo ok && rm -rf x` inherited `echo`'s silence.
        let policy = policy();
        assert!(policy.needs_confirmation("echo ok && rm -rf x"));
        assert!(policy.needs_confirmation("cargo test; rm -rf target"));
        assert!(policy.needs_confirmation("ls | xargs rm"));
        assert!(policy.needs_confirmation("cargo build &rm -rf x"));
        // ...while a chain of benign links stays silent, and backgrounding one adds no program to judge.
        assert!(!policy.needs_confirmation("cargo fmt && cargo test"));
        assert!(!policy.needs_confirmation("cargo build &"));
    }

    #[test]
    fn an_fd_redirect_is_not_a_chain() {
        // `2>&1` must not read as a background `&`, or every build command would confirm.
        assert_eq!(segments("cargo build 2>&1"), vec!["cargo build 2>&1"]);
        assert_eq!(segments("cargo build >&2"), vec!["cargo build >&2"]);
        assert_eq!(segments("cargo build &> log"), vec!["cargo build &> log"]);
    }

    #[test]
    fn an_unclassifiable_command_asks() {
        // A substitution can run anything, and the splitter cannot attribute it to a program.
        let policy = policy();
        assert!(policy.needs_confirmation("echo $(rm -rf x)"));
        assert!(policy.needs_confirmation("echo `rm -rf x`"));
        assert!(policy.needs_confirmation("diff <(ls a) <(ls b)"));
        // An empty line names no program at all.
        assert!(policy.needs_confirmation("   "));
        assert!(policy.needs_confirmation(""));
    }

    #[test]
    fn a_wrapper_cannot_hide_the_destructive_program() {
        let policy = policy();
        assert!(policy.needs_confirmation("timeout 5 rm -rf x"));
        assert!(policy.needs_confirmation("env FOO=1 rm -rf x"));
        assert!(policy.needs_confirmation("FOO=1 rm -rf x"));
        assert!(policy.needs_confirmation("/usr/bin/rm -rf x"));
        assert!(policy.needs_confirmation("RM -rf x"));
    }

    #[test]
    fn an_interpreter_asks_only_for_inline_code() {
        // Running a file in the repo is ordinary work; running a string the model just invented is not.
        let policy = policy();
        assert!(!policy.needs_confirmation("python3 build.py"));
        assert!(!policy.needs_confirmation("node scripts/gen.js"));
        assert!(policy.needs_confirmation("python3 -c \"print(1)\""));
        assert!(policy.needs_confirmation("node -e \"require('fs')\""));
        assert!(policy.needs_confirmation("deno eval \"Deno.exit()\""));
        assert!(policy.needs_confirmation("perl -e \"unlink 'x'\""));
    }

    #[test]
    fn plan_mode_admits_investigation_and_refuses_the_rest() {
        let policy = policy();
        for command in [
            "ls -la",
            "cat Cargo.toml",
            "rg needle src",
            "git diff",
            "git log --oneline -5",
            "cargo check",
            "cargo test",
            "python3 build.py",
            "npm test",
        ] {
            assert!(
                policy.plan_refusal(command).is_none(),
                "plan mode must allow: {command}"
            );
        }
        for command in [
            "rm -rf x",
            "git push",
            "cargo install cargo-audit",
            "./evil-cat",
            "just build",
            "docker run alpine",
            "python3 -c \"print(1)\"",
            "cargo test && rm -rf x",
            "echo $(whoami)",
            "   ",
        ] {
            assert!(
                policy.plan_refusal(command).is_some(),
                "plan mode must refuse: {command}"
            );
        }
    }

    #[test]
    fn plan_refusal_names_the_program_and_stays_one_line() {
        let refusal = policy().plan_refusal("./evil-cat").unwrap();
        assert!(refusal.contains("blocked in plan mode"), "got: {refusal}");
        assert!(refusal.contains("evil-cat"), "got: {refusal}");
        assert!(!refusal.contains('\n'), "got: {refusal}");
    }

    #[test]
    fn extra_destructive_hardens_by_program_and_by_subcommand() {
        let policy = CommandPolicy::new(&["just".to_string(), "git commit".to_string()], &[]);
        assert!(policy.needs_confirmation("just deploy"));
        assert!(policy.needs_confirmation("git commit -m x"));
        // The pair form hardens only that pair.
        assert!(!policy.needs_confirmation("git diff"));
        // ...and a hardened program is refused in plan mode too, even though it was unlisted before.
        assert!(policy.plan_refusal("just deploy").is_some());
    }

    #[test]
    fn extra_plan_safe_grants_plan_fluency_without_granting_silence() {
        let policy = CommandPolicy::new(&[], &["just".to_string()]);
        assert!(policy.plan_refusal("just build").is_none());
        // Plan fluency is not a destructive-set exemption: `git push` still asks, and `rm` still does.
        assert!(policy.needs_confirmation("just build && rm -rf x"));
    }

    #[test]
    fn no_override_can_silence_a_built_in_destructive_command() {
        // Locked decision: overrides only ever add. Listing `git`/`rm` as plan-safe must not make an
        // irreversible invocation run unattended — otherwise a config edit could disarm the deny-list.
        let policy = CommandPolicy::new(
            &[],
            &["git".to_string(), "rm".to_string(), "sudo".to_string()],
        );
        assert!(policy.needs_confirmation("git push origin main"));
        assert!(policy.needs_confirmation("rm -rf /"));
        assert!(policy.needs_confirmation("sudo rm x"));
        // Plan mode still refuses them: `plan_admits` is consulted only after the destructive check.
        assert!(policy.plan_refusal("git push").is_some());
        assert!(policy.plan_refusal("rm -rf /").is_some());
    }

    #[test]
    fn the_table_has_no_duplicate_program() {
        // Two entries for one program would make `rule_for` silently honor the first and ignore the
        // second, so a later hardening line would look applied while doing nothing.
        let mut programs: Vec<&str> = RULES.iter().map(|rule| rule.program).collect();
        programs.sort_unstable();
        let count = programs.len();
        programs.dedup();
        assert_eq!(count, programs.len(), "duplicate program in RULES");
    }

    #[test]
    fn an_always_destructive_rule_is_never_plan_safe() {
        for rule in RULES.iter().filter(|rule| rule.always_destructive) {
            assert!(
                matches!(rule.plan_safety, PlanSafety::No),
                "{} is always destructive, so plan mode must never admit it",
                rule.program
            );
        }
    }

    #[test]
    fn a_plan_safe_subcommand_is_never_also_destructive() {
        // The two lists are read in sequence (destructive first), so an overlap would be a dead entry in
        // the plan-safe list — and a reader would take the wrong one as the effective answer.
        for rule in RULES {
            let PlanSafety::Subcommands(allowed) = rule.plan_safety else {
                continue;
            };
            for subcommand in allowed {
                assert!(
                    !rule.destructive_subcommands.contains(subcommand),
                    "{} {subcommand} is listed as both plan-safe and destructive",
                    rule.program
                );
            }
        }
    }
}
