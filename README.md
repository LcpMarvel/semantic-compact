# semantic-compact

[![CI](https://github.com/LcpMarvel/semantic-compact/actions/workflows/ci.yml/badge.svg)](https://github.com/LcpMarvel/semantic-compact/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/LcpMarvel/semantic-compact)](https://github.com/LcpMarvel/semantic-compact/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A Claude Code plugin that catches the moment a task changes — and hands the
decision back to you before the old context gets dragged along.

Long sessions pile up tool results, debug logs, and exploration history from
work that is already done. Token thresholds only tell you the context is
*full*; they can't tell you it's *stale*. semantic-compact watches every
prompt you submit, asks a typed LLM judge whether you just crossed a task
boundary, and — only at high confidence — returns the prompt to you with a
suggestion.

> Compact when the task changes — not merely when the context is full.

**Why it pays for itself:** every turn re-sends the whole conversation to
the model, and cache reuse only covers what the previous turn started
with. Stale context from a finished task is dead weight you pay for again
on every single turn — and the moment to shed it is *before* the new task
runs, not after you notice the bill. A judgment costs ~$0.00003; a single
uncached re-send of a 120K-token context costs ~$0.60. Missing the switch
once is 20,000× the price of watching for it.

**It never compacts or clears anything by itself, never rewrites your
prompt, and every failure path degrades to silence.**

## What it looks like

Switch to an unrelated task mid-session:

```
❯ done with the login work — now write me an ffmpeg batch transcode script

  Semantic Compact: this looks like a new, unrelated task.
  The previous context is unlikely to be useful for it.
  Consider /clear for a fresh start, or /compact to keep a summary.
  (suggestion only — nothing was cleared)

  The prompt was returned to you, not lost. Run /clear or /compact now,
  then re-send it; re-sending it as-is simply continues with the current
  context.
```

Run `/clear`, re-send (↑ + Enter), done. Or just re-send to continue with
the old context — your call, every time.

## How it works

```
UserPromptSubmit hook
  → parse transcript: recent real prompts, assistant outcomes, files touched
  → build a compact task-state summary (a few KB, not the whole history)
  → one request to a Jev Decisions endpoint (OpenRouter or TypeSafe direct)
     asking four typed yes/no probability questions:
       p(new_task), p(depends_on_previous_context),
       p(previous_task_complete), p(shares_context_with_previous_task)
  → three-way decision:
       same task / depends on history     → silence
       new task, shares project context   → suggest /compact (a summary still helps)
       new task, essentially unrelated    → suggest /clear (even a summary is dead weight)
```

The compact-vs-clear split exists because `/compact` itself costs a
summarization pass and keeps a summary in context: when the new task shares
the project that summary still earns its tokens; when it doesn't, `/clear`
is the cheaper, cleaner reset.

Judgment behavior:

- **Calibrated on real sessions.** Thresholds ship at
  `p_new ≥ 0.85 AND p_dep ≤ 0.30`; measured clusters are follow-ups
  0.04–0.38, coding tangents ~0.80, genuine switches 0.89–0.98.
- **Continuations stay silent.** Cross-layer shifts that serve one goal
  (OAuth backend → its login button), "add tests for it"-style follow-ups,
  questions about earlier work, and brief tangents are the same task.
- **Cooldown only guards the re-send.** After a suggestion, only the
  immediate re-sent prompt (and switches within ~15s) are swallowed; the
  judge's calibrated threshold is the real precision gate. Clearing the
  conversation rearms it instantly.
- **Cheap.** ~1 s and ~$0.00003 per judged prompt; slash commands and the
  very first prompt of a session skip the judge entirely.

## Install

```sh
claude plugin marketplace add LcpMarvel/semantic-compact
claude plugin install semantic-compact@semantic-compact
```

No prerequisites: on the first prompt after install the plugin provisions
itself with a checksum-verified static binary for your platform (macOS
arm64/x64, Linux amd64/arm64), and `claude plugin update semantic-compact`
re-provisions on upgrades. Restart Claude Code after installing.

Uninstall: `claude plugin uninstall semantic-compact` and optionally
`claude plugin marketplace remove semantic-compact`.

## Setup

Judgments are served over the Jev Decisions API — pick a provider; only an
API key is required:

| provider | endpoint | model | key from |
|---|---|---|---|
| `openrouter` (default) | `openrouter.ai/api/alpha/decisions` | `~typesafe/jev-latest` | [openrouter.ai/keys](https://openrouter.ai/keys) |
| `typesafe` | `api.typesafe.ai/v1/systemone` | `jev-latest` | [console.typesafe.ai/keys](https://console.typesafe.ai/keys) |
| `custom` | you set `jev.url` / `jev.model` | yours | yours |

Pick one of:

1. **`/sc-setup`** (most reliable) — the wizard shipped with the plugin:
   provider choice, then only the key for presets; field-by-field walk
   (or a written template) for fully custom setups; live verification at
   the end. The wizard never takes your key in conversation — it hands you
   a one-liner to run in your own terminal, so the key never enters the
   session transcript or any request context.
2. **Install-time dialog** — installing via `/plugin` inside a session
   opens the plugin's configuration dialog (`JEV_PROVIDER`, `JEV_API_KEY`:
   masked input, stored in the OS keychain).
3. **`/plugin configure semantic-compact`** — the same dialog any time.
   CLI form:
   `claude plugin install semantic-compact@semantic-compact --config JEV_PROVIDER=typesafe --config JEV_API_KEY=...`
4. **Files / environment** — see below.

> Version note: on some Claude Code builds (verified on 2.1.283) dialog /
> `--config` values are stored but the `CLAUDE_PLUGIN_OPTION_*` variables
> are not exported to hook processes, so the plugin never sees them. If the
> one-time setup notice keeps appearing after configuring through the
> dialog, use `/sc-setup` or the file/env path — they work everywhere.

## Configuration

Nothing is required beyond the key; everything else has defaults. Layers,
lowest to highest:

1. built-in defaults (compiled in)
2. `<plugin root>/config.json` — maintainer-tuned defaults
3. `~/.config/semantic-compact/config.json` — your overrides
   (`$XDG_CONFIG_HOME/semantic-compact/config.json` if set)
4. environment variables

API key resolution, high to low:

1. `CLAUDE_PLUGIN_OPTION_JEV_API_KEY` — the dialog / `--config` value
2. `JEV_API_KEY`, then the selected provider's key in the environment
   (`OPENROUTER_API_KEY` or `TYPESAFE_API_KEY`; custom providers use only the generic key)
3. `"jev": { "api_key": "..." }` in a config JSON layer
4. `~/.config/semantic-compact/env` (KEY=VALUE lines)
5. `<project>/.env` next to where Claude Code runs
6. `<plugin root>/.env`

Without a key the plugin stays inert, and the first prompt of a session
carries a one-time notice explaining how to configure it.

Full option reference:

```jsonc
{
  "thresholds": { "new_task_min": 0.85, "depends_max": 0.3, "shared_context_min": 0.5 },
  "jev": {
    "provider": "openrouter",          // openrouter | typesafe | custom
    "url": "(filled from the provider preset)",
    "model": "(filled from the provider preset)",
    "api_key": null,
    "auth_header": "Authorization",    // e.g. x-api-key for other gateways
    "auth_scheme": "Bearer",           // empty string = send the raw key
    "timeout_ms": 10000
  },
  "cooldown": { "prompts": 1, "seconds": 15 },
  "history": {
    "max_user_prompts": 8,
    "max_assistant_outcomes": 3,
    "max_files": 15,
    "user_prompt_chars": 1500,
    "assistant_outcome_chars": 500,
    "new_prompt_chars": 3000,
    "state_char_limit": 10000
  },
  "skip": { "min_prior_prompts": 1 },
  "reminder": { "mode": "block", "message": "...", "clear_message": "..." },
  "logging": { "decisions": false, "debug": false }
}
```

Environment overrides (highest priority): `SC_NEW_TASK_MIN`,
`SC_DEPENDS_MAX`, `SC_SHARED_CONTEXT_MIN`, `SC_REMINDER_MODE`,
`SC_COOLDOWN_PROMPTS`, `SC_COOLDOWN_SECONDS`, `SC_MAX_PROMPTS`,
`SC_MIN_PRIOR_PROMPTS`, `SC_LOG_DECISIONS`, `SC_DEBUG`,
`JEV_DECISIONS_URL`, `JEV_MODEL`, `JEV_AUTH_HEADER`, `JEV_AUTH_SCHEME`,
`JEV_TIMEOUT_MS`. The provider also reads `CLAUDE_PLUGIN_OPTION_JEV_PROVIDER`
(set by the plugin dialog); explicit `JEV_DECISIONS_URL` / `JEV_MODEL`
always win over presets.

Notable semantics:

- `reminder.mode` — `"block"` (default) or `"remind"`. A warning that
  appears while the model is already running can't be acted on, so a
  suggestion returns the prompt to you **before** it is processed: the
  reason is shown, the prompt is not lost — run `/clear` or `/compact`,
  then re-send it; re-sending as-is simply continues with the current
  context. Nothing is ever cleared automatically, and failure paths never
  block. `"remind"` gives the non-intrusive warning-only behavior.
- `cooldown` — after one suggestion the session is quiet until **both** N
  prompts have passed **and** M seconds elapsed. Clearing the conversation
  (history suddenly below the floor) rearms it immediately.
- `skip.min_prior_prompts` — the first prompt of a session has nothing to
  compare against; the second already judges fine (one prior exchange is
  enough to spot a full topic switch).
- `jev.auth_header` / `auth_scheme` — non-OpenRouter gateways work without
  code changes.

### Runtime cost

One judge call per judged prompt (~0.7–1.5 s latency, ~$0.00003 each with
the default model).

## Privacy

- **What leaves the machine:** the task-state summary only — recent prompt
  texts, short assistant outcome summaries, touched file paths, and the new
  prompt. Never the full transcript, tool outputs, or file contents.
- **The API key** is read at runtime from env/config/keychain, never
  logged, never committed.
- **Local logging is opt-in.** A fresh install writes nothing but the
  cooldown state. `"logging": { "decisions": true }` (or
  `SC_LOG_DECISIONS=1`) appends one JSONL line per judgment — timestamp,
  session id, prompt hash + 120-char preview, the four probabilities,
  decision, thresholds, latency, cost, error. `logging.debug`
  additionally records the raw task state and judge responses. Nothing is
  ever uploaded.
- The data dir is `SC_DATA_DIR` → `CLAUDE_PLUGIN_DATA` (installed plugins)
  → `<plugin root>/data` (local dev). It is gitignored.

### Reviewing your own data

The log doubles as a calibration dataset. Edit `human_label` by hand
(`same_task` | `new_task` | `depends_on_context` | `uncertain`) and look
at precision first:

```sh
jq -s '[.[] | select(.decision=="SUGGEST_CLEAR")] | length' \
  ~/.claude/plugins/data/semantic-compact-*/logs/decisions-*.jsonl
```

False positives (blocked, you just re-sent) and false negatives (silent,
but you cleared anyway) are exactly the samples thresholds should be tuned
on — ideally with a PR.

## Development

```sh
cargo build --release && cp target/release/semantic-compact bin/  # dev binary for --plugin-dir
cargo test                    # 33 unit + integration tests, mocked judge
cargo clippy && cargo fmt
sh scripts/setup.sh           # same provisioning the hook entry point does
claude plugin validate .
```

Live smoke against the real Decisions API (uses `.env` in the repo root):

```sh
echo '{"session_id":"smoke","transcript_path":"'"$PWD"'/testdata/fixtures/oauth-session.jsonl",
       "cwd":"'"$PWD"'","hook_event_name":"UserPromptSubmit",
       "prompt":"Enough of the login work — now redesign the pricing page"}' \
  | ./bin/semantic-compact
# → {"decision":"block","reason":"Semantic Compact: this looks like a new task. ..."}
```

The fixtures under `testdata/fixtures/` double as regression snapshots for
the (undocumented, internal) transcript format — verified against Claude
Code 2.1.283; if a future version changes the layout, `cargo test` is the
canary.

Releases are cut from the Actions tab (`release` workflow): it bumps
`Cargo.toml` + `plugin.json`, tags, builds the four platform binaries with
`SHA256SUMS`, and publishes the GitHub release the setup script installs
from. Installed users update by version — any change to plugin content
(commands, hooks, manifest, binary) must ship as a release with a bumped
version; an unreleased push to `main` updates the marketplace clone but
never the installed cache.

### Known constraints

- Claude Code's transcript JSONL is internal and may change between
  versions; the parser is deliberately defensive and any parse failure
  fails open to silence.
- `UserPromptSubmit` runs synchronously, so judge latency (~1 s) is added
  to each judged prompt; the internal timeout is 10 s, on timeout the
  prompt proceeds normally.

## Contributing

PRs welcome — especially threshold recalibrations backed by decision-log
evidence (mask or summarize your prompts; never paste real API keys). Bug
reports with a redacted log line (`skip_reason` / probabilities) are far
more actionable than "it didn't fire".

## License

[MIT](LICENSE)
