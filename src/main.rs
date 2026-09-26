#![forbid(unsafe_code)]

use semantic_compact::pipeline;
use std::io::{Read, Write};

// UserPromptSubmit hook entry. Reads the event JSON from stdin, runs the
// boundary judgment, and prints at most one JSON line. Every failure path
// is silent with exit code 0 so the prompt is never blocked or altered:
// exit 2 or a `decision` field would erase the user's prompt, and plain
// text stdout would be injected into Claude's context.
fn main() {
    // Panics must neither print noise nor produce a non-zero exit code.
    let debug = std::env::var("SC_DEBUG")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    if !debug {
        std::panic::set_hook(Box::new(|_| {}));
    }

    let mut input_text = String::new();
    let _ = std::io::stdin().read_to_string(&mut input_text);
    let input: serde_json::Value =
        serde_json::from_str(&input_text).unwrap_or(serde_json::Value::Null);

    let env_getter = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let post = semantic_compact::judge::http_post;
    let opts = pipeline::PipelineOpts {
        env_getter: &env_getter,
        data_dir: None,
        plugin_root: None,
        post: &post,
    };

    let result = pipeline::run(&input, &opts);

    if let Some(message) = result.system_message {
        // Block mode returns the prompt to the user (reason shown, prompt
        // not processed); remind mode shows a warning while it proceeds.
        // Every failure path reaches here with neither set.
        let payload = if result.block {
            serde_json::json!({ "decision": "block", "reason": message, "suppressOriginalPrompt": true })
        } else {
            serde_json::json!({ "systemMessage": message })
        };
        if let Ok(json) = serde_json::to_string(&payload) {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            let _ = lock.write_all(json.as_bytes());
            let _ = lock.write_all(b"\n");
            let _ = lock.flush();
        }
    }

    std::process::exit(0);
}
