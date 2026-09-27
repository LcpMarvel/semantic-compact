use crate::config::{load_config, resolve_data_dir, resolve_plugin_root};
use crate::decision::{
    bump_and_check, decide, load_session, mark_suggested, now_ms, save_session, SessionRecord,
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
    // In block mode a suggestion is returned as a `decision: "block"` that
    // bounces the prompt back to the user (who then clears/compacts and
    // re-sends) instead of a fire-and-forget warning nobody can act on.
    pub block: bool,
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
            block: false,
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
) -> PipelineResult {
    let persisted = record.is_some_and(|record| {
        ctx.session_id != "unknown" && save_session(&ctx.data_dir, &ctx.session_id, record).is_ok()
    });
    if ctx.config.logging.decisions {
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
    }
    let reminder_text = match decision {
        "SUGGEST_COMPACT" => Some(ctx.config.reminder.message.clone()),
        "SUGGEST_CLEAR" => Some(ctx.config.reminder.clear_message.clone()),
        _ => None,
    };
    let block =
        suggested && persisted && reminder_text.is_some() && ctx.config.reminder.mode != "remind";
    let system_message = if block {
        reminder_text.map(|text| {
            format!(
                "{text}\n\nThe prompt was returned to you, not lost. Run /clear or /compact now, \
                 then re-send it; re-sending it as-is simply continues with the current context."
            )
        })
    } else {
        reminder_text
    };
    PipelineResult {
        suggested,
        decision,
        skip_reason: skip_reason.map(String::from),
        system_message,
        block,
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
        );
    }
    if prompt.starts_with("<task-notification>") {
        return finish(
            &ctx,
            input,
            Some("task_notification"),
            None,
            None,
            "SILENT",
            false,
            None,
        );
    }

    let cwd = input.get("cwd").and_then(Value::as_str).map(PathBuf::from);
    let api_key = resolve_api_key(
        &ctx.config.jev.provider,
        opts.env_getter,
        ctx.config.jev.api_key.as_deref(),
        cwd.as_deref(),
        &plugin_root,
    );
    let now = now_ms();
    let Some(api_key) = api_key else {
        // Inactive plugin: on the first keyless prompt of a session, show
        // how to configure the key, then stay quiet for the rest of it.
        let notice = if ctx.session_id != "unknown" {
            let mut record = load_session(&ctx.data_dir, &ctx.session_id).unwrap_or_default();
            if record.setup_notice_ts.is_none() {
                record.setup_notice_ts = Some(now);
                record.last_seen = now;
                save_session(&ctx.data_dir, &ctx.session_id, &record).is_ok()
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
        );
        if notice {
            result.system_message = Some(ctx.config.reminder.setup_message.clone());
        }
        return result;
    };

    // A custom provider with nothing configured cannot reach any endpoint.
    if ctx.config.jev.url.trim().is_empty() {
        return finish(
            &ctx,
            input,
            Some("no_api_url"),
            None,
            None,
            "SILENT",
            false,
            None,
        );
    }

    // Count this turn for the session before anything expensive.
    let prior = if ctx.session_id == "unknown" {
        None
    } else {
        load_session(&ctx.data_dir, &ctx.session_id)
    };
    let (record, suppressed) = bump_and_check(prior.as_ref(), now, &ctx.config.cooldown);

    let parsed = match input
        .get("transcript_path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
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
            error: Some("transcript_path_missing".into()),
            ..Default::default()
        },
    };
    if !parsed.ok {
        return finish(
            &ctx,
            input,
            parsed.error.as_deref(),
            None,
            None,
            "SILENT",
            false,
            Some(&record),
        );
    }

    let task = build_task_state(&parsed, &prompt, &ctx.config.history);
    if ctx.config.logging.debug {
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
        // History only ever grows, so an armed session suddenly dropping
        // below the floor means the user cleared the conversation — usually
        // by acting on our suggestion. The cooldown has served its purpose.
        let record = if record.last_suggest_ts.is_some() {
            SessionRecord {
                last_suggest_ts: None,
                ..record
            }
        } else {
            record
        };
        return finish(
            &ctx,
            input,
            Some("insufficient_history"),
            Some(&task.stats),
            None,
            "SILENT",
            false,
            Some(&record),
        );
    }

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
        );
    }

    let decision = decide(
        &result
            .probabilities
            .expect("ok outcome carries probabilities"),
        &ctx.config.thresholds,
        // Drift persists across adjacent prompts: allow a full recent window
        // between reminders, while genuine task switches keep their fast rearm.
        task.has_older_context
            && (record.last_suggest_ts.is_none()
                || record.prompts_since_suggest
                    > ctx.config.history.max_user_prompts.max(1) as u64),
    );
    let suggested = decision != "SILENT";
    let final_record = if suggested {
        mark_suggested(record, now_ms())
    } else {
        record
    };

    if ctx.config.logging.debug {
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
    )
}
