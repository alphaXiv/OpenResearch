//! Patterns read signals from a log an agent cannot change: each line that matches one becomes a
//! record, as if the run had printed it as an `::orx::` line.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use std::collections::BTreeMap;

const MAX_PATTERNS: usize = 16;
const MAX_REGEX: usize = 4096;
const MAX_GROUPS: usize = 64;

/// One pattern as an agent writes it: a regex, and the fields every line it matches carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    pub regex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<String>,
}

/// What a run's fold applies beyond the wire: `run-signals/<run>.config.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    pub patterns: Vec<Pattern>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Config {
    /// Names the fold this configuration makes, so a checkpoint folded under another is folded
    /// again; with no patterns it is the fold of the wire alone.
    pub fn hash(&self) -> String {
        use sha2::{Digest, Sha256};
        if self.patterns.is_empty() {
            return crate::DEFAULT_CONFIG_HASH.into();
        }
        let text = serde_json::to_string(&self.patterns).expect("patterns serialize");
        format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
    }
}

/// Why pattern `index` (from 0) cannot be used, and where in its regex, when that is known.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternError {
    pub index: usize,
    pub column: Option<usize>,
    pub reason: String,
}

struct Compiled {
    regex: regex::Regex,
    pattern: Pattern,
}

/// The patterns of a [`Config`], compiled once for a fold.
pub struct Patterns {
    hash: String,
    list: Vec<Compiled>,
}

impl std::fmt::Debug for Patterns {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Patterns({})", self.hash)
    }
}

impl PartialEq for Patterns {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

fn identifier_field(index: usize, field: &str, value: &Option<String>) -> Result<(), PatternError> {
    match value {
        Some(value) if value.is_empty() || value.len() > 128 => Err(PatternError {
            index,
            column: None,
            reason: format!("{field} must be 1 to 128 characters"),
        }),
        Some(value) if field == "row" && !crate::identifier(value) => Err(PatternError {
            index,
            column: None,
            reason: format!("row \"{value}\" is not a name: use letters, digits and _"),
        }),
        _ => Ok(()),
    }
}

impl Patterns {
    pub fn compile(config: &Config) -> Result<Self, PatternError> {
        if config.patterns.len() > MAX_PATTERNS {
            return Err(PatternError {
                index: MAX_PATTERNS,
                column: None,
                reason: format!("at most {MAX_PATTERNS} patterns"),
            });
        }
        let mut list = Vec::new();
        for (index, pattern) in config.patterns.iter().enumerate() {
            let fail = |column, reason: String| PatternError {
                index,
                column,
                reason,
            };
            if pattern.regex.len() > MAX_REGEX {
                return Err(fail(None, format!("a regex is at most {MAX_REGEX} bytes")));
            }
            let regex = regex::RegexBuilder::new(&pattern.regex)
                .size_limit(256 * 1024)
                .dfa_size_limit(256 * 1024)
                .build()
                .map_err(|error| {
                    let text = error.to_string();
                    let lines: Vec<&str> = text.lines().collect();
                    // The caret line under the regex the error echoes, indented by four.
                    let column = lines
                        .iter()
                        .find(|line| {
                            let marks = line.trim();
                            !marks.is_empty() && marks.chars().all(|c| c == '^' || c == '-')
                        })
                        .and_then(|line| line.find('^'))
                        .map(|at| at.saturating_sub(4) + 1);
                    let reason = lines
                        .iter()
                        .find_map(|line| line.strip_prefix("error: "))
                        .unwrap_or(&text);
                    fail(column, reason.to_owned())
                })?;
            if regex.captures_len() > MAX_GROUPS + 1 {
                return Err(fail(
                    None,
                    format!("a regex has at most {MAX_GROUPS} groups"),
                ));
            }
            let names: Vec<&str> = regex.capture_names().flatten().collect();
            if let Some(name) = names.iter().find(|name| !crate::identifier(name)) {
                return Err(fail(
                    None,
                    format!("group \"{name}\" is not a name: use letters, digits and _"),
                ));
            }
            if names.contains(&"name") != names.contains(&"value") {
                return Err(fail(
                    None,
                    "groups name and value go together: the log names the signal and gives its value".into(),
                ));
            }
            identifier_field(index, "phase", &pattern.phase)?;
            identifier_field(index, "unit", &pattern.unit)?;
            identifier_field(index, "row", &pattern.row)?;
            list.push(Compiled {
                regex,
                pattern: pattern.clone(),
            });
        }
        Ok(Self {
            hash: config.hash(),
            list,
        })
    }

    pub fn hash(&self) -> &str {
        &self.hash
    }

    pub fn regexes(&self) -> Vec<&str> {
        self.list.iter().map(|c| c.pattern.regex.as_str()).collect()
    }

    /// The record the first pattern matching `line` makes of it, and which pattern that is.
    pub(crate) fn record(&self, line: &str) -> Option<(usize, Map<String, Value>)> {
        self.list.iter().enumerate().find_map(|(index, compiled)| {
            let captures = compiled.regex.captures(line)?;
            let mut record = Map::new();
            let fixed = &compiled.pattern;
            for (key, value) in [
                ("phase", &fixed.phase),
                ("unit", &fixed.unit),
                ("row", &fixed.row),
            ] {
                if let Some(value) = value {
                    record.insert(key.into(), Value::from(value.as_str()));
                }
            }
            let mut measures = Map::new();
            for name in compiled.regex.capture_names().flatten() {
                let Some(text) = captures.name(name).map(|m| m.as_str()) else {
                    continue;
                };
                match name {
                    "phase" | "unit" | "row" => {
                        record.insert(name.into(), Value::from(text.trim()));
                    }
                    "step" | "of" | "t" | "spend" => {
                        record.insert(name.into(), lexeme(text));
                    }
                    "value" => {}
                    "name" => {
                        if let Some(value) = captures.name("value") {
                            measures.insert(text.trim().into(), lexeme(value.as_str()));
                        }
                    }
                    _ => {
                        measures.insert(name.into(), lexeme(text));
                    }
                }
            }
            if !measures.is_empty() {
                record.insert("m".into(), Value::Object(measures));
            }
            Some((index, record))
        })
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// A captured value: a number when it reads as one, the spelling of a non-finite one as the wire
/// writes it, else the text.
fn lexeme(text: &str) -> Value {
    let text = text.trim();
    match text.to_ascii_lowercase().as_str() {
        "nan" | "+nan" | "-nan" => return Value::from("NaN"),
        "inf" | "+inf" | "infinity" | "+infinity" => return Value::from("Infinity"),
        "-inf" | "-infinity" => return Value::from("-Infinity"),
        _ => {}
    }
    text.parse::<f64>()
        .ok()
        .and_then(Number::from_f64)
        .map_or_else(|| Value::from(text), Value::Number)
}

/// What each pattern matched in one fold, for `orx exp view check`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tally {
    /// Lines read so far.
    pub lines: u64,
    pub hits: Vec<Hit>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hit {
    pub matched: u64,
    /// The first line it matched: its number from 1, and its text.
    pub first: Option<(u64, String)>,
}
