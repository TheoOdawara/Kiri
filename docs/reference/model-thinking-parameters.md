# Model thinking parameters

How major model families expose reasoning / extended thinking, what Kiri sends by default, and how to
override the generalist path in `~/.kiri/config.toml`.

Last reviewed: 2026-07-14. Wire shapes change often — treat unverified cells as “send nothing.”

## Kiri dial

Two controls feed every adapter:

| Control | Where | Meaning |
|---|---|---|
| Global / profile `thinking` | `Settings.thinking`, optional `providers.<id>.thinking` | Master on/off (profile overrides kind default) |
| `/effort` → `Effort` | Global config `effort` | `off` · `low` · `medium` · `high` · `xhigh` · `max` |

Adapters only emit reasoning params when **both** allow it: thinking is on **and** effort ≠ `off`.

Shared enum labels are provider-agnostic; each adapter maps them to a native shape (or drops the
request field when the host has no confirmed mapping).

## Native kinds (official shapes)

### GPT (OpenAI) — `ProviderKind::Openai`

| Item | Value |
|---|---|
| Request | Chat Completions: `reasoning_effort` · Responses API: `reasoning.effort` (Kiri uses chat completions) |
| Levels | Model-dependent: `none` / `minimal` / `low` / `medium` / `high` / `xhigh` |
| Kiri map | `off` → omit · `low`/`medium` → same · `high`/`xhigh`/`max` → `"high"` (safe collapse) |
| Disable | Omit `reasoning_effort` (or send `none`/`minimal` on models that accept them) |
| Response | Reasoning is usually not streamed as visible CoT on chat completions |

### Claude (Anthropic) — `ProviderKind::Anthropic`

Two wire eras; Kiri classifies by model id:

| Mode | When | Request |
|---|---|---|
| **Budget** | Haiku 4.5 and older Claude 4-style ids (fallback) | `thinking: { type: "enabled", budget_tokens, display: "summarized" }` |
| **Adaptive opt-in** | Opus 4.7 / 4.8 | `thinking: { type: "adaptive" }` + `output_config: { effort }` · omit both to disable |
| **Adaptive default-on** | Sonnet 5 | Same adaptive shape; disable needs explicit `thinking: { type: "disabled" }` |

| Effort → budget (Budget mode) | Tokens |
|---|---|
| low | 1_024 |
| medium | 4_096 |
| high | 8_192 |
| xhigh | 12_000 |
| max | 14_000 |

Adaptive effort strings use the same labels as Kiri (`low`…`max`). Assistant turns must lead with a
thinking block when the API returned one (visible or redacted).

### DeepSeek (native API)

| Item | Value |
|---|---|
| Toggle | `thinking: { "type": "enabled" \| "disabled" }` (default often **enabled**) |
| Effort | `reasoning_effort`: `"high"` \| `"max"` only · low/medium → high · xhigh → max |
| Response | `reasoning_content` alongside `content` |
| Tools | After a tool-using turn, **must** echo `reasoning_content` on subsequent requests (else 400) |
| Kiri | No first-class kind. Via `openai-compatible` **auto does not invent DeepSeek kwargs** (NIM hang / vLLM ignore history). Use a dedicated proxy that already injects the right body, or set `thinking_style` only when you know the host accepts the official shape (future styles may grow; v1 keeps auto safe). |

### Qwen (Alibaba / open weights)

| Host | Shape |
|---|---|
| DashScope native | Top-level `enable_thinking` (+ optional `thinking_budget`) via `extra_body` |
| vLLM / SGLang / llama.cpp / NVIDIA NIM | `chat_template_kwargs: { "enable_thinking": true\|false }` |

Many Qwen3 sizes default thinking **on**; small Qwen3.5 sizes often default **off**. Prefer an
explicit `false` when disabling.

### Kimi (Moonshot)

| Host | Shape |
|---|---|
| NVIDIA NIM | `chat_template_kwargs: { "thinking": true\|false }` |
| Moonshot official | Confirm against current Moonshot docs before hardcoding a non-NIM path |

### GLM (Zhipu / Z.AI)

| Host | Shape |
|---|---|
| Z.AI native | `thinking: { "type": "enabled" \| "disabled" }` |
| vLLM / SGLang / NVIDIA NIM | `chat_template_kwargs: { "enable_thinking": true\|false }` |

Same family, **two shapes** — host decides. Kiri’s generalist/NIM path uses chat-template kwargs.

### Gemma 4 (Google)

| Item | Value |
|---|---|
| Request | `chat_template_kwargs: { "enable_thinking": true\|false }` (Vertex open models, vLLM/SGLang, llama.cpp, LM Studio, NVIDIA when cataloged) |
| Default | **off** on most IT hosts |
| Levels | toggle only (no effort dial) |
| Disable | `enable_thinking: false` or omit (default off) |
| Response | CoT often in `reasoning_content` when the host parses thinking tokens |
| Kiri | Model id must match **Gemma 4** (`gemma-4` / `gemma4` / `gemma_4`). Gemma 3 and bare `gemma` stay unsupported (no confirmed kwargs). |

Transformers uses `enable_thinking=True` on `apply_chat_template`; Ollama may expose a separate `think`
flag or a `*:thinking` variant — not the generalist default.

### Grok (xAI)

| Item | Value |
|---|---|
| Chat Completions | `reasoning_effort`: `low` / `medium` / `high` (default often `high`) |
| Responses API | `reasoning: { "effort": "…" }` |
| Disable | Often **not** allowed on flagship reasoning models |
| Kiri | No first-class kind → `openai-compatible` + base `https://api.x.ai/v1` · auto sends `reasoning_effort` |

### Composer (Cursor)

Proprietary Cursor models (Composer 1 / 1.5 / 2 / 2.5). **No public inference API** — not a Kiri
provider surface. Listed only so agents do not invent a wire shape.

---

## Host notes (non-native)

| Host | Typical thinking control | Auth in Kiri |
|---|---|---|
| **NVIDIA NIM** | Family-keyed `chat_template_kwargs` (see native NVIDIA table in code) | `kind = "nvidia"` |
| **OpenRouter** | Unified `reasoning: { effort, max_tokens, enabled }` and/or OpenAI `reasoning_effort`; maps downstream per model | `openai-compatible` + API key |
| **Ollama** | Native `think: true\|false\|"low"\|…` on Ollama APIs; OpenAI-compat layer may or may not forward foreign fields | keyless `openai-compatible` |
| **LM Studio** | OpenAI-compatible; often honors `chat_template_kwargs` or server UI toggles | keyless `openai-compatible` |
| **llama.cpp server** | `--chat-template-kwargs '{"enable_thinking":…}'` and/or request `chat_template_kwargs` | keyless `custom` / `openai-compatible` |

Kiri does **not** special-case Ollama/`think` or OpenRouter’s `reasoning` object in v1 defaults —
those are not the universal OpenAI-compatible convention. The generalist default below covers most
setups; power users override with `thinking_style` or server-side flags.

---

## What Kiri sends by default

### First-party kinds

| Kind | When thinking on + effort ≠ off | When off |
|---|---|---|
| `openai` | `reasoning_effort` | omit |
| `anthropic` | Adaptive or Budget shape by model id | omit, or explicit `disabled` for Sonnet 5 |
| `nvidia` | Nemotron/Kimi → `chat_template_kwargs.thinking` · Qwen/GLM/Gemma4 → `enable_thinking` · DeepSeek/other → **nothing** | known template families send explicit `false` |

### `openai-compatible` / `custom` (generalist **auto**)

| Model id looks like… | Request field |
|---|---|
| `qwen` / `glm` / `kimi` / `nemotron` / `gemma-4`/`gemma4` | `chat_template_kwargs` (same keys as NVIDIA) |
| `deepseek` | **nothing** (unsafe to guess) |
| `gpt` / `o1`/`o3`/`o4` / `grok` / other | `reasoning_effort` when enabled |

This is the market default: template kwargs for open-weights hybrid models, OpenAI effort for
cloud-style endpoints (OpenRouter GPT/Grok, etc.).

---

## Override in `~/.kiri/config.toml`

```toml
[providers.my-local]
kind = "openai-compatible"
base_url = "http://127.0.0.1:1234/v1"
model = "qwen3-32b"
auth = "none"
# optional — default is "auto"
thinking_style = "auto"   # auto | reasoning_effort | chat_template | off
# optional — None uses kind default
# thinking = true
```

| `thinking_style` | Behavior |
|---|---|
| `auto` | Heuristic table above |
| `reasoning_effort` | Always OpenAI-style effort (never template kwargs) |
| `chat_template` | Always family-keyed `chat_template_kwargs` (nothing if family unknown) |
| `off` | Never send thinking-related fields |

Native kinds (`openai`, `anthropic`, `nvidia`) ignore `thinking_style` and always use their official
shape.

---

## Effort map summary

| Kiri `Effort` | OpenAI / generalist `reasoning_effort` | Anthropic budget | Anthropic adaptive | Template families |
|---|---|---|---|---|
| off | omit | omit | omit / explicit disabled | kwargs `false` when family known |
| low | `low` | 1024 | `low` | kwargs `true` |
| medium | `medium` | 4096 | `medium` | kwargs `true` |
| high | `high` | 8192 | `high` | kwargs `true` |
| xhigh | `high` (collapsed) | 12000 | `xhigh` | kwargs `true` |
| max | `high` (collapsed) | 14000 | `max` | kwargs `true` |

DeepSeek official maps low/medium → `high`, xhigh → `max` (not applied by Kiri auto — see above).

## Invariants

1. **Never invent** a shape for an unconfirmed (family × host) pair — wrong kwargs hang or 400.
2. **Defaults-on models** need an explicit disable when the user turns thinking off.
3. Compatible endpoints stay **user-owned**: free base URL / model / key in TOML; Kiri only ships a
   generalist default + optional `thinking_style`.
