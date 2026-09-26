use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub thresholds: Thresholds,
    pub jev: JevConfig,
    pub cooldown: Cooldown,
    pub history: History,
    pub skip: Skip,
    pub reminder: Reminder,
    pub debug_logging: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    pub new_task_min: f64,
    pub depends_max: f64,
    pub shared_context_min: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct JevConfig {
    pub url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub auth_header: String,
    pub auth_scheme: String,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Cooldown {
    pub prompts: u64,
    pub seconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub max_user_prompts: usize,
    pub max_assistant_outcomes: usize,
    pub max_files: usize,
    pub user_prompt_chars: usize,
    pub assistant_outcome_chars: usize,
    pub new_prompt_chars: usize,
    pub state_char_limit: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Skip {
    pub min_prior_prompts: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Reminder {
    pub message: String,
    pub clear_message: String,
    pub setup_message: String,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            new_task_min: 0.90,
            depends_max: 0.20,
            shared_context_min: 0.50,
        }
    }
}

impl Default for JevConfig {
    fn default() -> Self {
        Self {
            url: "https://openrouter.ai/api/alpha/decisions".to_string(),
            model: "~typesafe/jev-latest".to_string(),
            api_key: None,
            auth_header: "Authorization".to_string(),
            auth_scheme: "Bearer".to_string(),
            timeout_ms: 10_000,
        }
    }
}

impl Default for Cooldown {
    fn default() -> Self {
        Self {
            prompts: 8,
            seconds: 600,
        }
    }
}

impl Default for History {
    fn default() -> Self {
        Self {
            max_user_prompts: 8,
            max_assistant_outcomes: 3,
            max_files: 15,
            user_prompt_chars: 1500,
            assistant_outcome_chars: 500,
            new_prompt_chars: 3000,
            state_char_limit: 10_000,
        }
    }
}

impl Default for Skip {
    fn default() -> Self {
        Self {
            min_prior_prompts: 2,
        }
    }
}

impl Default for Reminder {
    fn default() -> Self {
        Self {
            message: "Semantic Compact: this looks like a new task.\n\
                      The previous task may no longer need to stay in active context.\n\
                      Consider running /compact before continuing. \
                      (suggestion only — nothing was compacted)"
                .to_string(),
            clear_message: "Semantic Compact: this looks like a new, unrelated task.\n\
                            The previous context is unlikely to be useful for it.\n\
                            Consider /clear for a fresh start, or /compact to keep a summary. \
                            (suggestion only — nothing was cleared)"
                .to_string(),
            setup_message: "Semantic Compact: no API key found — the plugin is inactive.\n\
                            Set one via `claude plugin configure`, or:\n\
                            mkdir -p ~/.config/semantic-compact && printf 'JEV_API_KEY=sk-or-...\\n' >> ~/.config/semantic-compact/env\n\
                            (this notice shows once per session)"
                .to_string(),
        }
    }
}

fn is_object(v: &Value) -> bool {
    v.is_object()
}

// Deep-merge `over` into `base`; nested objects merge, anything else replaces.
fn deep_merge(base: &mut Value, over: &Value) {
    if is_object(base) && is_object(over) {
        let over_map = over.as_object().expect("checked object");
        for (k, v) in over_map {
            let slot = base
                .as_object_mut()
                .expect("checked object")
                .entry(k.clone())
                .or_insert(Value::Null);
            deep_merge(slot, v);
        }
    } else {
        *base = over.clone();
    }
}

fn read_json_file(path: &PathBuf) -> Option<Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

pub fn user_config_home(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(xdg) = env("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("semantic-compact"));
        }
    }
    env("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".config").join("semantic-compact"))
}

pub const ENV_OVERRIDES: &[(&str, &str)] = &[
    ("SC_NEW_TASK_MIN", "thresholds.new_task_min"),
    ("SC_DEPENDS_MAX", "thresholds.depends_max"),
    ("SC_SHARED_CONTEXT_MIN", "thresholds.shared_context_min"),
    ("JEV_DECISIONS_URL", "jev.url"),
    ("JEV_MODEL", "jev.model"),
    ("JEV_AUTH_HEADER", "jev.auth_header"),
    ("JEV_AUTH_SCHEME", "jev.auth_scheme"),
    ("JEV_TIMEOUT_MS", "jev.timeout_ms"),
    ("SC_COOLDOWN_PROMPTS", "cooldown.prompts"),
    ("SC_COOLDOWN_SECONDS", "cooldown.seconds"),
    ("SC_MAX_PROMPTS", "history.max_user_prompts"),
    ("SC_MIN_PRIOR_PROMPTS", "skip.min_prior_prompts"),
    ("SC_DEBUG", "debug_logging"),
];

fn parse_bool(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

// Layers (low to high): built-in defaults, plugin config.json,
// user config.json, then environment variables.
pub fn load_config(plugin_root: &Path, env_getter: &dyn Fn(&str) -> Option<String>) -> Config {
    let mut merged = serde_json::to_value(Config::default()).unwrap_or_else(|_| json!({}));

    let user_dir = user_config_home(env_getter);
    let layers = [
        Some(plugin_root.join("config.json")),
        user_dir.map(|d| d.join("config.json")),
    ];
    for layer in layers.into_iter().flatten() {
        if let Some(over) = read_json_file(&layer) {
            deep_merge(&mut merged, &over);
        }
    }

    for (name, path) in ENV_OVERRIDES {
        let Some(raw) = env_getter(name).filter(|v| !v.is_empty()) else {
            continue;
        };
        let leaf = if *name == "SC_DEBUG" {
            Value::Bool(parse_bool(&raw))
        } else if let Ok(n) = raw.parse::<u64>() {
            Value::Number(n.into())
        } else if let Ok(f) = raw.parse::<f64>() {
            serde_json::Number::from_f64(f)
                .map(Value::Number)
                .unwrap_or(Value::Null)
        } else {
            Value::String(raw)
        };
        let mut cur = leaf;
        for part in path.split('.').rev() {
            let mut m = serde_json::Map::new();
            m.insert(part.to_string(), cur);
            cur = Value::Object(m);
        }
        deep_merge(&mut merged, &cur);
    }

    serde_json::from_value(merged).unwrap_or_default()
}

pub fn resolve_plugin_root(env_getter: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(root) = env_getter("CLAUDE_PLUGIN_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root);
        }
    }
    // binary lives in <plugin root>/bin/ or <plugin root>/target/release/
    env::current_exe()
        .ok()
        .and_then(|p| {
            p.ancestors().skip(1).find_map(|a| {
                let candidate = a.join(".claude-plugin");
                if candidate.join("plugin.json").is_file() {
                    Some(a.to_path_buf())
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

// SC_DATA_DIR env -> CLAUDE_PLUGIN_DATA (installed plugins) -> <plugin root>/data
pub fn resolve_data_dir(
    plugin_root: &Path,
    env_getter: &dyn Fn(&str) -> Option<String>,
) -> PathBuf {
    if let Some(d) = env_getter("SC_DATA_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    if let Some(d) = env_getter("CLAUDE_PLUGIN_DATA") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    plugin_root.join("data")
}

pub fn process_env_getter(name: &str) -> Option<String> {
    env::var(name).ok()
}
