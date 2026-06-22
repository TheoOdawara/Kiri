<p align="center">
  <img src="docs/marca/Logo.png" alt="Kiri" width="200">
</p>

<h1 align="center">KIRI</h1>

<p align="center">
  <em>Engineering-Grade Code Harness — Forged from Tradition, Built for Precision.</em>
</p>

<p align="center">
  An async Rust coding-agent harness for NVIDIA's OpenAI-compatible API —<br>
  every reasoning step, tool call, and diff visible and under human control.
</p>

---

## The idea

Most "AI coding" tooling optimizes for vibes: type a wish, watch a wall of edits scroll by, hope it
holds. **Kiri is the opposite.** It is a harness for real software engineers — a disciplined loop where
the model reasons out loud, proposes one concrete action at a time, and **you approve every tool call
before it touches the disk.** Production-grade safety, full visibility, human control. The agent does the
work; you keep the wheel.

The name carries the thesis. **Kiri** (桐) is the paulownia — the family *kamon*, and the wood of the
*kiri-dansu* chest that guards a household's most precious goods. It is also a homophone of 切り
("to cut", as in code). The mark is the **Kiri-Gate**: a *tsuba* — a katana's hand-guard — reimagined as
a containment ring, a **Quality Gate** the harness runs around your work. Inside it, three hexagons stand
for the foundations every change rests on: **Code · Infra · Data**. Monochrome, technical, forged — not
decorative.

## Preview

On launch, the harness greets you with the seal; once you start, the transcript streams live above a
borderless prompt whose gate glyph changes color with its state.

```
 ⬢ kiri  Engineering-Grade Code Harness

                            ▄▄▄███████▄▄▄
                         ▄▄████████████████▄▄
                       ▄████▀▀   ▄██▄   ▀▀████▄
                     ▄████▀   ▄███████▄▄   ▀████
                    ▄███▀    ███████████     ▀███
                   ▄███      ███████████      ▀███
                   ███       ███████████       ████
                  ████      ▄▄█▀██████▀▄▄▄      ███
                  ████  ▄▄██████▄▄▀▀▄███████▄   ███
                  ████  ██████████ ███████████  ███
                   ███  ██████████ ███████████ ▄███
                   ▀███▄██████████ ███████████▄███
                    ████▀▀██████▀▀  ▀███████▀████▀
                     ▀███▄ ▀▀▀▀        ▀▀▀  ▄███▀
                       ▀███▄             ▄▄███▀
                         ▀████▄▄▄▄▄▄▄▄▄▄████▀
                            ▀▀██████████▀▀

                          [ KIRI ]
        KIRI harness system: Protecting codebase... [OK]
           Forged from Tradition, Built for Precision

◈─ moonshotai/kimi-k2-instruct · ~/dev/Kiri ──────────────◈
⬡ ›_
  Enter envia · Alt+Enter nova linha · ↑↓ histórico · ^C sai
```

## Features

- **Directed agent loop** — the model plans, then acts through tools, **one approved call at a time**.
- **Streaming reasoning + content** — thoughts and answer stream token-by-token over SSE.
- **9 filesystem tools** behind a **path sandbox** — the workspace root is the single I/O chokepoint.
- **Two frontends, one engine** — a full-screen [ratatui](https://ratatui.rs) **TUI**, or a plain
  line-based REPL (used automatically when stdout isn't a TTY, or forced with `--plain`).
- **Runaway guard** — a 30-minute wall-clock checkpoint pauses long turns to ask whether to keep going.
- **NVIDIA OpenAI-compatible provider** — talks to `integrate.api.nvidia.com`'s `/chat/completions`.
- **Built to hold up** — modular-hexagonal, single binary, `unsafe` forbidden, green-gated.

## Getting started

**Prerequisites**

- Rust **stable** (edition 2024; `rust-toolchain.toml` pins the toolchain with `rustfmt` + `clippy`).
- An NVIDIA API key from [build.nvidia.com](https://build.nvidia.com).

**Configure** — create a `.env` in the project root (it is git-ignored; the key is **never** a CLI flag):

```dotenv
NVIDIA_API_KEY=nvapi-...
NVIDIA_MODEL=moonshotai/kimi-k2-instruct   # any model from NVIDIA's catalog
```

**Build & run**

```bash
cargo build --release
cargo run                      # full-screen TUI on a terminal
cargo run -- "list the crates in this repo and summarize Cargo.toml"
```

## Usage

```text
kiri [OPTIONS] [PROMPT]

Arguments:
  [PROMPT]         Optional first message; the chat then continues interactively

Options:
      --path <DIR> Sandbox root for file tools (also via KIRI_PATH). Defaults to the current directory
      --plain      Use the plain line REPL instead of the TUI (also auto when stdout isn't a TTY)
  -h, --help       Print help
```

**Approval.** Every tool call is confirmed before it runs. Paths **inside** the workspace default to
accept (`[S/n]`); absolute or `~/` paths **outside** it default to decline (`[s/N]`). Press `y`/`s` to
approve, `n` to decline, `Enter` for the default, `Esc`/`Ctrl+C` to abort the session.

**Key bindings (TUI)**

| Key | Action |
|---|---|
| `Enter` | Submit the prompt |
| `Alt+Enter` / `Shift+Enter` | Insert a newline |
| `↑` / `↓` | Recall input history |
| `PgUp` / `PgDn` | Scroll the transcript |
| `Ctrl+C` | Cancel the running turn, or quit when idle |
| `Ctrl+D` | Quit (empty input) |

**Commands.** `/exit`, `/sair`, `/quit` end the session. The plain REPL also accepts `/cd [path]` to show
or change the active workspace.

## Tools

Each tool resolves paths through the sandbox: relative paths stay under the workspace root (`..` and
symlink escapes are rejected), while absolute or `~/` paths reach outside it **only with your approval**.

| Tool | Purpose |
|---|---|
| `read_file` | Read a UTF-8 text file (capped at 64 KB) |
| `write_file` | Create or overwrite a file, making parent directories |
| `edit_file` | Replace the first exact occurrence of a string in a file |
| `delete_file` | Delete a file |
| `move_path` | Move or rename a file or directory |
| `list_dir` | List one level of a directory (directories suffixed with `/`) |
| `create_dir` | Create a directory, including parents |
| `delete_dir` | Delete a directory and its contents |
| `search` | Recursively search file contents for a substring |

## Architecture

Modular hexagonal (ports & adapters, vertical slices) in a single binary. Each module depends inward:
`domain` (pure data and rules) → `application` (use-cases and the **ports** they need, as traits) →
`infrastructure` (the **adapters** that implement them).

```
src/
  main.rs                 # ~8-line entry
  app.rs                  # composition root — wires adapters, picks the frontend
  shared/{kernel,infra}   # cross-cutting primitives; CLI + env + Settings
  modules/
    agent/                # conversation domain + the agent loop + the UI ports
    provider/             # CompletionProvider port + the NVIDIA OpenAI/SSE adapter
    tools/                # Tool trait + ToolRegistry + the sandbox + fs adapters
    repl/                 # the plain line-based frontend
    tui/                  # the full-screen ratatui frontend (Elm-style state machine)
```

Invariants: network I/O lives only in `provider/infrastructure`, filesystem I/O only behind the sandbox,
and `domain` has no I/O at all. The decisions behind this are recorded as ADRs in
[`docs/decisions/`](docs/decisions/) — provider (0001), tools & sandbox (0002), architecture (0003),
the rename & TUI (0004).

## Development

The definition-of-done gate — each must exit 0:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build
cargo test
```

## Identity

The TUI wears the **Tamahagane Void** palette — deep steel-on-void, with sharp gate accents:

| Token | Hex | Use |
|---|---|---|
| Void | `#0D1117` | Background |
| Steel | `#E6EDF3` | Foreground text |
| Brand | `#8B949E` | Rules, delimiters, idle gate |
| Success | `#3FB950` | Passing gate / `[OK]` |
| Warning | `#D29922` | Notices, pending approval |
| Error | `#F85149` | Failures — the blade cut |
| Highlight | `#58A6FF` | Active input, streaming |

The full brand guide, logo, and harness icon live in [`docs/marca/`](docs/marca/).

## License

[MIT](LICENSE) © 2026 Theo Odawara
