# semantic-compact

A Claude Code plugin that suggests running `/compact` when a new task begins.

Long coding sessions accumulate tool results, debug logs, and exploration
history from tasks that are already done. This plugin watches every prompt
you submit, asks a typed LLM judge whether you just crossed a task boundary,
and — only at high confidence — shows a one-line reminder to consider
`/compact`.

> Compact when the task changes — not merely when the context is full.

It **never** compacts by itself, never blocks or rewrites your prompt, and
stays completely silent (and harmless) on every failure path.

## How it works

```
UserPromptSubmit hook
  → parse transcript: recent real user prompts, assistant outcomes, files touched
  → build a compact task-state summary (~a few KB)
  → one request to the OpenRouter Decisions API (model ~typesafe/jev-latest)
     asking four calibrated yes/no probability questions:
       p(new_task), p(depends_on_previous_context),
       p(previous_task_complete), p(shares_context_with_previous_task)
  → three-way decision:
       same task / depends on history      → silence
       new task, shares project context    → suggest /compact (a summary still helps)
       new task, essentially unrelated     → suggest /clear (even a summary is dead weight)
  → suggestion renders as a systemMessage warning; otherwise: silence
```

The compact-vs-clear split exists because `/compact` itself costs a
summarization pass and keeps a summary in context. When the new task shares
the project (`p_shared >= shared_context_min`), that summary still earns its
tokens; when it does not, `/clear` is the cheaper, cleaner reset.

Design principles, in order: **precision over recall** (a missed switch
costs nothing, a false nag costs trust), **reminder only** (the decision to
compact is always yours), **fail open** (missing key, timeout, HTTP error,
unreadable transcript, malformed response — every path degrades to silence
and the prompt proceeds normally).

Follow-ups like "给它补一下测试" / "fix that", cross-layer shifts that serve
the same goal (backend → its frontend button), questions about earlier work,
and brief tangents are all treated as the same task. "登录先到这里，现在做
pricing 页面" is a boundary.

## Install

Requirements for users: none beyond Claude Code — the Setup hook downloads a
prebuilt static binary for your platform (macOS arm64/x64, Linux amd64/arm64)
and verifies its checksum.

- From this repository (local, session-scoped):
  `claude --plugin-dir /path/to/semantic-compact`
- Daily driver without flags: install via your marketplace / `claude plugin install`.

Inside an interactive session, `/reload-plugins` picks up code changes.

## Configuration

No configuration is required besides an API key. Everything else has
defaults. Layers, lowest to highest:

1. built-in defaults (compiled in)
2. `<plugin root>/config.json` — maintainer-tuned defaults
3. `~/.config/semantic-compact/config.json` — your overrides
   (`$XDG_CONFIG_HOME/semantic-compact/config.json` if set)
4. environment variables — highest priority

### API key

The judge needs an OpenRouter API key (or a compatible Decisions endpoint —
see `jev.url`). Resolution order:

1. `JEV_API_KEY` (or `OPENROUTER_API_KEY`) in the environment
2. `"jev": { "api_key": "..." }` in a config JSON layer
3. `~/.config/semantic-compact/env` (KEY=VALUE lines)
4. `<project>/.env` next to where Claude Code runs
5. `<plugin root>/.env`

Quick start:

```sh
mkdir -p ~/.config/semantic-compact
printf 'JEV_API_KEY=sk-or-...' >> ~/.config/semantic-compact/env
```

Without a key the plugin is inert — every prompt is skipped silently.

### Full option reference

```jsonc
{
  "thresholds": { "new_task_min": 0.9, "depends_max": 0.2, "shared_context_min": 0.5 },
  "jev": {
    "url": "https://openrouter.ai/api/alpha/decisions",
    "model": "~typesafe/jev-latest",
    "api_key": null,
    "auth_header": "Authorization",
    "auth_scheme": "Bearer",
    "timeout_ms": 10000
  },
  "cooldown": { "prompts": 8, "seconds": 600 },
  "history": {
    "max_user_prompts": 8,
    "max_assistant_outcomes": 3,
    "max_files": 15,
    "user_prompt_chars": 1500,
    "assistant_outcome_chars": 500,
    "new_prompt_chars": 3000,
    "state_char_limit": 10000
  },
  "skip": { "min_prior_prompts": 2 },
  "reminder": { "message": "...", "clear_message": "..." },
  "debug_logging": false
}
```

Environment overrides (highest priority): `SC_NEW_TASK_MIN`,
`SC_DEPENDS_MAX`, `SC_SHARED_CONTEXT_MIN`, `JEV_DECISIONS_URL`, `JEV_MODEL`,
`JEV_AUTH_HEADER`, `JEV_AUTH_SCHEME`, `JEV_TIMEOUT_MS`,
`SC_COOLDOWN_PROMPTS`, `SC_COOLDOWN_SECONDS`, `SC_MAX_PROMPTS`,
`SC_MIN_PRIOR_PROMPTS`, `SC_DEBUG`.

Notes:

- `auth_header`/`auth_scheme` exist so non-OpenRouter Decisions deployments
  (e.g. `x-api-key` with no scheme) work without code changes.
- `cooldown`: after one suggestion, further suggestions for that session are
  suppressed until **both** N prompts have passed **and** M seconds have
  elapsed — one reminder per task switch, not a nag.
- `skip.min_prior_prompts`: sessions younger than this many real prompts are
  not judged (nothing worth compacting yet, saves latency and cost).
- `debug_logging` writes extra detail (raw task state, judge response) to
  `data/logs/debug-*.jsonl`. It records prompt text; keep it off by default.

### Runtime cost

One judge call per prompt (~0.7–1.5 s latency, ~$0.00003 each with the
default model). Sessions below `min_prior_prompts` and slash commands are
skipped without a call.

## Data & privacy

- What leaves the machine: the task-state summary only — recent user prompt
  texts, short assistant outcome summaries, touched file paths, and the new
  prompt. Never the full transcript, tool outputs, or file contents.
- The API key is read at runtime, never logged, never committed.
- Every judgment is appended to a local JSONL log:
  `<data dir>/logs/decisions-YYYY-MM-DD.jsonl` — one line per judgment with
  timestamp, session id, prompt hash + 120-char preview, the three
  probabilities (all four), decision, thresholds, latency, cost, and error (if any).
  `human_label` and `outcome` stay null until you fill them in by hand.
- The data dir is `SC_DATA_DIR` → `CLAUDE_PLUGIN_DATA` (installed plugins)
  → `<plugin root>/data` (local dev). It is gitignored; nothing is uploaded.

### Reviewing your own data

The log doubles as a calibration dataset. To audit the plugin, edit the
`human_label` field by hand (`same_task` | `new_task` | `depends_on_context`
| `uncertain`), then look at precision first:

```sh
jq -s '[.[] | select(.decision=="SUGGEST_COMPACT")] | length' data/logs/decisions-*.jsonl
jq -s '[.[] | select(.human_label=="new_task")] | length' data/logs/decisions-*.jsonl
```

False positives (suggested, you kept working) and false negatives (silent,
but you compacted anyway) are exactly the samples worth tuning thresholds
and judge wording on.

## Development

```sh
cargo build --release && cp target/release/semantic-compact bin/   # dev binary for --plugin-dir
cargo test                    # unit + integration tests, mocked judge
cargo clippy && cargo fmt
sh scripts/setup.sh           # same provisioning the Setup hook does
claude plugin validate .      # plugin manifest check
```

Live smoke against the real Decisions API (uses `.env` in the repo root):

```sh
echo '{"session_id":"smoke","transcript_path":"'"$PWD"'/testdata/fixtures/oauth-session.jsonl",
       "cwd":"'"$PWD"'","hook_event_name":"UserPromptSubmit",
       "prompt":"登录先到这里。现在帮我重新设计 pricing 页面"}' \
  | ./bin/semantic-compact
# → {"systemMessage":"Semantic Compact: this looks like a new task. ..."}
```

End-to-end in Claude Code (here via the zclaude wrapper):

```sh
zclaude --plugin-dir /path/to/semantic-compact -p "..."   # watch for the reminder
```

Releases are cut from the Actions tab (`release` workflow): it bumps
`Cargo.toml` + `plugin.json` to the given version, tags, builds the four
platform binaries, and attaches them plus `SHA256SUMS` to the GitHub
release. The Setup hook installs from there.

## Known constraints

- Claude Code's transcript JSONL is an internal, undocumented format; the
  parser is deliberately defensive (unknown lines skipped, any parse failure
  → silent skip). It is verified against Claude Code 2.1.283; if a future
  version changes the layout, `cargo test` against the fixtures is the
  canary.
- `UserPromptSubmit` runs synchronously before the model call, so judge
  latency (~1 s) is added to every judged prompt. The internal timeout is
  10 s; on timeout the prompt proceeds normally.
