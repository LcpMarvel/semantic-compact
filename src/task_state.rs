use crate::config::History;
use crate::transcript::{strip_pasted_tags, ParsedTranscript};

pub struct TaskState {
    pub state: String,
    pub prior_prompt_count: usize,
    pub stats: serde_json::Value,
}

// UTF-8 safe truncation on character boundaries.
pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let prefix: String = s.chars().take(n.saturating_sub(1)).collect();
    format!("{}…", prefix)
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// Compact, sectioned summary of what the session has been about, ending with
// the prompt that was just submitted. Kept small on purpose: the judge only
// needs the shape of the work, not the full history.
pub fn build_task_state(
    parsed: &ParsedTranscript,
    new_prompt: &str,
    history: &History,
) -> TaskState {
    // The transcript may already contain the prompt that triggered this hook.
    let mut prior_count = parsed.total_user_prompts;
    let mut prior_prompts: Vec<(usize, &str)> = parsed
        .user_prompts
        .iter()
        .map(|e| (e.seq, e.text.as_str()))
        .collect();
    if let Some((_, last)) = prior_prompts.last() {
        if *last == strip_pasted_tags(new_prompt).trim() {
            prior_prompts.pop();
            prior_count = prior_count.saturating_sub(1);
        }
    }

    let mut turns: Vec<(usize, &str, String)> = Vec::new();
    for (seq, text) in &prior_prompts {
        turns.push((*seq, "User", truncate(text, history.user_prompt_chars)));
    }
    for e in &parsed.assistant_outcomes {
        turns.push((
            e.seq,
            "Assistant",
            truncate(&e.text, history.assistant_outcome_chars),
        ));
    }
    turns.sort_by_key(|(seq, _, _)| *seq);

    let clean_prompt = truncate(
        collapse_ws(&strip_pasted_tags(new_prompt)).as_str(),
        history.new_prompt_chars,
    );

    let render = |turns: &[(usize, &str, String)]| -> String {
        let mut parts: Vec<String> = Vec::new();
        if !turns.is_empty() {
            parts.push("== Recent conversation (oldest first) ==".to_string());
            for (_, role, text) in turns {
                parts.push(format!("{}: {}", role, collapse_ws(text)));
            }
        }
        if !parsed.files.is_empty() {
            parts.push("== Files recently touched ==".to_string());
            parts.push(parsed.files.join(", "));
        }
        parts.push("== Newest user prompt ==".to_string());
        parts.push(if clean_prompt.is_empty() {
            "(empty)".to_string()
        } else {
            clean_prompt.clone()
        });
        parts.join("\n\n")
    };

    // If the whole state is too long, drop the oldest conversation turns
    // first; the newest prompt and the file list are always kept.
    let mut kept: Vec<(usize, &str, String)> = turns;
    let mut state = render(&kept);
    while state.len() > history.state_char_limit && kept.len() > 1 {
        kept.remove(0);
        state = render(&kept);
    }

    let stats = serde_json::json!({
        "user_prompts": prior_count,
        "assistant_outcomes": parsed.assistant_outcomes.len(),
        "files": parsed.files.len(),
        "state_chars": state.len(),
    });

    TaskState {
        state,
        prior_prompt_count: prior_count,
        stats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_char_boundaries() {
        let s = "登录先到这里现在开始".repeat(10);
        let t = truncate(&s, 10);
        assert!(t.chars().count() <= 10);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn strips_pasted_tag_lines() {
        let s = "<pasted_content id=\"x\">\nreal text\n</pasted_content id=\"x\">";
        assert_eq!(strip_pasted_tags(s), "real text");
    }
}
