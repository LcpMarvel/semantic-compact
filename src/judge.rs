use crate::config::JevConfig;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

// Question set for the OpenRouter Decisions API (typed LLM judgments).
// The wording is the calibrated baseline — change it only together with
// re-running the offline fixtures, since thresholds are tuned to it.
pub fn questions() -> Value {
    json!({
        "new_task": {
            "type": "noul",
            "instructions": "Does the newest user prompt begin a materially different task or topic from what the user has been working on or discussing, such that the previous context (debug logs, tool results, explored files, earlier answers) would likely no longer be needed?",
            "criteria": {
                "true": "The prompt starts a new self-contained task, or — in a general conversation — a wholly different subject, with a different goal. In casual chat a new unrelated question (food, another product, a schedule, a different life question) IS a new topic. Cross-layer shifts that still serve the same user goal (e.g. backend fix then frontend button for the same feature) are NOT a new task. Short follow-ups, references to earlier work (continue / fix that / add tests for it / what about X), questions about the current subject, and quick side questions during hands-on work are NOT a new task.",
                "false": "The prompt continues, refines, debugs, extends, or asks about the current task or subject, or depends on prior context."
            }
        },
        "depends_on_previous_context": {
            "type": "noul",
            "instructions": "Would answering the newest prompt well require the conversation history above (prior decisions, code already written, earlier explanations)?",
            "criteria": {
                "true": "The prompt relies on prior context: pronoun references (it/that), continue, the same approach, questions about what was done, or building directly on earlier changes.",
                "false": "The prompt is fully self-contained and could be answered in a fresh session with only the project files. Merely working in the same project, codebase, or feature area is NOT dependency — project structure, conventions, and file contents are available without this conversation and do not count."
            }
        },
        "previous_task_complete": {
            "type": "noul",
            "instructions": "Is the task the user was working on before this prompt complete or explicitly abandoned?",
            "criteria": {
                "true": "The user signaled completion, explicitly moved on, or the conversation shows the task finished.",
                "false": "The task is still in progress, unresolved, or unclear."
            }
        },
        "shares_context_with_previous_task": {
            "type": "noul",
            "instructions": "Assuming this is a new task: does it share meaningful context with the previous task — the same application, repo, service, or module, or overlapping files, domain, or constraints — such that a brief summary of the previous work would still be useful for it?",
            "criteria": {
                "true": "The tasks share context worth keeping a summary for: same application, repo, service, or module — e.g. from the OAuth backend to the same app's pricing page — or overlapping files, domain, or constraints. The goal changed, but a compacted summary of earlier work (files changed, decisions, conventions) would still help.",
                "false": "Essentially unrelated: a different project or concern with no meaningful overlap, where a summary of the previous task would inform almost nothing about the new one — e.g. jumping from a web app's login flow to a standalone ffmpeg script."
            }
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probabilities {
    pub p_new_task: f64,
    pub p_depends_on_previous: f64,
    pub p_complete: f64,
    pub p_shared_context: f64,
}

#[derive(Debug, Clone)]
pub struct JudgeOutcome {
    pub ok: bool,
    pub error: Option<String>,
    pub latency_ms: u64,
    pub probabilities: Option<Probabilities>,
    pub model: Option<String>,
    pub gen_id: Option<String>,
    pub cost: Option<f64>,
}

impl JudgeOutcome {
    fn err(error: &str, latency_ms: u64) -> Self {
        Self {
            ok: false,
            error: Some(error.to_string()),
            latency_ms,
            probabilities: None,
            model: None,
            gen_id: None,
            cost: None,
        }
    }
}

fn probability(v: f64) -> Option<f64> {
    if v.is_finite() && (0.0..=1.0).contains(&v) {
        Some(v)
    } else {
        None
    }
}

// A complete HTTP request, auth headers already applied. Keeping this as a
// plain struct lets tests mock the transport without any HTTP stack.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub url: String,
    pub body: String,
    pub headers: Vec<(String, String)>,
    pub timeout_ms: u64,
}

pub type PostFn<'a> = dyn Fn(&HttpRequest) -> Result<String, String> + 'a;

// Never panics; every failure mode comes back as ok=false.
pub fn run_judge(state: &str, cfg: &JevConfig, api_key: &str, post: &PostFn) -> JudgeOutcome {
    let started = Instant::now();

    let auth_header = if cfg.auth_header.is_empty() {
        "Authorization"
    } else {
        cfg.auth_header.as_str()
    };
    let auth_value = if cfg.auth_scheme.is_empty() {
        api_key.to_string()
    } else {
        format!("{} {}", cfg.auth_scheme, api_key)
    };
    let request = HttpRequest {
        url: cfg.url.clone(),
        body: json!({ "model": cfg.model, "state": state, "questions": questions() }).to_string(),
        headers: vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            (auth_header.to_string(), auth_value),
            ("X-Title".to_string(), "semantic-compact".to_string()),
        ],
        timeout_ms: cfg.timeout_ms,
    };

    let response = match post(&request) {
        Ok(r) => r,
        Err(e) => return JudgeOutcome::err(&e, started.elapsed().as_millis() as u64),
    };
    let latency = started.elapsed().as_millis() as u64;

    let parsed: Value = match serde_json::from_str(&response) {
        Ok(v) => v,
        Err(_) => return JudgeOutcome::err("schema", latency),
    };
    let Some(answers) = parsed.get("answers") else {
        return JudgeOutcome::err("schema", latency);
    };

    let noul = |name: &str| -> Option<f64> {
        answers
            .get(name)
            .and_then(|a| a.get("noul"))
            .and_then(Value::as_f64)
            .and_then(probability)
    };

    let probabilities = match (
        noul("new_task"),
        noul("depends_on_previous_context"),
        noul("previous_task_complete"),
        noul("shares_context_with_previous_task"),
    ) {
        (Some(a), Some(b), Some(c), Some(d)) => Probabilities {
            p_new_task: a,
            p_depends_on_previous: b,
            p_complete: c,
            p_shared_context: d,
        },
        _ => return JudgeOutcome::err("schema", latency),
    };

    JudgeOutcome {
        ok: true,
        error: None,
        latency_ms: latency,
        probabilities: Some(probabilities),
        model: parsed
            .get("model")
            .and_then(Value::as_str)
            .map(String::from),
        gen_id: parsed.get("id").and_then(Value::as_str).map(String::from),
        cost: parsed.pointer("/usage/cost").and_then(Value::as_f64),
    }
}

// Production transport over ureq. Error strings are stable tokens
// ("timeout" / "network" / "http_<status>") and carry no secrets.
pub fn http_post(req: &HttpRequest) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(req.timeout_ms.max(1)))
        .build();
    let mut request = agent.post(&req.url);
    for (name, value) in &req.headers {
        request = request.set(name, value);
    }
    match request.send_string(&req.body) {
        Ok(res) => {
            if res.status() >= 400 {
                return Err(format!("http_{}", res.status()));
            }
            res.into_string().map_err(|_| "network".to_string())
        }
        Err(ureq::Error::Status(code, _)) => Err(format!("http_{}", code)),
        Err(ureq::Error::Transport(t)) => {
            // ureq surfaces agent timeouts as an Io transport error; the kind
            // distinction only matters for the log's error field.
            if t.to_string().to_lowercase().contains("timed out") {
                Err("timeout".to_string())
            } else {
                Err("network".to_string())
            }
        }
    }
}
