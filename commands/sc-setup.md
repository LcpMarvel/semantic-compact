---
description: Set up semantic-compact — choose provider (TypeSafe direct / OpenRouter / custom), save the API key and config, then verify end-to-end
---

Run the semantic-compact setup wizard. Conduct the conversation in the user's
language (default to Chinese if unclear). NEVER echo, quote, or log the API
key after the user provides it — pass it straight into the file write.

## Step 1 — provider choice

Present exactly three options and wait for the user to pick one:

1. **TypeSafe 官方直连** — Jev 官方 API,key 在 https://console.typesafe.ai/keys 创建
2. **OpenRouter** — key 在 https://openrouter.ai/keys 创建
3. **完全自定义** — 自己填 endpoint / model / 鉴权方式

## Step 2 — preset providers (options 1 and 2)

Ask ONLY for the API key, then write two files (create directories as needed).
Use the Write tool for both — the permission prompt is expected.

`~/.config/semantic-compact/env` — a single line, nothing else:

- option 1: `TYPESAFE_API_KEY=<key>`
- option 2: `OPENROUTER_API_KEY=<key>`

`~/.config/semantic-compact/config.json` — the preset (auth defaults already match):

- option 1: `{"jev": {"url": "https://api.typesafe.ai/v1/systemone", "model": "jev-latest"}}`
- option 2: `{"jev": {"url": "https://openrouter.ai/api/alpha/decisions", "model": "~typesafe/jev-latest"}}`

## Step 3 — fully custom (option 3)

Offer two sub-modes and let the user choose:

**(a) 逐项引导** — walk through the fields ONE at a time, showing the default
and accepting a blank answer to keep it:

1. `url` (jev.url) — required, no default. Example: `https://your-gateway.example/v1/decisions`
2. `model` (jev.model) — required, no default
3. API key — optional; blank means "configure later"
4. `auth_header` (jev.auth_header) — default `Authorization`; e.g. `x-api-key`
5. `auth_scheme` (jev.auth_scheme) — default `Bearer`; empty string means send the raw key
6. `timeout_ms` (jev.timeout_ms) — default `10000`

Then write `~/.config/semantic-compact/env` (only if a key was given, as
`JEV_API_KEY=<key>`) and `~/.config/semantic-compact/config.json` as
`{"jev": {"url": ..., "model": ..., "auth_header": ..., "auth_scheme": ..., "timeout_ms": ...}}`
(omit fields kept at defaults).

**(b) 手写模板** — write `~/.config/semantic-compact/config.json` with every
field above as an empty string / the numeric defaults, tell the user to fill
in `url` and `model` by hand, and stop after Step 4 is skipped.

## Step 4 — verify (only when a key was saved)

Run, in order:

```sh
sh "$CLAUDE_PLUGIN_ROOT/scripts/setup.sh"
```

```sh
echo '{"session_id":"sc-setup-verify","transcript_path":"'"$CLAUDE_PLUGIN_ROOT"'/testdata/fixtures/oauth-session.jsonl","cwd":"'"$CLAUDE_PLUGIN_ROOT"'/hooks","hook_event_name":"UserPromptSubmit","prompt":"登录先到这里。现在帮我重新设计 pricing 页面"}' | "$CLAUDE_PLUGIN_ROOT/bin/semantic-compact"
```

Interpretation:
- A `{"systemMessage": ...}` line mentioning compact/clear → fully working.
- Empty output with exit 0 → the hook ran but stayed silent; check
  `"$CLAUDE_PLUGIN_ROOT"/data/logs/` (or `~/.claude/plugins/data/`) for the
  decision line and report `skip_reason`.
- Non-zero exit or a setup error → report the message and the last log line.

## Step 5 — summary

Tell the user: which files were written where, that thresholds can be tuned
later in `~/.config/semantic-compact/config.json` (see the plugin README),
and that the plugin is active in new sessions (or after `/reload-plugins`).
