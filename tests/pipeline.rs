use semantic_compact::config::{load_config, resolve_data_dir, Cooldown, JevConfig, Thresholds};
use semantic_compact::decision::{bump_and_check, decide, mark_suggested, SessionRecord};
use semantic_compact::judge::{run_judge, HttpRequest, Probabilities};
use semantic_compact::pipeline::{run, PipelineOpts};
use semantic_compact::task_state::build_task_state;
use semantic_compact::transcript::{is_real_user_prompt, parse_transcript, ParseOpts};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/fixtures")
        .join(name)
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sc-test-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn env_map<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |name| map.get(name).cloned()
}

fn decision_response(p_new: f64, p_depends: f64, p_complete: f64, p_shared: f64) -> String {
    json!({
        "model": "typesafe/jev-1.13-test",
        "answers": {
            "new_task": { "type": "noul", "noul": p_new },
            "depends_on_previous_context": { "type": "noul", "noul": p_depends },
            "previous_task_complete": { "type": "noul", "noul": p_complete },
            "shares_context_with_previous_task": { "type": "noul", "noul": p_shared }
        },
        "usage": { "input_tokens": 700, "output_tokens": 60, "cost": 3.0e-5 },
        "id": "gen-dec-test",
        "provider": "TypeSafe"
    })
    .to_string()
}

fn stdin(prompt: &str, transcript: &Path) -> Value {
    json!({
        "session_id": "sess-test",
        "transcript_path": transcript.to_string_lossy(),
        "cwd": "/tmp",
        "permission_mode": "default",
        "hook_event_name": "UserPromptSubmit",
        "prompt": prompt
    })
}

fn run_pipeline(
    input: &Value,
    data_dir: &Path,
    post: &dyn Fn(&HttpRequest) -> Result<String, String>,
) -> semantic_compact::pipeline::PipelineResult {
    // isolated plugin root so the repo's own .env / config.json never leak in
    let plugin_root = temp_dir("pluginroot");
    let env = env_map(&[
        ("JEV_API_KEY", "sk-test-key-123"),
        ("SC_LOG_DECISIONS", "1"),
    ]);
    let opts = PipelineOpts {
        env_getter: &env,
        data_dir: Some(data_dir.to_path_buf()),
        plugin_root: Some(plugin_root),
        post,
    };
    run(input, &opts)
}

fn read_decisions(data_dir: &Path) -> Vec<Value> {
    let logs = data_dir.join("logs");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&logs) {
        for e in entries.flatten() {
            let path = e.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .starts_with("decisions-")
            {
                for line in std::fs::read_to_string(&path).unwrap_or_default().lines() {
                    if let Ok(v) = serde_json::from_str::<Value>(line) {
                        out.push(v);
                    }
                }
            }
        }
    }
    out
}

#[test]
fn transcript_parses_oauth_session() {
    let parsed = parse_transcript(&fixture("oauth-session.jsonl"), &ParseOpts::default());
    assert!(parsed.ok);
    let texts: Vec<&str> = parsed
        .user_prompts
        .iter()
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(
        texts,
        vec![
            "帮我实现 OAuth 登录",
            "refresh token 怎么存?",
            "给它补一下测试"
        ]
    );
    assert_eq!(parsed.total_user_prompts, 3);
    assert_eq!(parsed.assistant_outcomes.len(), 3);
    assert!(parsed.files.contains(&"src/auth/oauth.ts".to_string()));
    assert!(parsed.files.contains(&"src/db/tokens.ts".to_string()));
    assert!(parsed.files.contains(&"src/auth/oauth.test.ts".to_string()));
    assert!(parsed.files.contains(&"package.json".to_string()));
}

#[test]
fn transcript_filters_noise() {
    let parsed = parse_transcript(&fixture("noise-only.jsonl"), &ParseOpts::default());
    assert!(parsed.ok);
    assert_eq!(parsed.total_user_prompts, 0);
    assert!(parsed.user_prompts.is_empty());
}

#[test]
fn user_prompt_filter_rules() {
    assert!(is_real_user_prompt("继续改这个 bug"));
    assert!(!is_real_user_prompt("/compact"));
    assert!(!is_real_user_prompt("  "));
    assert!(!is_real_user_prompt(
        "[Request interrupted by user for tool use]"
    ));
    assert!(!is_real_user_prompt("Caveat: whatever follows"));
}

#[test]
fn task_state_dedupes_current_prompt_and_counts_priors() {
    let parsed = parse_transcript(&fixture("oauth-session.jsonl"), &ParseOpts::default());
    let history = semantic_compact::config::History::default();
    let task = build_task_state(
        &parsed,
        "登录先到这里。现在帮我重新设计 pricing 页面",
        &history,
    );
    assert_eq!(task.prior_prompt_count, 3);

    // if the transcript already contains the triggering prompt, it is not counted
    let task_dupe = build_task_state(&parsed, "给它补一下测试", &history);
    assert_eq!(task_dupe.prior_prompt_count, 2);

    let s = &task.state;
    assert!(s.contains("== Recent conversation (oldest first) =="));
    assert!(s.contains("== Files recently touched =="));
    assert!(s.contains("== Newest user prompt =="));
    assert!(s.contains("User: 帮我实现 OAuth 登录"));
    assert!(s.contains("Assistant:"));
}

#[test]
fn decide_threshold_boundaries() {
    let t = Thresholds {
        new_task_min: 0.90,
        depends_max: 0.20,
        shared_context_min: 0.50,
    };
    let mk = |a: f64, b: f64, c: f64| Probabilities {
        p_new_task: a,
        p_depends_on_previous: b,
        p_complete: 0.5,
        p_shared_context: c,
    };
    assert_eq!(decide(&mk(0.90, 0.20, 0.50), &t), "SUGGEST_COMPACT");
    assert_eq!(decide(&mk(0.899, 0.20, 0.50), &t), "SILENT");
    assert_eq!(decide(&mk(0.95, 0.21, 0.50), &t), "SILENT");
    assert_eq!(decide(&mk(0.95, 0.10, 0.49), &t), "SUGGEST_CLEAR");
    assert_eq!(decide(&mk(0.95, 0.10, 0.50), &t), "SUGGEST_COMPACT");
    assert_eq!(decide(&mk(0.60, 0.10, 0.10), &t), "SILENT"); // tangent: low new_task despite low shared
}

#[test]
fn cooldown_window_semantics() {
    let cd = Cooldown {
        prompts: 8,
        seconds: 600,
    };
    let now = 1_000_000u64;

    let (rec, supp) = bump_and_check(None, now, &cd);
    assert!(!supp); // no suggestion yet -> nothing suppressed

    let rec = mark_suggested(rec, now);
    // both windows active (1s later, 1st prompt since suggestion)
    let (_, s1) = bump_and_check(Some(&rec), now + 1_000, &cd);
    assert!(s1);
    // prompt budget exhausted, time window still active (60s < 600s)
    let mut r = rec.clone();
    r.prompts_since_suggest = 9;
    let (_, s2) = bump_and_check(Some(&r), now + 60_000, &cd);
    assert!(s2);
    // time window exhausted, prompt budget still active
    let (_, s3) = bump_and_check(Some(&rec), now + 700_000, &cd);
    assert!(s3);
    // both exhausted -> free to suggest again
    let mut r2 = rec.clone();
    r2.prompts_since_suggest = 9;
    let (_, s4) = bump_and_check(Some(&r2), now + 700_000, &cd);
    assert!(!s4);
}

#[test]
fn judge_parses_success_and_failures() {
    let cfg = JevConfig::default();
    let ok = |body: String| move |_req: &HttpRequest| Ok(body.clone());
    let outcome = run_judge(
        "state",
        &cfg,
        "key",
        &ok(decision_response(0.93, 0.18, 0.9, 0.7)),
    );
    assert!(outcome.ok);
    let p = outcome.probabilities.unwrap();
    assert!((p.p_new_task - 0.93).abs() < 1e-9);
    assert_eq!(outcome.model.as_deref(), Some("typesafe/jev-1.13-test"));

    let schema = |_req: &HttpRequest| Ok("{\"answers\": {}}".to_string());
    assert!(!run_judge("state", &cfg, "key", &schema).ok);

    let http = |_req: &HttpRequest| Err("http_500".to_string());
    let o = run_judge("state", &cfg, "key", &http);
    assert!(!o.ok && o.error.as_deref() == Some("http_500"));

    let bad_json = |_req: &HttpRequest| Ok("not json".to_string());
    assert!(!run_judge("state", &cfg, "key", &bad_json).ok);
}

#[test]
fn judge_out_of_range_probabilities_are_clamped() {
    let cfg = JevConfig::default();
    let resp = |_req: &HttpRequest| Ok(decision_response(1.4, -0.2, 0.5, 0.5));
    let o = run_judge("state", &cfg, "k", &resp);
    assert!(o.ok);
    let p = o.probabilities.unwrap();
    assert_eq!(p.p_new_task, 1.0);
    assert_eq!(p.p_depends_on_previous, 0.0);
}

#[test]
fn config_layers_and_env_override() {
    let dir = temp_dir("cfg");
    std::fs::write(
        dir.join("config.json"),
        r#"{"thresholds": {"depends_max": 0.3}, "jev": {"model": "custom/model"}}"#,
    )
    .unwrap();

    let env = env_map(&[]);
    let cfg = load_config(&dir, &env);
    assert!((cfg.thresholds.depends_max - 0.3).abs() < 1e-9);
    assert_eq!(cfg.jev.model, "custom/model");
    assert!((cfg.thresholds.new_task_min - 0.90).abs() < 1e-9); // untouched default

    let env2 = env_map(&[("SC_NEW_TASK_MIN", "0.95")]);
    let cfg2 = load_config(&dir, &env2);
    assert!((cfg2.thresholds.new_task_min - 0.95).abs() < 1e-9);
    assert!((cfg2.thresholds.depends_max - 0.3).abs() < 1e-9); // JSON value survives
}

#[test]
fn data_dir_resolution_order() {
    let env = env_map(&[]);
    assert_eq!(
        resolve_data_dir(&PathBuf::from("/plugin"), &env),
        PathBuf::from("/plugin/data")
    );
    let env2 = env_map(&[("CLAUDE_PLUGIN_DATA", "/pd")]);
    assert_eq!(
        resolve_data_dir(&PathBuf::from("/plugin"), &env2),
        PathBuf::from("/pd")
    );
    let env3 = env_map(&[("CLAUDE_PLUGIN_DATA", "/pd"), ("SC_DATA_DIR", "/custom")]);
    assert_eq!(
        resolve_data_dir(&PathBuf::from("/plugin"), &env3),
        PathBuf::from("/custom")
    );
}

#[test]
fn pipeline_suggests_on_clear_task_switch() {
    let data = temp_dir("suggest");
    let calls = AtomicUsize::new(0);
    let post = |_req: &HttpRequest| -> Result<String, String> {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(decision_response(0.93, 0.18, 0.93, 0.7))
    };
    let input = stdin(
        "登录先到这里。现在帮我重新设计 pricing 页面",
        &fixture("oauth-session.jsonl"),
    );
    let result = run_pipeline(&input, &data, &post);

    assert!(result.suggested);
    assert_eq!(result.decision, "SUGGEST_COMPACT");
    assert!(result.block, "default reminder mode blocks the prompt");
    let msg = result.system_message.expect("reminder present");
    assert!(msg.contains("Semantic Compact"));
    assert!(msg.contains("/compact"));
    assert!(msg.contains("nothing was compacted"));
    assert!(msg.contains("re-send"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let entries = read_decisions(&data);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["decision"], "SUGGEST_COMPACT");
    assert_eq!(entries[0]["suggested"], true);
    assert!((entries[0]["p_new_task"].as_f64().unwrap() - 0.93).abs() < 1e-9);
    assert!((entries[0]["p_shared_context"].as_f64().unwrap() - 0.7).abs() < 1e-9);
    assert!(entries[0]["jev"]["model"].is_string());

    // the api key must never appear in the logs
    for path in [data.join("logs"), data.clone()] {
        for e in std::fs::read_dir(&path).unwrap().flatten() {
            if e.path().is_file() {
                let content = std::fs::read_to_string(e.path()).unwrap_or_default();
                assert!(
                    !content.contains("sk-test-key-123"),
                    "key leaked into {}",
                    e.path().display()
                );
            }
        }
    }
}

#[test]
fn pipeline_stays_silent_on_same_task() {
    let data = temp_dir("silent");
    let post = |_req: &HttpRequest| Ok(decision_response(0.05, 0.95, 0.10, 0.5));
    let input = stdin("给它补一下测试", &fixture("oauth-session.jsonl"));
    let result = run_pipeline(&input, &data, &post);
    assert!(!result.suggested);
    assert!(result.system_message.is_none());
    assert_eq!(result.decision, "SILENT");
    assert!(result.skip_reason.is_none());
}

#[test]
fn pipeline_cooldown_suppresses_after_suggestion() {
    let data = temp_dir("cooldown");
    let calls = AtomicUsize::new(0);
    let post = |_req: &HttpRequest| -> Result<String, String> {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(decision_response(0.95, 0.05, 0.9, 0.6))
    };
    let transcript = fixture("oauth-session.jsonl");
    let input = stdin("现在做 pricing 页面", &transcript);
    assert!(run_pipeline(&input, &data, &post).suggested);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let input2 = stdin("再来一个全新的任务", &transcript);
    let result2 = run_pipeline(&input2, &data, &post);
    assert!(!result2.suggested);
    assert_eq!(result2.skip_reason.as_deref(), Some("cooldown"));
    assert_eq!(calls.load(Ordering::SeqCst), 1); // judge not called again
}

#[test]
fn pipeline_skips_without_key_or_with_slash_or_empty_prompt() {
    let data = temp_dir("skips");
    let transcript = fixture("oauth-session.jsonl");
    let post = |_req: &HttpRequest| Ok(decision_response(0.95, 0.05, 0.9, 0.6));

    // slash command
    let r1 = run_pipeline(&stdin("/compact", &transcript), &data, &post);
    assert_eq!(r1.skip_reason.as_deref(), Some("slash_command"));
    // empty prompt
    let r2 = run_pipeline(&stdin("   ", &transcript), &data, &post);
    assert_eq!(r2.skip_reason.as_deref(), Some("empty_prompt"));

    // no key anywhere: env map empty, isolated plugin root, no .env files
    let env = env_map(&[("SC_LOG_DECISIONS", "1")]);
    let opts = PipelineOpts {
        env_getter: &env,
        data_dir: Some(data.clone()),
        plugin_root: Some(temp_dir("pluginroot")),
        post: &post,
    };
    let r3 = run(&stdin("随便什么", &transcript), &opts);
    assert_eq!(r3.skip_reason.as_deref(), Some("no_api_key"));
    // first keyless prompt of a session carries the setup notice
    let notice = r3
        .system_message
        .expect("setup notice on first keyless prompt");
    assert!(notice.contains("no API key"));
    assert!(notice.contains("~/.config/semantic-compact"));

    let entries = read_decisions(&data);
    assert!(entries.iter().any(|e| e["skip_reason"] == "no_api_key"));
    assert!(entries.iter().any(|e| e["skip_reason"] == "slash_command"));
}

#[test]
fn pipeline_skips_when_history_insufficient() {
    let data = temp_dir("fresh");
    let post = |_req: &HttpRequest| Ok(decision_response(0.95, 0.05, 0.9, 0.6));
    let result = run_pipeline(
        &stdin("再帮我做点别的", &fixture("fresh-session.jsonl")),
        &data,
        &post,
    );
    assert_eq!(result.skip_reason.as_deref(), Some("insufficient_history"));
    assert!(!result.suggested);
}

#[test]
fn pipeline_fails_open_on_judge_error() {
    let data = temp_dir("judgefail");
    let post = |_req: &HttpRequest| Err("timeout".to_string());
    let result = run_pipeline(
        &stdin("现在开始全新任务", &fixture("oauth-session.jsonl")),
        &data,
        &post,
    );
    assert!(!result.suggested);
    assert_eq!(result.skip_reason.as_deref(), Some("judge_timeout"));
    let entries = read_decisions(&data);
    assert_eq!(entries[0]["error"], "timeout");
}

#[test]
fn pipeline_fails_open_on_missing_transcript() {
    let data = temp_dir("notranscript");
    let post = |_req: &HttpRequest| Ok(decision_response(0.95, 0.05, 0.9, 0.6));
    let result = run_pipeline(
        &stdin("新任务", &PathBuf::from("/nonexistent/transcript.jsonl")),
        &data,
        &post,
    );
    assert_eq!(result.skip_reason.as_deref(), Some("transcript_unreadable"));
}

#[test]
fn pipeline_reads_key_from_project_dotenv() {
    let data = temp_dir("dotenv");
    let cwd = temp_dir("dotenvcwd");
    std::fs::write(cwd.join(".env"), "JEV_API_KEY=sk-from-dotenv\n").unwrap();
    let calls = AtomicUsize::new(0);
    let post = |_req: &HttpRequest| -> Result<String, String> {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(decision_response(0.05, 0.9, 0.1, 0.5))
    };
    let empty_env = env_map(&[]);
    let opts = PipelineOpts {
        env_getter: &empty_env,
        data_dir: Some(data.clone()),
        plugin_root: Some(temp_dir("pluginroot")),
        post: &post,
    };
    let mut input = stdin("给它补一下测试", &fixture("oauth-session.jsonl"));
    input["cwd"] = json!(cwd.to_string_lossy());
    let result = run(&input, &opts);
    assert!(result.skip_reason.is_none()); // got past the key check
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn pipeline_suggests_clear_for_unrelated_task() {
    let data = temp_dir("clear");
    let post = |_req: &HttpRequest| Ok(decision_response(0.96, 0.05, 0.9, 0.08));
    let input = stdin(
        "登录先到这里。现在帮我写一个 ffmpeg 批量转码脚本,和这个项目无关",
        &fixture("oauth-session.jsonl"),
    );
    let result = run_pipeline(&input, &data, &post);

    assert!(result.suggested);
    assert_eq!(result.decision, "SUGGEST_CLEAR");
    assert!(result.block);
    let msg = result.system_message.expect("clear reminder present");
    assert!(msg.contains("/clear"));
    assert!(msg.contains("/compact"));
    assert!(msg.contains("nothing was cleared"));
    assert!(msg.contains("re-send"));

    let entries = read_decisions(&data);
    assert_eq!(entries[0]["decision"], "SUGGEST_CLEAR");
    assert_eq!(entries[0]["suggested"], true);
}

#[test]
fn pipeline_setup_notice_shows_once_per_session() {
    let data = temp_dir("notice");
    let post = |_req: &HttpRequest| Ok(decision_response(0.95, 0.05, 0.9, 0.6));
    let env = env_map(&[]);
    let transcript = fixture("oauth-session.jsonl");

    let opts = PipelineOpts {
        env_getter: &env,
        data_dir: Some(data.clone()),
        plugin_root: Some(temp_dir("pluginroot")),
        post: &post,
    };
    let first = run(&stdin("随便什么", &transcript), &opts);
    assert!(first.system_message.is_some());
    let second = run(&stdin("再来一句", &transcript), &opts);
    assert!(second.system_message.is_none());
    assert_eq!(second.skip_reason.as_deref(), Some("no_api_key"));
}

#[test]
fn jev_provider_presets() {
    let dir = temp_dir("presets");

    // default provider is openrouter and fills url/model
    let env = env_map(&[]);
    let cfg = load_config(&dir, &env);
    assert_eq!(cfg.jev.provider, "openrouter");
    assert_eq!(cfg.jev.url, "https://openrouter.ai/api/alpha/decisions");
    assert_eq!(cfg.jev.model, "~typesafe/jev-latest");

    // provider from the plugin option env var switches the preset
    let env = env_map(&[("CLAUDE_PLUGIN_OPTION_JEV_PROVIDER", "typesafe")]);
    let cfg = load_config(&dir, &env);
    assert_eq!(cfg.jev.url, "https://api.typesafe.ai/v1/systemone");
    assert_eq!(cfg.jev.model, "jev-latest");

    // explicit url/model from a config layer survive the preset
    std::fs::write(
        dir.join("config.json"),
        r#"{"jev": {"url": "https://gw.example/decisions", "model": "other"}}"#,
    )
    .unwrap();
    let cfg = load_config(&dir, &env_map(&[]));
    assert_eq!(cfg.jev.url, "https://gw.example/decisions");
    assert_eq!(cfg.jev.model, "other");

    // a plain env var still beats the preset
    let env = env_map(&[("JEV_DECISIONS_URL", "https://env.example/d")]);
    let cfg = load_config(&dir, &env);
    assert_eq!(cfg.jev.url, "https://env.example/d");

    // custom provider with nothing configured leaves the url empty
    let empty = temp_dir("presets-empty");
    let cfg = load_config(
        &empty,
        &env_map(&[("CLAUDE_PLUGIN_OPTION_JEV_PROVIDER", "custom")]),
    );
    assert!(cfg.jev.url.is_empty());
}

#[test]
fn pipeline_skips_custom_provider_without_url() {
    let data = temp_dir("nourl");
    let post = |_req: &HttpRequest| Ok(decision_response(0.95, 0.05, 0.9, 0.6));
    let env = env_map(&[
        ("JEV_API_KEY", "sk-test"),
        ("CLAUDE_PLUGIN_OPTION_JEV_PROVIDER", "custom"),
    ]);
    let opts = PipelineOpts {
        env_getter: &env,
        data_dir: Some(data.clone()),
        plugin_root: Some(temp_dir("pluginroot")),
        post: &post,
    };
    let result = run(&stdin("随便什么", &fixture("oauth-session.jsonl")), &opts);
    assert_eq!(result.skip_reason.as_deref(), Some("no_api_url"));
}

#[test]
fn pipeline_remind_mode_keeps_old_behavior() {
    let data = temp_dir("remind");
    let cwd = temp_dir("remindcwd");
    let cfg_dir = cwd.join(".config").join("semantic-compact");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.json"),
        r#"{"reminder": {"mode": "remind"}}"#,
    )
    .unwrap();
    let post = |_req: &HttpRequest| Ok(decision_response(0.95, 0.05, 0.9, 0.6));
    // HOME points at the config dir so user-layer config.json is found
    let home = cwd.to_string_lossy().into_owned();
    let pairs = [("JEV_API_KEY", "sk-test"), ("HOME", home.as_str())];
    let env = env_map(&pairs);
    let opts = PipelineOpts {
        env_getter: &env,
        data_dir: Some(data.clone()),
        plugin_root: Some(temp_dir("pluginroot")),
        post: &post,
    };
    let result = run(
        &stdin("全新无关任务", &fixture("oauth-session.jsonl")),
        &opts,
    );
    assert!(result.suggested);
    assert!(!result.block);
    let msg = result.system_message.unwrap();
    assert!(!msg.contains("re-send"));
}

#[test]
fn pipeline_setup_notice_never_blocks() {
    let data = temp_dir("noticeblock");
    let env = env_map(&[]);
    let post = |_req: &HttpRequest| Ok(decision_response(0.95, 0.05, 0.9, 0.6));
    let opts = PipelineOpts {
        env_getter: &env,
        data_dir: Some(data.clone()),
        plugin_root: Some(temp_dir("pluginroot")),
        post: &post,
    };
    let result = run(&stdin("随便什么", &fixture("oauth-session.jsonl")), &opts);
    assert_eq!(result.skip_reason.as_deref(), Some("no_api_key"));
    assert!(result.system_message.is_some());
    assert!(!result.block, "informational notices must never block");
}

#[test]
fn decision_log_is_opt_in() {
    let data = temp_dir("nolog");
    let post = |_req: &HttpRequest| Ok(decision_response(0.95, 0.05, 0.9, 0.6));
    // no SC_LOG_DECISIONS anywhere: nothing may be written
    let env = env_map(&[("JEV_API_KEY", "sk-test-key")]);
    let opts = PipelineOpts {
        env_getter: &env,
        data_dir: Some(data.clone()),
        plugin_root: Some(temp_dir("pluginroot")),
        post: &post,
    };
    let result = run(&stdin("全新任务", &fixture("oauth-session.jsonl")), &opts);
    assert!(result.suggested);
    assert!(
        read_decisions(&data).is_empty(),
        "no decision log without opt-in"
    );
    assert!(!data.join("logs").exists());

    // opted in via env: the judgment lands
    let env_on = env_map(&[("JEV_API_KEY", "sk-test-key"), ("SC_LOG_DECISIONS", "true")]);
    let opts_on = PipelineOpts {
        env_getter: &env_on,
        data_dir: Some(data.clone()),
        plugin_root: Some(temp_dir("pluginroot")),
        post: &post,
    };
    let _ = run(
        &stdin("全新任务2", &fixture("oauth-session.jsonl")),
        &opts_on,
    );
    assert_eq!(read_decisions(&data).len(), 1);
}

#[test]
fn clearing_the_conversation_rearms_the_cooldown() {
    let data = temp_dir("clearreset");
    let post = |_req: &HttpRequest| Ok(decision_response(0.95, 0.05, 0.9, 0.6));
    let full = fixture("oauth-session.jsonl");
    let fresh = fixture("fresh-session.jsonl"); // 1 prior prompt

    // a suggestion arms the cooldown
    let r1 = run_pipeline(&stdin("全新任务", &full), &data, &post);
    assert!(r1.suggested);

    // a prompt on a wiped conversation (fewer priors than the floor) must
    // disarm the cooldown instead of skipping with `cooldown`
    let r2 = run_pipeline(&stdin("清空后的新任务", &fresh), &data, &post);
    assert_eq!(r2.skip_reason.as_deref(), Some("insufficient_history"));
    assert!(!r2.block);

    // the very next switch can therefore suggest again
    let r3 = run_pipeline(&stdin("又一个全新任务", &full), &data, &post);
    assert!(
        r3.suggested,
        "cooldown must not survive a conversation clear"
    );
}

#[test]
fn pipeline_session_record_roundtrips_through_state_file() {
    let data = temp_dir("statefile");
    let state = semantic_compact::decision::load_state(&data);
    assert!(state.sessions.is_empty());
    let now = semantic_compact::decision::now_ms();
    let mut sessions = state.sessions.clone();
    sessions.insert(
        "s1".to_string(),
        SessionRecord {
            last_suggest_ts: Some(42),
            prompts_since_suggest: 0,
            last_seen: now,
            setup_notice_ts: None,
        },
    );
    semantic_compact::decision::save_state(&data, &semantic_compact::decision::State { sessions });
    let reloaded = semantic_compact::decision::load_state(&data);
    assert_eq!(
        reloaded.sessions.get("s1").and_then(|r| r.last_suggest_ts),
        Some(42)
    );
}
