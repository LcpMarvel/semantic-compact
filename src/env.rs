use std::collections::HashMap;
use std::path::Path;

// Minimal KEY=VALUE parser: comments (#), blank lines, optional `export `,
// single- or double-quoted values. Good enough for local env files.
pub fn parse_dotenv(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let rest = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let Some((key, value)) = rest.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            continue;
        }
        if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let mut value = value.trim();
        if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            value = &value[1..value.len() - 1];
        }
        out.insert(key.to_string(), value.to_string());
    }
    out
}

fn read_env_file(path: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(path)
        .map(|t| parse_dotenv(&t))
        .unwrap_or_default()
}

fn lookup(vars: &HashMap<String, String>) -> Option<&str> {
    vars.get("JEV_API_KEY")
        .or_else(|| vars.get("OPENROUTER_API_KEY"))
        .or_else(|| vars.get("TYPESAFE_API_KEY"))
        .map(|s| s.as_str())
}

// Resolution order (high to low): the plugin option set via /plugin
// configure (sensitive value from secure storage), other process
// environment variables, the api_key field of the JSON config, the
// user-level env file (~/.config/semantic-compact/env), the project .env
// next to the user's cwd, and finally an .env inside the plugin directory.
// The key itself is never logged or persisted.
pub fn resolve_api_key(
    env_getter: &dyn Fn(&str) -> Option<String>,
    json_key: Option<&str>,
    cwd: Option<&Path>,
    plugin_root: &Path,
) -> Option<String> {
    if let Some(k) = env_getter("CLAUDE_PLUGIN_OPTION_JEV_API_KEY").filter(|v| !v.is_empty()) {
        return Some(k);
    }
    if let Some(k) = env_getter("JEV_API_KEY").filter(|v| !v.is_empty()) {
        return Some(k);
    }
    if let Some(k) = env_getter("OPENROUTER_API_KEY").filter(|v| !v.is_empty()) {
        return Some(k);
    }
    if let Some(k) = env_getter("TYPESAFE_API_KEY").filter(|v| !v.is_empty()) {
        return Some(k);
    }
    if let Some(k) = json_key.filter(|v| !v.is_empty()) {
        return Some(k.to_string());
    }

    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(user_dir) = crate::config::user_config_home(env_getter) {
        candidates.push(user_dir.join("env"));
    }
    if let Some(cwd) = cwd {
        candidates.push(cwd.join(".env"));
    }
    if plugin_root != cwd.unwrap_or(Path::new("")) {
        candidates.push(plugin_root.join(".env"));
    }

    for file in candidates {
        if let Some(k) = lookup(&read_env_file(&file)) {
            if !k.is_empty() {
                return Some(k.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_dotenv() {
        let vars = parse_dotenv("# comment\nA=1\nexport B=\"two\"\nC='three'\n\nBROKEN\n");
        assert_eq!(vars.get("A").map(String::as_str), Some("1"));
        assert_eq!(vars.get("B").map(String::as_str), Some("two"));
        assert_eq!(vars.get("C").map(String::as_str), Some("three"));
        assert!(!vars.contains_key("BROKEN"));
    }
}
