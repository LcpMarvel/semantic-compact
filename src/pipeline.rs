use crate::config::{load_config, resolve_data_dir, resolve_plugin_root};
use crate::decision::{
    bump_and_check, decide, load_state, mark_suggested, now_ms, save_state, SessionRecord, State,
};
use crate::env::resolve_api_key;
use crate::judge::{run_judge, HttpRequest, JudgeOutcome};
use crate::log::{append_debug, append_decision, decision_entry};
use crate::task_state::build_task_state;
use crate::transcript::{parse_transcript, ParseOpts};
use serde_json::Value;
use std::path::PathBuf;

pub struct PipelineOpts<'a> {
    pub env_getter: &'a dyn Fn(&str) -> Option<String>,
    pub data_dir: Option<PathBuf>,
    pub plugin_root: Option<PathBuf>,
    pub post: &'a dyn Fn(&HttpRequest) -> Result<String, String>,
}

pub struct PipelineResult {
    pub suggested: bool,
    pub decision: &'static str,
    pub skip_reason: Option<String>,
    pub system_message: Option<String>,
}

// Full judgment pipeline for one submitted prompt. Never fails: every error
// path degrades to a silent skip. A system_message is produced exclusively
// for a high-confidence suggestion.
pub fn run(input: &Value, opts: &PipelineOpts) -> PipelineResult {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_inner(input, opts))) {
        Ok(result) => result,
        Err(_) => PipelineResult {
            suggested: false,
            decision: "SILENT",
            skip_reason: Some("internal_error".into()),
            system_message: None,
        },
    }
}

struct Ctx {
    data_dir: PathBuf,
    session_id: String,
    config: crate::config::Config,
}

#[allow(clippy::too_many_arguments)]
fn finish(
    ctx: &Ctx,
    input: &Value,
    skip_reason: Option<&str>,
    state_stats: Option<&Value>,
    result: Option<&JudgeOutcome>,
    decision: &'static str,
    suggested: bool,
    record: Option<&SessionRecord>,
    state: Option<&State>,
) -> PipelineResult {
    if let (Some(record), Some(state)) = (record, state) {
        if ctx.session_id != "unknown" {
            let mut sessions = state.sessions.clone();
            sessions.insert(ctx.session_id.clone(), record.clone());
            save_state(&ctx.data_dir, &State { sessions });
        }
    }
    append_decision(
        &ctx.data_dir,
        &decision_entry(
            input,
            state_stats,
            result,
            decision,
            skip_reason,
            &ctx.config.thresholds,
            suggested,
        ),
    );
    PipelineResult {
        suggested,
        decision,
        skip_reason: skip_reason.map(String::from),
        system_message: match decision {
            "SUGGEST_COMPACT" => Some(ctx.config.reminder.message.clone()),
            "SUGGEST_CLEAR" => Some(ctx.config.reminder.clear_message.clone()),
            _ => None,
        },
    }
}

fn run_inner(input: &Value, opts: &PipelineOpts) -> PipelineResult {
    let plugin_root = opts
        .plugin_root
        .clone()
        .unwrap_or_else(|| resolve_plugin_root(opts.env_getter));
    let config = load_config(&plugin_root, opts.env_getter);
    let data_dir = opts
        .data_dir
        .clone()
        .unwrap_or_else(|| resolve_data_dir(&plugin_root, opts.env_getter));

    let prompt = input
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let session_id = input
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string();
    let ctx = Ctx {
        data_dir,
        session_id,
        config,
    };

    if prompt.is_empty() {
        return finish(
            &ctx,
            input,
            Some("empty_prompt"),
            None,
            None,
            "SILENT",
            false,
            None,
            None,
        );
    }
    if prompt.starts_with('/') {
        return finish(
            &ctx,
            input,
            Some("slash_command"),
            None,
            None,
            "SILENT",
            false,
            None,
            None,
        );
    }

    let cwd = input.get("cwd").and_then(Value::as_str).map(PathBuf::from);
    let api_key = resolve_api_key(
        opts.env_getter,
        ctx.config.jev.api_key.as_deref(),
        cwd.as_deref(),
        &plugin_root,
    );
    let now = now_ms();
    let Some(api_key) = api_key else {
        // Inactive plugin: on the first keyless prompt of a session, show
        // how to configure the key, then stay quiet for the rest of it.
        let mut state = load_state(&ctx.data_dir);
        let notice = if ctx.session_id != "unknown" {
            let entry = state.sessions.entry(ctx.session_id.clone()).or_default();
            if entry.setup_notice_ts.is_none() {
                entry.setup_notice_ts = Some(now);
                entry.last_seen = now;
                save_state(&ctx.data_dir, &state);
                true
            } else {
                false
            }
        } else {
            false
        };
        let mut result = finish(
            &ctx,
            input,
            Some("no_api_key"),
            None,
            None,
            "SILENT",
            false,
            None,
            None,
        );
        if notice {
            result.system_message = Some(ctx.config.reminder.setup_message.clone());
        }
        return result;
    };

    // Count this turn for the session before anything expensive.
    let state = load_state(&ctx.data_dir);
    let (record, suppressed) = bump_and_check(
        state.sessions.get(&ctx.session_id),
        now,
        &ctx.config.cooldown,
    );
    if suppressed {
        return finish(
            &ctx,
            input,
            Some("cooldown"),
            None,
            None,
            "SILENT",
            false,
            Some(&record),
            Some(&state),
        );
    }

    let parsed = match input
        .get("transcript_path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
    {
        Some(path) => parse_transcript(
            &path,
            &ParseOpts {
                max_user_prompts: ctx.config.history.max_user_prompts,
                max_assistant_outcomes: ctx.config.history.max_assistant_outcomes,
                max_files: ctx.config.history.max_files,
            },
        ),
        None => crate::transcript::ParsedTranscript {
            ok: false,
            error: Some("transcript_unreadable".into()),
            ..Default::default()
        },
    };
    if !parsed.ok {
        return finish(
            &ctx,
            input,
            Some("transcript_unreadable"),
            None,
            None,
            "SILENT",
            false,
            Some(&record),
            Some(&state),
        );
    }

    let task = build_task_state(&parsed, &prompt, &ctx.config.history);
    if ctx.config.debug_logging {
        append_debug(
            &ctx.data_dir,
            serde_json::json!({
                "session_id": ctx.session_id,
                "stdin_keys": input.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()),
                "hook_event_name": input.get("hook_event_name"),
                "state": task.state,
            }),
        );
    }
    if task.prior_prompt_count < ctx.config.skip.min_prior_prompts {
        return finish(
            &ctx,
            input,
            Some("insufficient_history"),
            Some(&task.stats),
            None,
            "SILENT",
            false,
            Some(&record),
            Some(&state),
        );
    }

    let result = run_judge(&task.state, &ctx.config.jev, &api_key, opts.post);
    if !result.ok {
        let reason = format!("judge_{}", result.error.as_deref().unwrap_or("unknown"));
        return finish(
            &ctx,
            input,
            Some(&reason),
            Some(&task.stats),
            Some(&result),
            "SILENT",
            false,
            Some(&record),
            Some(&state),
        );
    }

    let decision = decide(
        &result
            .probabilities
            .expect("ok outcome carries probabilities"),
        &ctx.config.thresholds,
    );
    let suggested = decision != "SILENT";
    let final_record = if suggested {
        mark_suggested(record, now_ms())
    } else {
        record
    };

    if ctx.config.debug_logging {
        append_debug(
            &ctx.data_dir,
            serde_json::json!({ "session_id": ctx.session_id, "judge": format!("{:?}", result), "decision": decision }),
        );
    }

    finish(
        &ctx,
        input,
        None,
        Some(&task.stats),
        Some(&result),
        decision,
        suggested,
        Some(&final_record),
        Some(&state),
    )
}
