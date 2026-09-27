use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::Path;

// The transcript JSONL layout is internal to Claude Code and may change
// between versions, so every step here is defensive: unknown lines are
// skipped and errors surface as ok=false, which the caller treats as "skip".

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub text: String,
    pub seq: usize, // line position; transcripts are append-only
}

#[derive(Debug, Clone, Default)]
pub struct ParsedTranscript {
    pub ok: bool,
    pub error: Option<String>,
    pub total_user_prompts: usize,
    pub user_prompts: Vec<Entry>,       // oldest first, capped
    pub older_prompts: Vec<Entry>,      // up to four samples before the recent window
    pub assistant_outcomes: Vec<Entry>, // oldest first, capped
    pub files: Vec<String>,             // most recent first, capped
}

pub struct ParseOpts {
    pub max_user_prompts: usize,
    pub max_assistant_outcomes: usize,
    pub max_files: usize,
}

impl Default for ParseOpts {
    fn default() -> Self {
        Self {
            max_user_prompts: 8,
            max_assistant_outcomes: 3,
            max_files: 15,
        }
    }
}

const COMMAND_MARKERS: [&str; 4] = [
    "<command-name>",
    "<local-command-stdout>",
    "Caveat:",
    "<task-notification>",
];

pub(crate) fn strip_pasted_tags(text: &str) -> String {
    let mut clean = text.to_string();
    for marker in ["<pasted_content", "</pasted_content"] {
        while let Some(start) = clean.find(marker) {
            let Some(end) = clean[start..].find('>') else {
                break;
            };
            clean.replace_range(start..start + end + 1, "");
        }
    }
    clean.trim().to_string()
}

pub fn is_real_user_prompt(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || strip_pasted_tags(t).is_empty() {
        return false;
    }
    if t.starts_with('/') {
        return false; // slash command
    }
    if t.starts_with("[Request interrupted by") {
        return false;
    }
    !COMMAND_MARKERS.iter().any(|m| t.contains(m))
}

fn user_text_from_content(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            // tool results arrive as user-role messages; they are not prompts
            if blocks
                .iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            {
                return None;
            }
            let text: Vec<&str> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            if text.is_empty() {
                None
            } else {
                Some(text.join("\n"))
            }
        }
        _ => None,
    }
}

fn file_args_from_tool_use(block: &Value) -> Vec<&str> {
    let mut paths = Vec::new();
    if let Some(input) = block.get("input").and_then(Value::as_object) {
        for key in ["file_path", "path", "notebook_path"] {
            if let Some(v) = input.get(key).and_then(Value::as_str) {
                if !v.trim().is_empty() {
                    paths.push(v.trim());
                }
            }
        }
    }
    paths
}

pub fn parse_transcript(path: &Path, opts: &ParseOpts) -> ParsedTranscript {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(error) => {
            return ParsedTranscript {
                ok: false,
                error: Some(
                    match error.kind() {
                        std::io::ErrorKind::NotFound => "transcript_not_found",
                        std::io::ErrorKind::PermissionDenied => "transcript_permission_denied",
                        _ => "transcript_open_error",
                    }
                    .into(),
                ),
                ..Default::default()
            }
        }
    };

    let mut result = ParsedTranscript {
        ok: true,
        ..Default::default()
    };
    let mut files: Vec<String> = Vec::new(); // most recent first

    let reader = BufReader::new(file);
    for (idx, line) in reader.lines().enumerate() {
        let Ok(line) = line else {
            result.ok = false;
            result.error = Some("transcript_read_error".into());
            return result;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(obj) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(obj) = obj.as_object() else { continue };

        if obj.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if obj.get("type").and_then(Value::as_str) == Some("system")
            && obj.get("subtype").and_then(Value::as_str) == Some("compact_boundary")
        {
            result = ParsedTranscript {
                ok: true,
                ..Default::default()
            };
            files.clear();
            continue;
        }

        if obj.get("isMeta").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if obj.get("isCompactSummary").and_then(Value::as_bool) == Some(true) {
            continue;
        }

        let kind = obj.get("type").and_then(Value::as_str);
        let msg = obj.get("message");

        if kind == Some("user") {
            if let Some(content) = msg.and_then(|m| m.get("content")) {
                if let Some(text) = user_text_from_content(content) {
                    if is_real_user_prompt(&text) {
                        result.total_user_prompts += 1;
                        result.user_prompts.push(Entry {
                            text: strip_pasted_tags(&text).trim().to_string(),
                            seq: idx,
                        });
                    }
                }
            }
        } else if kind == Some("assistant") {
            if let Some(blocks) = msg.and_then(|m| m.get("content")).and_then(Value::as_array) {
                let text: Vec<&str> = blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect();
                let joined = text.join("\n").trim().to_string();
                if !joined.is_empty() {
                    result.assistant_outcomes.push(Entry {
                        text: joined,
                        seq: idx,
                    });
                    if result.assistant_outcomes.len() > opts.max_assistant_outcomes {
                        result.assistant_outcomes.remove(0);
                    }
                }
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                        for p in file_args_from_tool_use(block) {
                            if let Some(pos) = files.iter().position(|f| f == p) {
                                files.remove(pos);
                            }
                            files.insert(0, p.to_string());
                        }
                    }
                }
            }
        }
        // unknown line types (mode, attachment, queue-operation, ...) are skipped
    }

    let older_count = result
        .user_prompts
        .len()
        .saturating_sub(opts.max_user_prompts);
    let recent = result.user_prompts.split_off(older_count);
    // ponytail: four evenly spaced prompt samples can miss intermediate details;
    // increase the sample only if real-session calibration shows missed drift.
    let sample_count = older_count.min(4);
    result.older_prompts = (0..sample_count)
        .map(|i| {
            result.user_prompts
                [i * older_count.saturating_sub(1) / sample_count.saturating_sub(1).max(1)]
            .clone()
        })
        .collect();
    result.user_prompts = recent;
    result.files = files.into_iter().take(opts.max_files).collect();
    result
}
