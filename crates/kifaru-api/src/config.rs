//! Runtime configuration, read from the environment.
//!
//! Precedence: a real environment variable always wins. A `.env` file in the
//! repository root is consulted only for variables that are not already set,
//! which is what makes `cargo run` work on a fresh clone without exporting
//! anything.
//!
//! The `.env` file is parsed into a map rather than written back into the
//! process environment. Mutating the environment is process-global, is `unsafe`
//! in edition 2024 because of the data race it creates, and would leak state
//! between tests. A map has none of those problems.

use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Json,
    Pretty,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub bind: String,
    /// Development identity used when a request does not supply one.
    ///
    /// **This is not authentication and not a security control.** It exists so
    /// idempotency keys can be scoped while token verification is undecided.
    pub dev_client_id: String,
    pub log_format: LogFormat,
    pub log_level: String,
    /// Whether `serve` applies migrations at startup. A development convenience;
    /// a production deployment would run migrations as a separate step.
    pub auto_migrate: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required environment variable {0}")]
    Missing(&'static str),

    #[error("{0} must be one of: json, pretty")]
    BadLogFormat(&'static str),
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let dotenv = load_dotenv();
        Self::from_lookup(|key| std::env::var(key).ok().or_else(|| dotenv.get(key).cloned()))
    }

    /// Build a configuration from an arbitrary key lookup.
    ///
    /// Separated from [`Self::from_env`] so the parsing rules can be tested
    /// without touching the real environment.
    pub fn from_lookup(mut get: impl FnMut(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let database_url = get("DATABASE_URL")
            .filter(|v| !v.trim().is_empty())
            .ok_or(ConfigError::Missing("DATABASE_URL"))?;

        let log_format = match get("KIFARU_LOG_FORMAT").as_deref() {
            Some("json") => LogFormat::Json,
            None | Some("pretty") => LogFormat::Pretty,
            Some(_) => return Err(ConfigError::BadLogFormat("KIFARU_LOG_FORMAT")),
        };

        let bind = non_empty(get("KIFARU_BIND")).unwrap_or_else(|| "127.0.0.1:8080".to_string());
        let dev_client_id =
            non_empty(get("KIFARU_DEV_CLIENT_ID")).unwrap_or_else(|| "dev-client".to_string());
        let log_level = non_empty(get("KIFARU_LOG_LEVEL"))
            .unwrap_or_else(|| "kifaru=info,tower_http=info,info".to_string());
        let auto_migrate = non_empty(get("KIFARU_AUTO_MIGRATE")).as_deref() != Some("false");

        Ok(Self {
            database_url,
            bind,
            dev_client_id,
            log_format,
            log_level,
            auto_migrate,
        })
    }

    /// The database URL with the password replaced, for logging.
    pub fn redacted_database_url(&self) -> String {
        redact_url(&self.database_url)
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

/// Mask the password in a `scheme://user:password@host` URL.
pub fn redact_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return "<unparseable>".to_string();
    };
    let rest = &url[scheme_end + 3..];
    let Some(at) = rest.rfind('@') else {
        return url.to_string();
    };
    let credentials = &rest[..at];
    let Some(colon) = credentials.find(':') else {
        return url.to_string();
    };
    format!(
        "{}://{}:***@{}",
        &url[..scheme_end],
        &credentials[..colon],
        &rest[at + 1..]
    )
}

impl fmt::Display for LogFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LogFormat::Json => "json",
            LogFormat::Pretty => "pretty",
        })
    }
}

/// Read the nearest `.env`, searching upward from the current directory.
///
/// Returns an empty map when there is none. A malformed line is skipped rather
/// than fatal: a stray comment should not stop the server from booting.
fn load_dotenv() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(mut dir) = std::env::current_dir() else {
        return map;
    };

    loop {
        let candidate = dir.join(".env");
        if candidate.is_file() {
            if let Ok(contents) = std::fs::read_to_string(&candidate) {
                for line in contents.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let Some((key, value)) = line.split_once('=') else {
                        continue;
                    };
                    let key = key.trim();
                    if key.is_empty() {
                        continue;
                    }
                    map.insert(
                        key.to_string(),
                        value
                            .trim()
                            .trim_matches('"')
                            .trim_matches('\'')
                            .to_string(),
                    );
                }
            }
            return map;
        }
        if !dir.pop() {
            return map;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(pairs: &[(&str, &str)]) -> impl FnMut(&str) -> Option<String> + use<> {
        let owned: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key: &str| owned.get(key).cloned()
    }

    #[test]
    fn database_url_is_required() {
        let result = Config::from_lookup(|_| None);
        assert!(matches!(result, Err(ConfigError::Missing("DATABASE_URL"))));
    }

    #[test]
    fn defaults_are_applied_when_absent() {
        let cfg = Config::from_lookup(lookup(&[("DATABASE_URL", "postgres://x/y")])).unwrap();
        assert_eq!(cfg.bind, "127.0.0.1:8080");
        assert_eq!(cfg.dev_client_id, "dev-client");
        assert!(cfg.auto_migrate);
        assert_eq!(cfg.log_format, LogFormat::Pretty);
    }

    #[test]
    fn explicit_values_win() {
        let cfg = Config::from_lookup(lookup(&[
            ("DATABASE_URL", "postgres://x/y"),
            ("KIFARU_BIND", "0.0.0.0:9000"),
            ("KIFARU_LOG_FORMAT", "json"),
            ("KIFARU_AUTO_MIGRATE", "false"),
        ]))
        .unwrap();
        assert_eq!(cfg.bind, "0.0.0.0:9000");
        assert_eq!(cfg.log_format, LogFormat::Json);
        assert!(!cfg.auto_migrate);
    }

    #[test]
    fn an_unknown_log_format_is_rejected() {
        let result = Config::from_lookup(lookup(&[
            ("DATABASE_URL", "postgres://x/y"),
            ("KIFARU_LOG_FORMAT", "yaml"),
        ]));
        assert!(matches!(result, Err(ConfigError::BadLogFormat(_))));
    }

    #[test]
    fn blank_values_fall_back_to_defaults() {
        let cfg = Config::from_lookup(lookup(&[
            ("DATABASE_URL", "postgres://x/y"),
            ("KIFARU_BIND", "   "),
        ]))
        .unwrap();
        assert_eq!(cfg.bind, "127.0.0.1:8080");
    }

    #[test]
    fn redacts_the_password_only() {
        assert_eq!(
            redact_url("postgres://kifaru:hunter2@localhost:5434/kifaru"),
            "postgres://kifaru:***@localhost:5434/kifaru"
        );
    }

    #[test]
    fn redaction_survives_a_url_without_credentials() {
        assert_eq!(
            redact_url("postgres://localhost/kifaru"),
            "postgres://localhost/kifaru"
        );
    }

    #[test]
    fn redaction_does_not_leak_an_unparseable_url() {
        assert_eq!(redact_url("not a url"), "<unparseable>");
    }
}
