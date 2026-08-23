//! `ConfigSource` — spec §4.2's provenance enum — and the log-line
//! formatting that makes resolution explainable "out loud" (ADR 0007).

/// Where one resolved parameter's value came from. The exact enum from
/// spec §4.2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    Cli,
    EnvVar(String),
    ExperimentFile(String),
    ModeProfile(String),
    TopologyProfile(String),
    BuiltinFallback,
}

impl ConfigSource {
    fn tag(&self) -> &'static str {
        match self {
            ConfigSource::Cli => "cli",
            ConfigSource::EnvVar(_) => "env_var",
            ConfigSource::ExperimentFile(_) => "experiment_file",
            ConfigSource::ModeProfile(_) => "mode_profile",
            ConfigSource::TopologyProfile(_) => "topology_profile",
            ConfigSource::BuiltinFallback => "builtin_fallback",
        }
    }

    /// The extra JSON key/value naming *which* env var, file, or profile —
    /// e.g. `("profile", "cross_device")` — mirrored in spec §4.2's
    /// example: `"source":"topology_profile","profile":"cross_device"`.
    fn json_qualifier(&self) -> Option<(&'static str, &str)> {
        match self {
            ConfigSource::Cli | ConfigSource::BuiltinFallback => None,
            ConfigSource::EnvVar(name) => Some(("var", name.as_str())),
            ConfigSource::ExperimentFile(path) => Some(("file", path.as_str())),
            ConfigSource::ModeProfile(name) => Some(("profile", name.as_str())),
            ConfigSource::TopologyProfile(name) => Some(("profile", name.as_str())),
        }
    }

    /// The phrase shown in the text format's parenthetical, e.g.
    /// `topology profile "cross_device"` or `built-in fallback` — spec
    /// §4.2's `(source: ...)` examples.
    fn text_phrase(&self) -> String {
        match self {
            ConfigSource::Cli => "cli".to_string(),
            ConfigSource::EnvVar(name) => format!("env var {name}"),
            ConfigSource::ExperimentFile(path) => format!("experiment file {path:?}"),
            ConfigSource::ModeProfile(name) => format!("mode profile {name:?}"),
            ConfigSource::TopologyProfile(name) => format!("topology profile {name:?}"),
            ConfigSource::BuiltinFallback => "built-in fallback".to_string(),
        }
    }
}

use crate::LogFormat;

/// A resolved value ready to print: numbers/bools render bare in both
/// formats, text renders bare in text format but quoted in JSON.
pub(crate) enum LoggedValue<'a> {
    Number(String),
    Text(&'a str),
}

pub(crate) fn log_line(
    format: LogFormat,
    param: &str,
    value: LoggedValue<'_>,
    source: &ConfigSource,
) -> String {
    match format {
        LogFormat::Json => {
            let value_json = match value {
                LoggedValue::Number(s) => s,
                LoggedValue::Text(s) => format!("{s:?}"),
            };
            let mut line = format!(
                "{{\"param\":\"{param}\",\"value\":{value_json},\"source\":\"{}\"",
                source.tag()
            );
            if let Some((key, val)) = source.json_qualifier() {
                line.push_str(&format!(",\"{key}\":{val:?}"));
            }
            line.push('}');
            line
        }
        LogFormat::Text => {
            let value_text = match value {
                LoggedValue::Number(s) => s,
                LoggedValue::Text(s) => s.to_string(),
            };
            format!(
                "[config] {param:<24} = {value_text}  (source: {})",
                source.text_phrase()
            )
        }
    }
}
