# Providers — `PRV`

The model providers and subscriptions a session can run on.

<a id="fr-prv-01"></a>
## FR-PRV-01 — OpenAI-compatible endpoint

> As an engineer, I want to use any OpenAI-compatible endpoint, so that I am not locked to one vendor.

The system shall run turns on any OpenAI-compatible endpoint identified by a base URL and an API key.

| Attribute | Value |
| --- | --- |
| Rationale | Any model the user already has. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-PRV-01.1** — Given the base URL and API key of an OpenAI-compatible endpoint, when the user sends a prompt, then the turn completes through that endpoint.


<a id="fr-prv-02"></a>
## FR-PRV-02 — Anthropic and Anthropic-compatible endpoint

> As an engineer, I want to use Anthropic's API and endpoints compatible with it, so that providers such as MiniMax, GLM, and Kimi work without a dedicated integration.

The system shall run turns on Anthropic's API and on any Anthropic-compatible endpoint identified by a base URL and an API key.

| Attribute | Value |
| --- | --- |
| Rationale | Several providers expose the Anthropic wire format. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-PRV-02.1** — Given an Anthropic API key, when the user sends a prompt, then the turn completes through Anthropic's API.
- **FR-PRV-02.2** — Given the base URL and API key of an Anthropic-compatible endpoint, when the user sends a prompt, then the turn completes through that endpoint.


<a id="fr-prv-03"></a>
## FR-PRV-03 — OpenAI

> As an engineer, I want to use OpenAI's API with my key, so that OpenAI models work with their native interface.

The system shall run turns on OpenAI's API with an API key.

| Attribute | Value |
| --- | --- |
| Rationale | OpenAI is a first-party provider kind in ADR 0001. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-PRV-03.1** — Given an OpenAI API key, when the user sends a prompt, then the turn completes through OpenAI's API.


<a id="fr-prv-04"></a>
## FR-PRV-04 — Local model server

> As an engineer, I want to use a model served on my machine, so that I work with no key and no account.

The system shall run turns on a local model server (Ollama, LM Studio) with no API key.

| Attribute | Value |
| --- | --- |
| Rationale | A local model is one of the providers the user may already have. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-PRV-04.1** — Given only a running Ollama or LM Studio server, when the user selects it and sends a prompt, then the turn completes with no API key configured.


<a id="fr-prv-05"></a>
## FR-PRV-05 — ChatGPT subscription

> As an engineer, I want to sign in with my ChatGPT Plus/Pro account, so that turns draw on the plan I already pay for.

The system shall run turns on a ChatGPT Plus/Pro subscription through the "Sign in with ChatGPT" OAuth flow.

| Attribute | Value |
| --- | --- |
| Rationale | OpenAI supports this flow for third-party tools (verified 2026-09-30). |
| Source | Author interview, 2026-09-30 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-PRV-05.1** — Given a ChatGPT Plus/Pro account, when the user signs in with ChatGPT and sends a prompt, then the turn draws on that plan.


<a id="fr-prv-06"></a>
## FR-PRV-06 — Claude subscription

> As an engineer, I want to use my Claude Pro/Max subscription, so that turns draw on the plan I already pay for.

The system shall run turns on a Claude Pro/Max subscription by driving the user's installed, logged-in official `claude` CLI, with Kiri's tools executing every tool call.

| Attribute | Value |
| --- | --- |
| Rationale | Anthropic blocks third-party OAuth use of the subscription; driving the official CLI is the tolerated path. |
| Source | [ADR 0001](../../decisions/0001-architecture.md) |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-PRV-06.1** — Given a logged-in official `claude` CLI and the opt-in of FR-PRV-07 confirmed, when the user sends a prompt, then the turn runs through the CLI and every tool call is executed by Kiri's tools.
- **FR-PRV-06.2** — Given a `claude` CLI older than the minimum supported version, when the user enables the subscription, then Kiri refuses and names the required version.


<a id="fr-prv-07"></a>
## FR-PRV-07 — Claude subscription opt-in warning

> As an engineer, I want to be warned before enabling my Claude subscription, so that I decide knowing the risk to my account.

The system shall require the user to confirm a warning, stating that Anthropic restricts subscription use outside its own apps and may enforce that against the account, before enabling the Claude subscription.

| Attribute | Value |
| --- | --- |
| Rationale | The account risk is the user's to accept. |
| Source | Author interview, 2026-09-30 |
| Priority | Won't (this release) |
| Status | approved |
| Milestone | Later |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-PRV-07.1** — Given the Claude subscription not yet enabled, when the user enables it, then the warning is shown and the subscription stays disabled until the user confirms.


<a id="fr-prv-08"></a>
## FR-PRV-08 — Live switching

> As an engineer, I want to switch provider, model, and reasoning effort inside a session, so that I change the model without restarting my work.

The system shall let the user switch the provider, the model, and the reasoning effort inside a running session.

| Attribute | Value |
| --- | --- |
| Rationale | The right model changes during a task. Whether history survives a switch into or out of the Claude subscription is [OQ-03](../open-questions.md). |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-PRV-08.1** — Given a session on provider A, when the user switches to provider B, then the next turn uses B with the history preserved.
- **FR-PRV-08.2** — Given a running session, when the user switches the model or the reasoning effort, then the next turn uses the new value.


<a id="fr-prv-09"></a>
## FR-PRV-09 — In-app provider setup

> As an engineer, I want to add a provider from within Kiri, so that I never edit a configuration file by hand.

The system shall let the user add a provider from within Kiri, with no manual file editing.

| Attribute | Value |
| --- | --- |
| Rationale | Onboarding has to work for someone who has never seen the repository. |
| Source | Author interview, 2026-09-30 |
| Priority | Must |
| Status | approved |
| Milestone | M1 |
| Since | v1.0.0 |

**Acceptance criteria**
- **FR-PRV-09.1** — Given no provider configured, when the user adds one from within Kiri, then the next prompt runs on it and no file was edited by hand.
