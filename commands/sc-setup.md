---
description: Set up semantic-compact — choose provider (TypeSafe direct / OpenRouter / custom) and configure everything except the API key, which the user adds privately
---

Run the semantic-compact setup wizard. Conduct the conversation in the
user's language (default to English if unclear).

**Security rule — absolute:** NEVER ask the user to paste, type, or
otherwise reveal an API key in this conversation. Anything sent in chat
ends up in the session transcript and in requests to the model provider.
The key is always added by the user themselves, in their own terminal, via
the commands below.

## Step 1 — provider choice

Present exactly three options and wait for the user to pick one:

1. **TypeSafe direct** — the official Jev API; create a key at
   https://console.typesafe.ai/keys
2. **OpenRouter** — create a key at https://openrouter.ai/keys
3. **Fully custom** — your own endpoint / model / auth

## Step 2 — preset providers (options 1 and 2)

Write `~/.config/semantic-compact/config.json` (create directories as
needed, use the Write tool; the permission prompt is expected):

- option 1: `{"jev": {"provider": "typesafe"}}`
- option 2: `{"jev": {"provider": "openrouter"}}`

(url/model/auth come from the built-in preset; nothing else to configure.)

Then tell the user — do not run it yourself, the key must not pass through
this conversation:

> Now run this in your own terminal (not this session) to store the key:
>
> ```sh
> mkdir -p ~/.config/semantic-compact
> printf 'JEV_API_KEY=<your-key>\n' > ~/.config/semantic-compact/env
> ```
>
> Create an OpenRouter key at https://openrouter.ai/keys (TypeSafe keys at
> https://console.typesafe.ai/keys). Tell me once it is in place and I will
> verify the setup.

## Step 3 — fully custom (option 3)

Walk through the non-secret fields ONE at a time in chat, showing the
default and accepting a blank answer to keep it:

1. `url` (jev.url) — required, no default. Example: `https://your-gateway.example/v1/decisions`
2. `model` (jev.model) — required, no default
3. `auth_header` (jev.auth_header) — default `Authorization`; e.g. `x-api-key`
4. `auth_scheme` (jev.auth_scheme) — default `Bearer`; empty string means send the raw key
5. `timeout_ms` (jev.timeout_ms) — default `10000`

Then write `~/.config/semantic-compact/config.json` as
`{"jev": {"provider": "custom", "url": ..., "model": ..., "auth_header": ..., "auth_scheme": ..., "timeout_ms": ...}}`
(omit fields kept at defaults).

For the key, give the same self-service instruction as Step 2 (the env
file line `JEV_API_KEY=<your-key>`; alternatively they may set `"api_key"`
in the config.json by hand).

## Step 4 — verify (optional, only when the user says the key is in place)

Run, in order:

```sh
sh "$CLAUDE_PLUGIN_ROOT/scripts/setup.sh"
```

```sh
echo '{"session_id":"sc-setup-verify","transcript_path":"'"$CLAUDE_PLUGIN_ROOT"'/testdata/fixtures/oauth-session.jsonl","cwd":"'"$CLAUDE_PLUGIN_ROOT"'/hooks","hook_event_name":"UserPromptSubmit","prompt":"Enough of the login work — now redesign the pricing page"}' | "$CLAUDE_PLUGIN_ROOT/bin/semantic-compact"
```

Interpretation:
- A `{"decision": "block", ...}` line suggesting compact/clear → fully
  working (the prompt is intentionally returned; re-send or clear and
  re-send as you like).
- Empty output with exit 0 → the hook ran but stayed silent; check the
  latest decision log line (under `~/.claude/plugins/data/` or the
  plugin's `data/logs/`) and report its `skip_reason`.
- Non-zero exit or a setup error → report the message and the last log
  line.

Never print the key or the env file's contents at any point. To confirm the
file exists, check its path only.

## Step 4.5 — optional local judgment log

Ask whether to enable the local decision log (default off). Present it
exactly like this, without pushing either way:

> Enable the local judgment log? It is off by default. When enabled, each
> judgment appends one JSONL line locally (including a 120-char preview of
> your prompt), useful for reviewing suggestion quality and calibrating
> thresholds; when disabled, nothing but the cooldown state is written.

If yes, write `~/.config/semantic-compact/config.json` with
`{"logging": {"decisions": true}}` merged into whatever the earlier steps
wrote. If no, do nothing.

## Step 5 — summary

Tell the user: which files were written where, that thresholds can be tuned
later in `~/.config/semantic-compact/config.json` (see the plugin README),
and that the plugin is active in new sessions (or after
`/reload-plugins`).
