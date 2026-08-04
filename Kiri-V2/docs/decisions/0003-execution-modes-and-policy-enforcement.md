# 0003. Execution modes and policy enforcement

- Status: accepted
- Date: 2026-08-04

## Context

Kiri needs Claude-like freedom across arbitrary language ecosystems while
retaining Codex-like organization and explicit security boundaries. A fixed
allowlist for Rust, JavaScript, Python, or any other language would become an
unmaintainable substitute for a real execution policy.

The harness must also support long-running commands without making a mode
change stop the user's work. Project workflow customization must remain
possible without allowing a repository to weaken the harness.

## Decision

### One policy engine for every action

Filesystem operations, processes, shell commands, network access, skills,
agents, commands, hooks, validators, and external tools all pass through one
typed policy flow:

```text
classify effects -> resolve rules -> check trust and mode
-> create sandbox envelope -> execute -> redact output -> audit
```

The policy engine classifies effects rather than programming languages. Relevant
effect categories include:

- filesystem read, write, and delete;
- process creation and child-process activity;
- network read and write;
- remote-state mutation;
- credential access;
- destructive or irreversible behavior.

No skill, agent, hook, command, or external tool can bypass this engine.

### Execution modes

Kiri provides three modes:

#### `Plan`

- Reads are permissive except for sensitive values.
- Sensitive files and sensitive content expose only safe metadata, such as
  presence, names, schema, and size. Values are replaced with `[REDACTED]` in
  every output, including Git history and command output.
- Commands are not filtered by language or package manager. A command may run
  when the sandbox guarantees repository read access, temporary writes only,
  and no arbitrary network access.
- Temporary outputs may be written to Kiri-controlled disposable directories;
  repository files must not be changed.
- Remote reads are available only through a Kiri-controlled client/proxy or a
  recognized read-only operation. Arbitrary process network access is not
  granted automatically.
- Commands and providers receive opaque credential references, never the full
  environment or raw secret values.

#### `Default`

Every action requests permission. Persistent approvals remain bounded by the
project, executable, normalized arguments, working directory, paths, domains,
and capabilities they cover. An approval never expands a hard security deny.

#### `Auto`

Routine actions do not prompt. A classifier and the sandbox provide the safety
boundary:

- `allow` executes without interruption;
- `ask` opens the normal permission prompt;
- `deny` blocks the action without authorization for critical risks or
  immutable harness invariants.

Classifier uncertainty never produces `allow`.

### User authority and workflow context

An explicit user request can override contextual workflow instructions, such as
repository instruction files, rules, skills, agents, and an approved plan. Kiri
warns what context is being overridden and what will happen next.

An explicit request still passes through the policy engine, sandbox, credential
broker, mandatory validators, and hard security denies. It cannot disable those
controls through natural language.

### Command execution

Kiri supports both:

- structured execution with `program + args`, which is the default;
- shell execution for scripts and compound commands.

Shell input is parsed before execution. Each subcommand, pipeline,
redirection, and wrapper is classified independently. Shell syntax cannot hide
an operation from the policy engine.

There is no language-specific command allowlist. Tool-specific recognizers may
identify safe or remote-read operations, but they are policy data and not
language adapters. An unknown command may execute in `Plan` only when the
sandbox and network policy prove that it cannot mutate the repository or reach
an unauthorized network destination.

### Credentials and network

Network capabilities are separate:

- `no_network`;
- `read_remote`;
- `write_remote`.

Repository configuration cannot enable network access. Commands that need
credentials receive only the specific opaque credential reference approved for
that operation. Raw environment values never enter model context or
unrecognized subprocesses. All command and network output is redacted before
it is returned to the model or written to audit logs.

### Sandbox and path enforcement

The sandbox is an OS-enforced boundary. Path validation must:

- normalize parent traversal;
- resolve symlinks, junctions, and reparse points;
- validate the nearest existing parent for new paths;
- compare resolved paths with authorized roots;
- revalidate at operation time.

The implementation may use a cheap lexical check first, cache canonical roots,
and validate directory trees incrementally, but caching never replaces the
security check. The OS sandbox is the final enforcement layer.

If the sandbox is unavailable, `Plan` and `Auto` are unavailable. `Default` may
continue only with an explicit unsandboxed-execution warning for each command.
Kiri never falls back silently to unrestricted execution.

Every process also has mandatory resource limits: timeout and cancellation,
process-tree termination when explicitly stopped, output limits, temporary
storage limits, and child-process limits. Repository configuration cannot
increase these limits.

### Mode changes and running processes

Mode changes apply immediately to the session policy and every new action.
Each running process keeps the sandbox envelope and limits it received at
startup. Changing modes does not automatically stop it or expand its
permissions. The UI makes the process's original mode visible. Stopping a
running process is a separate explicit action.

### Trust and project-defined execution

Repository workflow artifacts may be discovered before trust, but executable
hooks, commands, validators, and similar components require explicit project
trust. Trust is stored globally under `~/.kiri/`, scoped to the canonical
project path and the identity of executable definitions. A changed executable
definition requires trust again.

Mandatory Kiri validators and security controls cannot be overridden by project
trust or repository configuration.

### Plan approval and revision

The plan approval UI always exposes:

- `Aprovar plano`;
- `Aprovar e executar no modo Auto`;
- `Continuar planejando`;
- `Negar`;
- `Responder...`.

`Negar` means reject and revise, not terminate planning. The plan is not
applied, execution does not begin, and the LLM asks a contextual question about
what should change. The question is generated from the current plan and
conversation, not from a static message. `Responder...` sends the user's text
as planning feedback and keeps the session in `Plan`.

Every plan has a `plan_id` and revision. Approval applies only to the exact
approved revision. Any plan change invalidates earlier approval and requires a
new approval. A material execution deviation creates a new revision and pauses
before the divergent action. Implementation details within the approved scope
may adapt without a new approval.

An explicit user request can create a recorded plan amendment for a small scope
change. Larger changes require a new plan revision.

### Audit

The policy engine emits structured audit events for relevant actions. Events
include the session, project, normalized command or tool, detected effects,
paths/domains, active mode, decision, deciding rule, and result. Audit logs are
operational data, not memory or documentation, live under `~/.kiri/`, and never
contain raw credentials, raw environment values, or unredacted output.

## Consequences

- Kiri supports arbitrary project ecosystems without maintaining a language
  catalog as its security boundary.
- OS-level sandboxing carries the hard enforcement burden; model classification
  remains a decision aid and routing layer.
- Unknown or ambiguous actions become less convenient because they may require
  a prompt or a denial.
- Mode changes do not interrupt active work, but an active process retains its
  original restrictions until it exits.
- Temporary directories, credential brokering, redaction, audit events, and
  process limits add implementation work that cannot be replaced by prompt
  instructions.

## Alternatives considered

- A per-language or per-package-manager allowlist was rejected because it does
  not scale to arbitrary ecosystems and confuses tool identity with effects.
- Model-only safety classification was rejected because instructions and
  classifiers cannot enforce filesystem or network boundaries.
- Killing every active process on a mode change was rejected because it makes
  normal iterative work unnecessarily disruptive.
- Silently falling back to unrestricted execution when a sandbox is missing was
  rejected because it converts an infrastructure failure into a security gap.
- Allowing repository workflow configuration to weaken the policy engine was
  rejected because project content is not a trusted security authority.

## References

- [Claude Code permissions](https://code.claude.com/docs/en/permissions)
- [Claude Code sandboxing](https://code.claude.com/docs/en/sandboxing)
- [Claude Code permission modes](https://code.claude.com/docs/en/permission-modes)
- [Codex approvals and security](https://learn.chatgpt.com/docs/codex/agent-approvals-security)
- [Codex configuration basics](https://learn.chatgpt.com/docs/config-file/config-basic)
