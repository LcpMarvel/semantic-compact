use crate::config::{Cooldown, Thresholds};
use crate::judge::Probabilities;
use crate::log::sha256_hex;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const STATE_DIR: &str = "sessions";
static NEXT_TMP: AtomicU64 = AtomicU64::new(0);
const SESSION_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionRecord {
    pub last_suggest_ts: Option<u64>,
    pub prompts_since_suggest: u64,
    pub last_seen: u64,
    pub setup_notice_ts: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct LegacyState {
    sessions: std::collections::HashMap<String, SessionRecord>,
}

// Three-way judgment: same task -> silence; new task sharing project context
// -> suggest compact (a summary still has value); new unrelated task ->
// suggest clear (even a summary would be carried dead weight). The shared
// threshold decides between the two suggestion flavors.
pub fn decide(p: &Probabilities, t: &Thresholds, stale_context_eligible: bool) -> &'static str {
    let is_new_task = p.p_new_task >= t.new_task_min && p.p_depends_on_previous <= t.depends_max;
    if !is_new_task {
        if stale_context_eligible && p.p_stale_context >= t.stale_context_min {
            return "SUGGEST_COMPACT";
        }
        return "SILENT";
    }
    if p.p_shared_context >= t.shared_context_min {
        "SUGGEST_COMPACT"
    } else {
        "SUGGEST_CLEAR"
    }
}

fn session_path(data_dir: &Path, session_id: &str) -> PathBuf {
    data_dir
        .join(STATE_DIR)
        .join(format!("{}.json", sha256_hex(session_id)))
}

pub fn load_session(data_dir: &Path, session_id: &str) -> Option<SessionRecord> {
    let path = session_path(data_dir, session_id);
    if path.exists() {
        return std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<SessionRecord>(&text).ok())
            .filter(|rec| now_ms().saturating_sub(rec.last_seen) <= SESSION_TTL_MS);
    }
    // Existing installs may have one shared state.json. Read it until each
    // session gets its own file; the next save migrates just that session.
    std::fs::read_to_string(data_dir.join("state.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<LegacyState>(&text).ok())
        .and_then(|state| state.sessions.get(session_id).cloned())
        .filter(|rec| now_ms().saturating_sub(rec.last_seen) <= SESSION_TTL_MS)
}

pub fn save_session(
    data_dir: &Path,
    session_id: &str,
    record: &SessionRecord,
) -> std::io::Result<()> {
    let path = session_path(data_dir, session_id);
    let dir = path.parent().expect("session path has parent");
    std::fs::create_dir_all(dir)?;
    let is_new = !path.exists();
    let tmp = dir.join(format!(
        "{}.{}.{}.tmp",
        sha256_hex(session_id),
        std::process::id(),
        NEXT_TMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        serde_json::to_writer(&mut file, record).map_err(std::io::Error::other)?;
        file.flush()?;
        std::fs::rename(&tmp, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    } else if is_new {
        prune_expired(dir);
    }
    result
}

// Scan only after a new session is created, so normal prompts touch one file.
fn prune_expired(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = now_ms();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let expired = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<SessionRecord>(&text).ok())
            .is_some_and(|rec| now.saturating_sub(rec.last_seen) > SESSION_TTL_MS);
        if expired {
            let _ = std::fs::remove_file(path);
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
