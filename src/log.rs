use crate::config::Thresholds;
use crate::judge::JudgeOutcome;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

const PREVIEW_CHARS: usize = 120;

pub fn sha256_hex(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

// UTF-8 safe single-line preview.
pub fn preview(text: &str) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(PREVIEW_CHARS).collect()
}

// UTC (timestamp, day) without pulling in a datetime crate.
fn utc_now() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );

    // civil_from_days (Howard Hinnant), epoch 1970-01-01 = day 0
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    (
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, hh, mm, ss),
        format!("{:04}-{:02}-{:02}", y, mo, d),
    )
}

fn append_jsonl(path: &Path, entry: &Value) -> bool {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    use std::io::Write;
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return false;
    };
    let mut line = entry.to_string();
    line.push('\n');
    file.write_all(line.as_bytes()).is_ok()
}

// One JSON line per boundary judgment. Append-only, local only.
pub fn append_decision(data_dir: &Path, entry: &Value) -> bool {
    let (ts, day) = utc_now();
    let mut full = entry.clone();
    if let Some(obj) = full.as_object_mut() {
        obj.insert("v".to_string(), json!(1));
        obj.insert("ts".to_string(), json!(ts));
    }
    append_jsonl(
        &data_dir
            .join("logs")
            .join(format!("decisions-{}.jsonl", day)),
        &full,
    )
}

// Extra detail for calibration sessions: contains raw prompt/state text,
// which is why it sits behind an explicit opt-in flag.
pub fn append_debug(data_dir: &Path, entry: Value) -> bool {
    let (ts, day) = utc_now();
    let mut full = entry.clone();
    if let Some(obj) = full.as_object_mut() {
        obj.insert("v".to_string(), json!(1));
        obj.insert("ts".to_string(), json!(ts));
    }
    append_jsonl(
        &data_dir.join("logs").join(format!("debug-{}.jsonl", day)),
        &full,
    )
}

pub fn decision_entry(
    input: &Value,
    state_stats: Option<&Value>,
    result: Option<&JudgeOutcome>,
    decision: &str,
    skip_reason: Option<&str>,
    thresholds: &Thresholds,
    suggested: bool,
) -> Value {
    let prompt = input.get("prompt").and_then(Value::as_str).unwrap_or("");
    let session_id = input.get("session_id").and_then(Value::as_str);
    let probabilities = result.and_then(|r| r.probabilities);

    json!({
        "session_id": session_id,
        "prompt_sha256": sha256_hex(prompt),
        "prompt_chars": prompt.chars().count(),
        "prompt_preview": preview(prompt),
        "state_stats": state_stats,
        "p_new_task": probabilities.map(|p| p.p_new_task),
        "p_depends_on_previous": probabilities.map(|p| p.p_depends_on_previous),
        "p_complete": probabilities.map(|p| p.p_complete),
        "p_shared_context": probabilities.map(|p| p.p_shared_context),
        "decision": decision,
        "skip_reason": skip_reason,
        "thresholds": {
            "new_task_min": thresholds.new_task_min,
            "depends_max": thresholds.depends_max,
        },
        "latency_ms": result.map(|r| r.latency_ms),
        "error": result.and_then(|r| r.error.clone()),
        "suggested": suggested,
        "jev": result.filter(|r| r.ok).map(|r| json!({
            "model": r.model,
            "gen_id": r.gen_id,
            "cost": r.cost,
        })),
        // human_label / outcome are for later manual review; left null on purpose
        "human_label": null,
        "outcome": null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn preview_collapses_and_limits() {
        let s = "a\n\nb   c\t".to_string() + &"字".repeat(300);
        let p = preview(&s);
        assert!(p.chars().count() <= PREVIEW_CHARS);
        assert!(!p.contains('\n'));
    }
}
