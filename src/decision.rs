use crate::config::{Cooldown, Thresholds};
use crate::judge::Probabilities;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

const STATE_FILE: &str = "state.json";
const MAX_SESSIONS: usize = 500;
const SESSION_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionRecord {
    pub last_suggest_ts: Option<u64>,
    pub prompts_since_suggest: u64,
    pub last_seen: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub sessions: HashMap<String, SessionRecord>,
}

// Three-way judgment: same task -> silence; new task sharing project context
// -> suggest compact (a summary still has value); new unrelated task ->
// suggest clear (even a summary would be carried dead weight). The shared
// threshold decides between the two suggestion flavors.
pub fn decide(p: &Probabilities, t: &Thresholds) -> &'static str {
    let is_new_task = p.p_new_task >= t.new_task_min && p.p_depends_on_previous <= t.depends_max;
    if !is_new_task {
        return "SILENT";
    }
    if p.p_shared_context >= t.shared_context_min {
        "SUGGEST_COMPACT"
    } else {
        "SUGGEST_CLEAR"
    }
}

pub fn load_state(data_dir: &Path) -> State {
    std::fs::read_to_string(data_dir.join(STATE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

// Cooldown state is best-effort: failing to persist must never break the
// prompt, so errors are swallowed.
pub fn save_state(data_dir: &Path, state: &State) {
    let dir = data_dir.to_path_buf();
    let now = now_ms();
    let mut sessions: HashMap<String, SessionRecord> = HashMap::new();
    for (id, rec) in &state.sessions {
        if sessions.len() >= MAX_SESSIONS {
            break;
        }
        if now.saturating_sub(rec.last_seen) > SESSION_TTL_MS {
            continue;
        }
        sessions.insert(id.clone(), rec.clone());
    }
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join(STATE_FILE.to_string() + ".tmp");
    let path = dir.join(STATE_FILE);
    if let Ok(json) = serde_json::to_string(&State { sessions }) {
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

// Counts this prompt for the session and reports whether suggestions are
// still suppressed by the cooldown window.
pub fn bump_and_check(
    record: Option<&SessionRecord>,
    now: u64,
    cooldown: &Cooldown,
) -> (SessionRecord, bool) {
    let mut rec = record.cloned().unwrap_or_default();
    rec.prompts_since_suggest += 1;
    rec.last_seen = now;

    let suppressed = match rec.last_suggest_ts {
        Some(last) => {
            rec.prompts_since_suggest <= cooldown.prompts
                || now.saturating_sub(last) < cooldown.seconds * 1000
        }
        None => false,
    };
    (rec, suppressed)
}

pub fn mark_suggested(mut record: SessionRecord, now: u64) -> SessionRecord {
    record.last_suggest_ts = Some(now);
    record.prompts_since_suggest = 0;
    record
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
