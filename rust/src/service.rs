use anyhow::{Result, bail};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Definition {
    pub name: String,
    pub command: String,
    pub location: String,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub status: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pid: i32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub log_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit_name: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_started_at: String,
}

fn is_zero(value: &i32) -> bool {
    *value == 0
}

pub fn validate_name(name: &str) -> Result<()> {
    let bytes = name.as_bytes();
    if bytes.len() < 2 || bytes.len() > 63 {
        bail!("service name must be 2-63 chars and use only letters, digits, dash, or underscore");
    }

    let first = bytes[0] as char;
    if !first.is_ascii_alphanumeric() {
        bail!("service name must be 2-63 chars and use only letters, digits, dash, or underscore");
    }

    if bytes[1..]
        .iter()
        .any(|b| !((*b as char).is_ascii_alphanumeric() || *b == b'-' || *b == b'_'))
    {
        bail!("service name must be 2-63 chars and use only letters, digits, dash, or underscore");
    }

    Ok(())
}

pub fn normalize_location(location: &str) -> Result<String> {
    let raw = if location.is_empty() { "." } else { location };
    let path = Path::new(raw);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    Ok(path_to_string(absolute))
}

pub fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn path_to_string(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::validate_name;

    #[test]
    fn validate_name_accepts_expected_values() {
        for name in ["api", "pkg_lat", "svc-01"] {
            validate_name(name).unwrap();
        }
    }

    #[test]
    fn validate_name_rejects_invalid_values() {
        for name in ["", "x", "bad name", "bad/name"] {
            assert!(validate_name(name).is_err(), "{name} should be invalid");
        }
    }
}
