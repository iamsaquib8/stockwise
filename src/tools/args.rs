//! Tool arguments checked against the tool's [`Param`] list. Models make
//! mistakes, so every message says which argument is wrong and what it
//! accepts.

use super::{Kind, Param, Tool};
use crate::market::Market;
use anyhow::{Result, bail};
use serde_json::{Map, Value};

/// Checked arguments of one call.
#[derive(Debug, Clone, Default)]
pub struct Args {
    values: Map<String, Value>,
}

impl Args {
    /// Check `raw` (an object, or null / `{}` for none) against `tool`.
    /// Unknown keys are refused: they are usually a misspelled parameter
    /// whose value would otherwise be silently ignored.
    pub fn parse(tool: &Tool, raw: &Value) -> Result<Self> {
        let values = match raw {
            Value::Null => Map::new(),
            Value::Object(m) => m.clone(),
            // Some models send the arguments as a JSON string.
            Value::String(s) if s.trim().is_empty() => Map::new(),
            Value::String(s) => match serde_json::from_str::<Value>(s) {
                Ok(Value::Object(m)) => m,
                _ => bail!("Arguments for {} must be a JSON object", tool.name),
            },
            _ => bail!("Arguments for {} must be a JSON object", tool.name),
        };
        for key in values.keys() {
            if !tool.params.iter().any(|p| p.name == key) {
                let known: Vec<&str> = tool.params.iter().map(|p| p.name).collect();
                bail!(
                    "Unknown argument '{}' for {} (accepted: {})",
                    key,
                    tool.name,
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                );
            }
        }
        let mut out = Map::new();
        for p in tool.params {
            match values.get(p.name) {
                None | Some(Value::Null) => {
                    if p.required {
                        bail!("Missing required argument '{}' for {}", p.name, tool.name);
                    }
                }
                Some(v) => {
                    out.insert(p.name.to_string(), check(p, v)?);
                }
            }
        }
        Ok(Self { values: out })
    }

    pub fn str(&self, name: &str) -> Option<&str> {
        self.values.get(name).and_then(Value::as_str)
    }

    /// A required string (checked present by [`Args::parse`]).
    pub fn need_str(&self, name: &str) -> Result<&str> {
        match self.str(name) {
            Some(s) => Ok(s),
            None => bail!("Missing required argument '{}'", name),
        }
    }

    pub fn list(&self, name: &str) -> Vec<String> {
        self.values
            .get(name)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn f64(&self, name: &str) -> Option<f64> {
        self.values.get(name).and_then(Value::as_f64)
    }

    pub fn need_f64(&self, name: &str) -> Result<f64> {
        match self.f64(name) {
            Some(v) => Ok(v),
            None => bail!("Missing required argument '{}'", name),
        }
    }

    pub fn i64(&self, name: &str) -> Option<i64> {
        self.values.get(name).and_then(Value::as_i64)
    }

    pub fn usize_or(&self, name: &str, default: usize) -> usize {
        self.i64(name).map_or(default, |v| v.max(0) as usize)
    }

    pub fn bool(&self, name: &str) -> bool {
        self.values
            .get(name)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// The `market` argument, or `default` (the user's market).
    pub fn market(&self, default: Market) -> Market {
        use clap::ValueEnum;
        self.str("market")
            .and_then(|m| Market::from_str(m, true).ok())
            .unwrap_or(default)
    }
}

fn check(p: &Param, v: &Value) -> Result<Value> {
    let name = p.name;
    Ok(match p.kind {
        Kind::Str => match v {
            Value::String(s) if !s.trim().is_empty() && s.len() <= 200 => {
                Value::String(s.trim().to_string())
            }
            Value::String(_) => bail!("'{}' must be a non-empty string", name),
            // A number where a string was asked for (a scheme code): accept it.
            Value::Number(n) => Value::String(n.to_string()),
            _ => bail!("'{}' must be a string", name),
        },
        Kind::StrList { max } => {
            let items: Vec<Value> = match v {
                Value::Array(a) => a.clone(),
                // "TCS, INFY" or "TCS" for a list.
                Value::String(s) => s
                    .split([',', ' '])
                    .filter(|x| !x.is_empty())
                    .map(|x| Value::String(x.to_string()))
                    .collect(),
                _ => bail!("'{}' must be a list of strings", name),
            };
            if items.is_empty() {
                bail!("'{}' needs at least one item", name);
            }
            if items.len() > max {
                bail!("'{}' takes at most {} items", name, max);
            }
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(s) if !s.trim().is_empty() && s.len() <= 40 => {
                        out.push(Value::String(s.trim().to_string()))
                    }
                    _ => bail!("'{}' must hold short non-empty strings", name),
                }
            }
            Value::Array(out)
        }
        Kind::Number { min, max } => {
            let n = number(v).ok_or_else(|| anyhow::anyhow!("'{}' must be a number", name))?;
            if !n.is_finite() || n < min || n > max {
                bail!("'{}' must be between {} and {}", name, min, max);
            }
            Value::from(n)
        }
        Kind::Integer { min, max } => {
            let n = number(v).ok_or_else(|| anyhow::anyhow!("'{}' must be an integer", name))?;
            if n.fract() != 0.0 {
                bail!("'{}' must be a whole number", name);
            }
            let n = n as i64;
            if n < min || n > max {
                bail!("'{}' must be between {} and {}", name, min, max);
            }
            Value::from(n)
        }
        Kind::Bool => match v {
            Value::Bool(b) => Value::Bool(*b),
            Value::String(s) if s.eq_ignore_ascii_case("true") => Value::Bool(true),
            Value::String(s) if s.eq_ignore_ascii_case("false") => Value::Bool(false),
            _ => bail!("'{}' must be true or false", name),
        },
        Kind::Enum(values) => {
            let s = v
                .as_str()
                .map(|s| s.trim().to_ascii_lowercase())
                .ok_or_else(|| anyhow::anyhow!("'{}' must be a string", name))?;
            match values.iter().find(|x| x.eq_ignore_ascii_case(&s)) {
                Some(x) => Value::String(x.to_string()),
                None => bail!("'{}' must be one of: {}", name, values.join(", ")),
            }
        }
        Kind::Market => {
            use clap::ValueEnum;
            let s = v
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("'{}' must be a string", name))?;
            if Market::from_str(s.trim(), true).is_err() {
                bail!(
                    "Unknown market '{}' (use one of: {})",
                    s,
                    super::market_codes().join(", ")
                );
            }
            Value::String(s.trim().to_ascii_lowercase())
        }
    })
}

/// A JSON number, or a numeric string (small models often quote numbers).
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().replace(',', "").parse().ok(),
        _ => None,
    }
}
