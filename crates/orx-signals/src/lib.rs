use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, fmt, sync::Arc};

mod patterns;
pub use patterns::{Config, Hit, Pattern, PatternError, Patterns, Tally};

pub const MAX_FRAME: usize = 64 * 1024;
pub const MAX_SNAPSHOT: usize = 16 * 1024 * 1024;
const MAX_LAST_LINE: usize = 200;
pub const DEFAULT_CONFIG_HASH: &str =
    "sha256:e0a0979e1bb38721b7d263d196d05600e66e5eccadbd3168aebff4073b121a22";
pub type Signals = BTreeMap<String, Value>;
pub type Point = (f64, Option<f64>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub ident: String,
    pub offset: u64,
    pub frame_start: u64,
    pub pending_base64: String,
    pub dropping: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    pub ranges: Vec<(u64, u64)>,
    pub complete: bool,
    pub reasons: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Coverage {
    pub fn add_reason(&mut self, reason: &str) {
        if !self.reasons.iter().any(|r| r == reason) {
            self.reasons.push(reason.into());
            self.reasons.sort();
        }
        self.complete = false;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Diagnostics {
    pub counts: BTreeMap<String, u64>,
    pub examples: Vec<Diagnostic>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub from: u64,
    pub to: u64,
    pub text: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Phase {
    pub name: String,
    pub occurrence: usize,
    pub of: Option<f64>,
    pub unit: Option<String>,
    pub done: Option<f64>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub timing_source: String,
    pub source: String,
    pub last_point: Option<(f64, f64)>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timing {
    pub progress_points: Vec<(usize, f64, f64)>,
    /// `[occurrence, t, at]`: the last report that advanced progress, placed when a scan reached
    /// the end of the log at `at`. It is the only place a report's `t` meets a clock.
    pub anchor: Option<(usize, f64, i64)>,
    /// `[occurrence, t]`: the last report that advanced progress with a `t`, placed or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_advance: Option<(usize, f64)>,
    pub last_log_at: Option<i64>,
    pub origin_ms: Option<i64>,
    pub payload_t: Option<f64>,
    /// When the payload started, as the host recorded it: the first phase's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_ms: Option<i64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Detectors {
    pub candidate: Option<String>,
    pub confirmations: u32,
    pub selected: Option<String>,
    pub previous: Option<(f64, Option<f64>)>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub run_id: String,
    pub generation: String,
    pub rev: u64,
    pub config_hash: String,
    pub cursor: Cursor,
    pub coverage: Coverage,
    pub latest: Signals,
    pub series: BTreeMap<String, Vec<Point>>,
    pub rows: BTreeMap<String, Vec<Signals>>,
    #[serde(default)]
    pub row_order: Vec<String>,
    #[serde(default)]
    pub row_columns: BTreeMap<String, Vec<String>>,
    pub groups: BTreeMap<String, Value>,
    pub cells: BTreeMap<String, Value>,
    pub counts: BTreeMap<String, u64>,
    pub phases: Vec<Phase>,
    pub timing: Timing,
    pub reducers: BTreeMap<String, Value>,
    pub detectors: Detectors,
    pub diagnostics: Diagnostics,
    pub updated_at: i64,
    #[serde(default)]
    pub signal_order: Vec<String>,
    #[serde(default)]
    pub omitted: BTreeMap<String, u64>,
    #[serde(default)]
    pub series_rev: u64,
    #[serde(default)]
    pub series_widths: BTreeMap<String, f64>,
    #[serde(default)]
    pub spend: Option<f64>,
    #[serde(default)]
    pub coordinate_report: Signals,
    #[serde(default)]
    pub last_canonical: Option<Value>,
    /// The newest log line that says something since the current stage began, past its logging
    /// prefix: what a run nobody instrumented, or a stage with no step yet, is doing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_line: String,
    /// The stages the run declared it goes through, newest declaration wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<Vec<Stage>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
    /// The patterns lines are read with; set by whoever folds, from the run's configuration.
    #[serde(skip)]
    pub patterns: Option<Arc<Patterns>>,
    /// Counts what each pattern matched, when set.
    #[serde(skip)]
    pub tally: Option<Tally>,
}

/// The estimator and retained reports use the same slice grid.
pub fn slice_count(total: f64) -> usize {
    if !total.is_finite() || total <= 0.0 {
        return 0;
    }
    total.ceil().min(40.) as usize
}

/// Progress at the start of slice `k` of `total`, shared bit for bit with the estimator.
pub fn slice_boundary(total: f64, k: usize) -> f64 {
    k as f64 * (total / slice_count(total) as f64)
}

/// One stage of a declared plan: `{"phase", "of"?, "unit"?}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

pub const MAX_STAGES: usize = 64;

/// Reads a plan: the stages a run goes through, in order. A stage that runs again is listed again
/// where it recurs, never twice in a row, since a log cannot tell two such occurrences apart.
/// `strict` refuses keys this orx does not know, as an agent's file is; the wire ignores them.
pub fn plan(value: &Value, strict: bool) -> Result<Vec<Stage>, String> {
    let list = value
        .as_array()
        .ok_or("a plan is a list of stages, each {\"phase\", \"of\", \"unit\"}")?;
    if list.len() > MAX_STAGES {
        return Err(format!(
            "a plan lists at most {MAX_STAGES} stages, not {}",
            list.len()
        ));
    }
    let mut stages: Vec<Stage> = Vec::with_capacity(list.len());
    for (index, stage) in list.iter().enumerate() {
        let n = index + 1;
        let fields = stage
            .as_object()
            .ok_or_else(|| format!("stage {n} is not an object like {{\"phase\": \"train\"}}"))?;
        if strict {
            if let Some(key) = fields
                .keys()
                .find(|key| !matches!(key.as_str(), "phase" | "of" | "unit"))
            {
                return Err(format!(
                    "stage {n} has an unknown key \"{key}\"; a stage takes phase, of and unit"
                ));
            }
        }
        let phase = fields
            .get("phase")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty() && name.len() <= 128)
            .ok_or_else(|| format!("stage {n} needs \"phase\", a name of 1 to 128 bytes"))?;
        let of = match fields.get("of") {
            None | Some(Value::Null) => None,
            Some(of) => Some(
                of.as_f64()
                    .filter(|of| of.is_finite() && *of > 0.0)
                    .ok_or_else(|| format!("stage {n}'s \"of\" must be a number above 0"))?,
            ),
        };
        let unit = match fields.get("unit") {
            None | Some(Value::Null) => None,
            Some(unit) => Some(
                unit.as_str()
                    .filter(|unit| unit.len() <= 64)
                    .ok_or_else(|| {
                        format!("stage {n}'s \"unit\" must be a word of at most 64 bytes")
                    })?
                    .to_owned(),
            ),
        };
        if stages.last().is_some_and(|last| last.phase == phase) {
            return Err(format!(
                "stages {index} and {n} are both \"{phase}\"; list a stage again only after another stage, since a log cannot tell two in a row apart"
            ));
        }
        stages.push(Stage {
            phase: phase.into(),
            of,
            unit,
        });
    }
    Ok(stages)
}

/// Reports stored per occurrence before retention compacts them to at most 96.
pub const MAX_TIMING_REPORTS: usize = 128;

impl Snapshot {
    pub fn new(run_id: &str, ident: &str, generation: &str) -> Self {
        Self {
            run_id: run_id.into(),
            generation: generation.into(),
            rev: 0,
            config_hash: DEFAULT_CONFIG_HASH.into(),
            cursor: Cursor {
                ident: ident.into(),
                offset: 0,
                frame_start: 0,
                pending_base64: String::new(),
                dropping: false,
                extra: BTreeMap::new(),
            },
            coverage: Coverage::default(),
            latest: BTreeMap::new(),
            series: BTreeMap::new(),
            rows: BTreeMap::new(),
            row_order: Vec::new(),
            row_columns: BTreeMap::new(),
            groups: BTreeMap::new(),
            cells: BTreeMap::new(),
            counts: BTreeMap::new(),
            phases: Vec::new(),
            timing: Timing::default(),
            reducers: BTreeMap::new(),
            detectors: Detectors::default(),
            diagnostics: Diagnostics::default(),
            updated_at: 0,
            signal_order: Vec::new(),
            omitted: BTreeMap::new(),
            series_rev: 0,
            series_widths: BTreeMap::new(),
            spend: None,
            coordinate_report: BTreeMap::new(),
            last_canonical: None,
            last_line: String::new(),
            plan: None,
            extra: BTreeMap::new(),
            patterns: None,
            tally: None,
        }
    }

    /// Folds lines with `patterns` from here on, under their configuration's hash.
    pub fn configure(&mut self, patterns: Option<Arc<Patterns>>) {
        self.config_hash = patterns
            .as_ref()
            .map_or(DEFAULT_CONFIG_HASH, |p| p.hash())
            .to_owned();
        if let Some(tally) = self.tally.as_mut() {
            tally.hits = vec![Hit::default(); patterns.as_ref().map_or(0, |p| p.len())];
        }
        self.patterns = patterns;
    }

    /// Every source byte was folded; retention reductions do not create collection debt.
    pub fn source_complete(&self) -> bool {
        self.extra.get("terminal").and_then(Value::as_bool) == Some(true)
            && self.coverage.reasons.iter().all(|r| r == "truncated")
    }

    pub fn remove_reason(&mut self, reason: &str) {
        self.coverage.reasons.retain(|r| r != reason);
        self.coverage.complete = self.extra.get("terminal").and_then(Value::as_bool) == Some(true)
            && self.coverage.reasons.is_empty();
    }

    pub fn progress(&self) -> Option<&Phase> {
        self.phases.last().filter(|phase| phase.done.is_some())
    }

    /// Whether the payload reported anything: a measurement, progress or a row.
    pub fn has_signals(&self) -> bool {
        !(self.latest.is_empty()
            && self.phases.is_empty()
            && self.rows.is_empty()
            && self.counts.is_empty())
    }

    /// A detected bar that restarted: the run has several bars, so how much of the whole run
    /// is done is unknown from then on, though each bar still knows its own step of total.
    pub fn several_bars(&self) -> bool {
        self.phases
            .last()
            .is_some_and(|phase| phase.source == "detected" && phase.occurrence > 0)
    }

    pub fn diagnose(&mut self, code: &str, from: u64, to: u64, text: &str) {
        *self.diagnostics.counts.entry(code.into()).or_default() += 1;
        if self.diagnostics.examples.len() < 16 {
            let text: String = text
                .chars()
                .scan(0, |n, c| {
                    *n += c.len_utf8();
                    (*n <= 512).then_some(c)
                })
                .collect();
            self.diagnostics.examples.push(Diagnostic {
                extra: BTreeMap::new(),
                code: code.into(),
                from,
                to,
                text,
            });
        }
    }

    fn truncate(&mut self, key: &str) {
        *self.omitted.entry(key.into()).or_default() += 1;
        self.coverage.add_reason("truncated");
        self.diagnose(
            "truncated",
            self.cursor.frame_start,
            self.cursor.offset,
            key,
        );
    }

    /// `observed` is false for historical replay whose individual arrival times are unknown.
    pub fn feed(
        &mut self,
        bytes: &[u8],
        terminal: bool,
        now: i64,
        observed: bool,
    ) -> Result<(), String> {
        if !bytes.is_empty() && self.extra.get("terminal").and_then(Value::as_bool) == Some(true) {
            return Err("cannot append bytes after terminal EOF; start a new generation".into());
        }
        let mut pending = STANDARD
            .decode(&self.cursor.pending_base64)
            .map_err(|e| format!("invalid pending frame: {e}"))?;
        if pending.len() > MAX_FRAME {
            return Err("checkpoint pending frame exceeds 64 KiB".into());
        }
        let mut row_bytes = serde_json::to_vec(&self.rows)
            .map_err(|e| e.to_string())?
            .len();
        let before = self.cursor.offset;
        let was_terminal = self
            .extra
            .get("terminal")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let was_complete = self.coverage.complete;
        let mut framing = Framing::new(&pending, self.cursor.frame_start == 0);
        for &byte in bytes {
            self.cursor.offset += 1;
            if byte == b'\n' || byte == b'\r' {
                if !self.cursor.dropping && !pending.is_empty() {
                    let keep = byte == b'\r' && framing.incomplete();
                    if keep {
                        self.buffer_byte(&mut pending, byte);
                        framing.push(byte);
                    } else {
                        self.frame(&pending, now, observed, false, &mut row_bytes);
                        pending.clear();
                        framing = Framing::default();
                    }
                }
                if self.cursor.dropping {
                    self.finish_dropped(&mut pending);
                    framing = Framing::default();
                }
                if byte == b'\n' {
                    if let Some(tally) = self.tally.as_mut() {
                        tally.lines += 1;
                    }
                }
                if byte == b'\n' || pending.is_empty() {
                    self.cursor.dropping = false;
                    self.cursor.frame_start = self.cursor.offset;
                }
            } else {
                self.buffer_byte(&mut pending, byte);
                framing.push(byte);
            }
        }
        let terminal = terminal || bytes.is_empty() && was_terminal;
        if terminal && self.cursor.dropping {
            self.finish_dropped(&mut pending);
        }
        if terminal && !pending.is_empty() && !self.cursor.dropping {
            self.frame(&pending, now, observed, true, &mut row_bytes);
            pending.clear();
            self.cursor.frame_start = self.cursor.offset;
        }
        self.cursor.pending_base64 = STANDARD.encode(pending);
        if before < self.cursor.offset {
            if let Some(last) = self.coverage.ranges.last_mut().filter(|r| r.1 == before) {
                last.1 = self.cursor.offset;
            } else {
                self.coverage.ranges.push((before, self.cursor.offset));
            }
        }
        self.coverage.complete = terminal && self.coverage.reasons.is_empty();
        self.extra.insert("terminal".into(), Value::Bool(terminal));
        if before != self.cursor.offset
            || self.coverage.complete != was_complete
            || terminal != was_terminal
        {
            self.rev += 1;
            self.updated_at = now;
            if before != self.cursor.offset && observed {
                self.timing.last_log_at = Some(now);
            }
        }
        Ok(())
    }

    fn buffer_byte(&mut self, pending: &mut Vec<u8>, byte: u8) {
        pending.push(byte);
        if self.cursor.dropping {
            if pending.len() > 160 {
                pending.remove(0);
            }
        } else if pending.len() > MAX_FRAME {
            self.diagnose(
                "oversized_record",
                self.cursor.frame_start,
                self.cursor.offset,
                "raw frame exceeds 64 KiB",
            );
            if let Some(example) = self
                .diagnostics
                .examples
                .last_mut()
                .filter(|e| e.code == "oversized_record" && e.from == self.cursor.frame_start)
            {
                example.extra.insert(
                    "headBase64".into(),
                    Value::String(STANDARD.encode(&pending[..160])),
                );
            }
            let discard = pending.len() - 160;
            pending.drain(..discard);
            self.cursor.dropping = true;
        }
    }

    fn finish_dropped(&mut self, pending: &mut Vec<u8>) {
        if let Some(example) = self
            .diagnostics
            .examples
            .iter_mut()
            .rev()
            .find(|e| e.code == "oversized_record" && e.from == self.cursor.frame_start)
        {
            example.to = self.cursor.offset;
            example.extra.insert(
                "tailBase64".into(),
                Value::String(STANDARD.encode(&*pending)),
            );
        }
        pending.clear();
        self.cursor.dropping = false;
    }

    fn frame(
        &mut self,
        bytes: &[u8],
        now: i64,
        observed: bool,
        terminal: bool,
        row_bytes: &mut usize,
    ) {
        if std::str::from_utf8(bytes).is_err() && bytes.windows(7).any(|w| w == b"::orx::") {
            self.diagnose(
                "invalid_record",
                self.cursor.frame_start,
                self.cursor.offset,
                "frame contains invalid UTF-8",
            );
            return;
        }
        let text = strip_ansi(bytes, self.cursor.frame_start == 0);
        let text = text.as_str();
        self.note_line(text);
        if let Some(json) = text.strip_prefix("::orx:: ") {
            match parse(json) {
                Ok(Value::Object(record)) => {
                    self.record(record, now, observed, "declared", row_bytes)
                }
                Ok(_) => self.diagnose(
                    "invalid_record",
                    self.cursor.frame_start,
                    self.cursor.offset,
                    "payload must be an object",
                ),
                Err(error) => self.diagnose(
                    if terminal && declared_incomplete(bytes) {
                        "incomplete_record"
                    } else {
                        "invalid_record"
                    },
                    self.cursor.frame_start,
                    self.cursor.offset,
                    &error,
                ),
            }
        } else if text.contains("::orx::") {
            self.diagnose(
                "misplaced_marker",
                self.cursor.frame_start,
                self.cursor.offset,
                "start the line with `::orx:: ` followed by a JSON object",
            );
        } else {
            let matched = self.patterns.as_ref().and_then(|p| p.record(text));
            if let Some((index, record)) = matched {
                let line = self.tally.as_ref().map_or(0, |t| t.lines + 1);
                if let Some(hit) = self.tally.as_mut().and_then(|t| t.hits.get_mut(index)) {
                    hit.matched += 1;
                    hit.first.get_or_insert_with(|| (line, text.to_owned()));
                }
                self.record(record, now, observed, "pattern", row_bytes);
            } else if self
                .phases
                .last()
                .is_none_or(|p| p.source != "declared" && p.source != "pattern")
            {
                self.detect(text, now, observed, row_bytes);
            }
        }
    }

    fn note_line(&mut self, text: &str) {
        let line = message(text.trim());
        if line.contains("::orx::") || !line.chars().any(char::is_alphanumeric) {
            return;
        }
        self.last_line.clear();
        self.last_line.extend(
            line.chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(MAX_LAST_LINE),
        );
    }

    fn record(
        &mut self,
        mut record: Map<String, Value>,
        now: i64,
        observed: bool,
        source: &str,
        row_bytes: &mut usize,
    ) {
        record.retain(|k, _| {
            matches!(
                k.as_str(),
                "step" | "of" | "phase" | "unit" | "t" | "row" | "spend" | "m" | "plan"
            )
        });
        let mut canonical = record.clone();
        canonical.remove("t");
        let canonical = Value::Object(canonical);
        let duplicate = !record.contains_key("row")
            && (record.contains_key("step") || self.progress().is_some())
            && self
                .phases
                .last()
                .is_none_or(|phase| phase.source == source)
            && self.last_canonical.as_ref() == Some(&canonical);
        self.last_canonical = Some(canonical);
        let time = record
            .get("t")
            .and_then(Value::as_f64)
            .filter(|x| x.is_finite() && *x >= 0.0);
        if record.contains_key("t") && time.is_none() {
            self.bad_field("t");
        }
        let previous_time = if source == "declared" {
            self.timing.payload_t
        } else {
            self.progress()
                .filter(|p| p.source == source && p.timing_source == "elapsed_bar")
                .and_then(|p| p.last_point.map(|point| point.0))
        };
        let origin = time.zip(previous_time).is_some_and(|(t, old)| t < old);
        if origin {
            // A clock back within its first second restarted with a new process, as a relaunch or
            // the next stage's script does; anywhere else, one process's reports disagree.
            let restarted = time.is_some_and(|t| t <= 1.0);
            self.new_origin(
                source,
                record.get("phase").and_then(Value::as_str),
                !restarted,
            );
        }
        if source == "declared" && time.is_some() {
            self.timing.payload_t = time;
        }
        if duplicate && !origin {
            return;
        }
        let phase = record
            .get("phase")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 128);
        if record.contains_key("phase") && phase.is_none() {
            self.bad_field("phase");
        }
        let step = record
            .get("step")
            .and_then(Value::as_f64)
            .filter(|s| s.is_finite() && *s >= 0.0);
        let valid_total = record
            .get("of")
            .is_some_and(|v| v.is_null() || v.as_f64().is_some_and(|n| n.is_finite() && n > 0.0));
        let unit = record
            .get("unit")
            .and_then(Value::as_str)
            .filter(|u| u.len() <= 64);
        let progress_record = phase.is_some() || step.is_some() || valid_total || unit.is_some();
        let changed = progress_record
            && self
                .phases
                .last()
                .is_some_and(|p| phase.is_some_and(|name| p.name != name) || p.source != source);
        if progress_record && (changed || self.phases.is_empty()) {
            self.begin_phase(phase.unwrap_or(""), source, now, observed);
        }

        let total_before = self.phases.last().and_then(|p| p.of);
        if let Some(value) = record.get("of") {
            let total = value.as_f64().filter(|n| n.is_finite() && *n > 0.0);
            if value.is_null() || total.is_some() {
                let phase = self.phases.last_mut().unwrap();
                if phase.of != total {
                    self.timing
                        .progress_points
                        .retain(|p| p.0 != phase.occurrence);
                    phase.last_point = None;
                }
                phase.of = total;
            } else {
                self.bad_field("of");
            }
        }
        if let Some(unit) = record.get("unit") {
            if let Some(unit) = unit.as_str().filter(|u| u.len() <= 64) {
                self.phases.last_mut().unwrap().unit = Some(unit.into());
            } else {
                self.bad_field("unit");
            }
        }
        let mut out_of_order = false;
        if record.contains_key("step") {
            if let Some(step) = step {
                let old = self.phases.last().unwrap().done.unwrap_or(0.0);
                // A report that changes the total starts another loop, and counts from its own start.
                let restated = self.phases.last().unwrap().of != total_before;
                if step < old && !origin && !restated {
                    self.bad_field("step_regression");
                    out_of_order = true;
                } else {
                    if step != old || origin {
                        self.coordinate_report.clear();
                    }
                    if step < old && !restated {
                        self.redo_after(step, old);
                    }
                    let phase = self.phases.last_mut().unwrap();
                    phase.done = Some(step);
                    let t = time.or_else(|| {
                        (observed
                            && !matches!(phase.timing_source.as_str(), "payload" | "elapsed_bar"))
                        .then(|| {
                            let last = phase.last_point.map_or(0.0, |point| point.0);
                            (now as f64 / 1000.0).max(last)
                        })
                    });
                    if time.is_some() && phase.timing_source == "observed" {
                        phase.last_point = None;
                        self.timing
                            .progress_points
                            .retain(|(occurrence, _, _)| *occurrence != phase.occurrence);
                    }
                    if let Some(t) = t {
                        phase.timing_source = if time.is_some() {
                            if source == "detected" {
                                "elapsed_bar"
                            } else {
                                "payload"
                            }
                        } else {
                            "observed"
                        }
                        .into();
                        phase.last_point = Some((t, step));
                        self.timing
                            .progress_points
                            .push((phase.occurrence, t, step));
                        retain_timing(&mut self.timing.progress_points, phase.occurrence, phase.of);
                        if (step > old || origin) && time.is_some() {
                            self.timing.last_advance = Some((phase.occurrence, t));
                        }
                    }
                }
            } else {
                self.bad_field("step");
            }
        }
        if let Some(value) = record.get("plan") {
            match plan(value, false) {
                Ok(stages) => self.plan = Some(stages),
                Err(_) => self.bad_field("plan"),
            }
        }
        if let Some(spend) = record.get("spend") {
            if let Some(spend) = spend
                .as_f64()
                .filter(|n| n.is_finite() && *n >= self.spend.unwrap_or(0.0))
            {
                self.spend = Some(spend);
            } else {
                self.bad_field("spend");
            }
        }
        if observed && self.timing.origin_ms.is_none() {
            self.timing.origin_ms = Some(now);
        }
        let x = self.progress().and_then(|p| p.done).or(time).or_else(|| {
            observed.then(|| (now - self.timing.origin_ms.unwrap_or(now)) as f64 / 1000.0)
        });
        // An out-of-order report has no step to plot at: its values are only the latest.
        let x = x.filter(|_| !out_of_order);
        let coordinated = record.contains_key("step") && !out_of_order;
        // In the order printed, so a new table's columns read as the run wrote them.
        let mut cells = Map::new();
        if let Some(metrics) = record.get("m") {
            if let Some(metrics) = metrics.as_object() {
                for (name, value) in metrics {
                    if !identifier(name)
                        || !(value.is_number() || value.is_string() || value.is_null())
                        || value.as_str().is_some_and(|s| s.len() > 1024)
                    {
                        self.bad_field(name);
                        continue;
                    }
                    if value.is_null() {
                        continue;
                    }
                    if matches!(value.as_str(), Some("NaN" | "Infinity" | "-Infinity")) {
                        self.diagnose(
                            "non_finite",
                            self.cursor.frame_start,
                            self.cursor.offset,
                            name,
                        );
                    }
                    cells.insert(name.clone(), value.clone());
                    if !self.latest.contains_key(name) && self.latest.len() == 64 {
                        self.truncate("metrics");
                        continue;
                    }
                    if coordinated
                        && self.coordinate_report.get(name).is_some_and(|v| {
                            v != value
                                && v.as_f64()
                                    .zip(value.as_f64())
                                    .is_none_or(|(a, b)| a != b || a.abs() > 9_007_199_254_740_992.)
                        })
                    {
                        self.bad_field("conflicting_report");
                    }
                    if !out_of_order {
                        self.coordinate_report.insert(name.clone(), value.clone());
                    }
                    if !self.latest.contains_key(name) {
                        self.signal_order.push(name.clone());
                    }
                    self.latest.insert(name.clone(), value.clone());
                    let Some(x) = x else {
                        continue;
                    };
                    let series = self.series.entry(name.clone()).or_default();
                    let point = (x, value.as_f64());
                    // A relaunch redoes the step it resumes at: its value replaces the old one.
                    let redone = series
                        .last_mut()
                        .filter(|last| origin && last.0 == x && **last != point);
                    if let Some(last) = redone {
                        *last = point;
                        self.series_rev += 1;
                    } else if series.last() != Some(&point) {
                        series.push(point);
                    }
                    if series.len() > 2048 {
                        let span = (series.last().unwrap().0 - series[0].0).abs();
                        let initial = if span > 0.0 {
                            2f64.powf((span / 2048.).log2().floor())
                                .max(f64::MIN_POSITIVE)
                        } else {
                            1.0
                        };
                        let width = self.series_widths.entry(name.clone()).or_insert(initial);
                        compact(series, *width);
                        while series.len() > 2048 {
                            *width = (*width * 2.0).min(f64::MAX);
                            compact(series, *width);
                        }
                        self.series_rev += 1;
                    }
                }
            } else {
                self.bad_field("m");
            }
        }
        if let Some(row) = record.get("row") {
            if let Some(row) = row.as_str().filter(|r| identifier(r)) {
                if !self.rows.contains_key(row) && self.rows.len() == 16 {
                    self.truncate("tables");
                } else {
                    let columns = self.row_columns.entry(row.into()).or_default();
                    let mut omitted = 0;
                    cells.retain(|name, _| {
                        if columns.contains(name) {
                            return true;
                        }
                        if columns.len() == 64 {
                            omitted += 1;
                            return false;
                        }
                        columns.push(name.clone());
                        true
                    });
                    for _ in 0..omitted {
                        self.truncate("row_columns");
                    }
                    let overhead = match self.rows.get(row) {
                        Some(rows) => usize::from(!rows.is_empty()),
                        None => row.len() + 5 + usize::from(!self.rows.is_empty()),
                    };
                    *row_bytes += serde_json::to_vec(&cells)
                        .expect("JSON scalar cells serialize")
                        .len()
                        + overhead;
                    self.rows
                        .entry(row.into())
                        .or_default()
                        .push(cells.into_iter().collect());
                    self.row_order.push(row.into());
                    while self.rows.values().map(Vec::len).sum::<usize>() > 2048
                        || *row_bytes > 2 * 1024 * 1024
                    {
                        let name = self.row_order.remove(0);
                        let rows = self.rows.get_mut(&name).unwrap();
                        let removed = rows.remove(0);
                        *row_bytes -= serde_json::to_vec(&removed)
                            .expect("JSON cells serialize")
                            .len()
                            + usize::from(!rows.is_empty());
                        self.truncate("rows");
                    }
                }
            } else {
                self.bad_field("row");
            }
        }
    }

    /// A relaunch that kept its total redoes the steps after `step`: the points the old stretch
    /// plotted there go, so each step shows once. Points of an earlier phase end below them.
    fn redo_after(&mut self, step: f64, high: f64) {
        let mut dropped = false;
        for points in self.series.values_mut() {
            while points.last().is_some_and(|&(x, _)| step < x && x <= high) {
                points.pop();
                dropped = true;
            }
        }
        if dropped {
            self.series_rev += 1;
        }
    }

    /// `t` stepped back: a process relaunched, or a second clock began. A new stretch starts, and
    /// no difference is taken across the step, so a phase the report continues forgets the
    /// points before it. The report says where the run is: its step and total replace what was
    /// known, even a lower step, which is out-of-order noise in any other report.
    fn new_origin(&mut self, source: &str, phase: Option<&str>, unexpected: bool) {
        if unexpected {
            self.diagnose(
                "t_origin",
                self.cursor.frame_start,
                self.cursor.offset,
                "t stepped back: a new origin",
            );
        }
        let continued = self
            .phases
            .last_mut()
            .filter(|p| p.source == source && phase.is_none_or(|name| name == p.name));
        let Some(phase) = continued else {
            return;
        };
        phase.last_point = None;
        let occurrence = phase.occurrence;
        self.timing.progress_points.retain(|p| p.0 != occurrence);
        if self.timing.anchor.is_some_and(|a| a.0 == occurrence) {
            self.timing.anchor = None;
        }
        if self.timing.last_advance.is_some_and(|a| a.0 == occurrence) {
            self.timing.last_advance = None;
        }
    }

    /// A scan read the log to its end; `at` is the earlier of the log's last change and the
    /// scan's time, so every report read so far was written no later. The last advance sits
    /// there unless an earlier scan already placed it sooner; reports timed by their reading
    /// move back to it, and a phase replayed without times gets its last advance there.
    pub fn reached_end(&mut self, at: i64) {
        let before = (self.timing.clone(), self.phases.clone());
        self.place(at);
        if (&self.timing, &self.phases) != (&before.0, &before.1) {
            self.rev += 1;
        }
    }

    fn place(&mut self, at: i64) {
        if let Some((occurrence, t)) = self.timing.last_advance {
            let at = match self.timing.anchor {
                Some((o, placed_t, placed_at)) if o == occurrence && placed_t == t => {
                    placed_at.min(at)
                }
                _ => at,
            };
            self.timing.anchor = Some((occurrence, t, at));
        }

        let seconds = at as f64 / 1000.0;
        let observed: Vec<usize> = self
            .phases
            .iter()
            .filter(|p| p.timing_source == "observed")
            .map(|p| p.occurrence)
            .collect();
        for point in &mut self.timing.progress_points {
            if observed.contains(&point.0) {
                point.1 = point.1.min(seconds);
            }
        }
        let Some(phase) = self.phases.last_mut() else {
            return;
        };
        if !matches!(phase.timing_source.as_str(), "observed" | "unknown") {
            return;
        }
        if let Some(point) = phase.last_point.as_mut() {
            point.0 = point.0.min(seconds);
        }
        let Some(done) = phase.done else {
            return;
        };
        if phase.last_point.is_some_and(|point| point.1 == done) {
            return;
        }
        phase.timing_source = "observed".into();
        phase.last_point = Some((seconds, done));
        self.timing
            .progress_points
            .push((phase.occurrence, seconds, done));
        retain_timing(&mut self.timing.progress_points, phase.occurrence, phase.of);
    }

    fn begin_phase(&mut self, name: &str, source: &str, now: i64, observed: bool) {
        let occurrence = self
            .phases
            .last()
            .map_or(0, |p| p.occurrence.saturating_add(1));
        if self.phases.len() == 64 {
            let removed = self.phases.remove(0);
            self.timing
                .progress_points
                .retain(|(phase, _, _)| *phase != removed.occurrence);
            *self.omitted.entry("phases".into()).or_default() += 1;
        }
        if let Some(old) = self.phases.last_mut() {
            old.ended_at = observed.then_some(now);
        }
        self.last_line.clear();
        self.phases.push(Phase {
            extra: BTreeMap::new(),
            name: name.into(),
            occurrence,
            of: None,
            unit: None,
            done: None,
            started_at: if occurrence == 0 {
                self.timing.started_ms.or(observed.then_some(now))
            } else {
                observed.then_some(now)
            },
            ended_at: None,
            timing_source: "unknown".into(),
            source: source.into(),
            last_point: None,
        });
        self.coordinate_report.clear();
    }

    fn bad_field(&mut self, field: &str) {
        self.diagnose(
            if field == "conflicting_report" {
                field
            } else {
                "invalid_field"
            },
            self.cursor.frame_start,
            self.cursor.offset,
            field,
        );
    }

    fn detect(&mut self, text: &str, now: i64, observed: bool, row_bytes: &mut usize) {
        let line = message(text.trim());
        let has_bar = line.contains('|');
        let text = line.rsplit_once('|').map_or(line, |(_, tail)| tail.trim());
        let (epoch, text) = text
            .strip_prefix("Epoch ")
            .map_or((false, text), |s| (true, s.trim_start()));
        let (coordinate, rest) = text
            .split_once(char::is_whitespace)
            .map_or((text, ""), |(c, r)| (c, r.trim()));
        let time = rest
            .strip_prefix('[')
            .and_then(|s| s.split([',', ']']).next())
            .and_then(elapsed);
        // `done` is the detector's own coordinate; an epoch line names the epoch in progress.
        let found = if let Some((done, total)) = coordinate.split_once('/') {
            number(done).zip(number(total)).and_then(|(done, total)| {
                if epoch {
                    Some(("epoch", done, Some(total), None, Some("epochs".into())))
                } else if rest.starts_with('[') {
                    Some(("tqdm", done, Some(total), time, None))
                } else if !has_bar && rest.is_empty() {
                    Some(("counter", done, Some(total), None, Some("items".into())))
                } else {
                    None
                }
            })
        } else if let Some(done) = coordinate.strip_suffix("it").and_then(number) {
            (!epoch && rest.starts_with('[')).then_some(("tqdm_open", done, None, time, None))
        } else {
            None
        };
        let found = found.or_else(|| {
            labelled(line)
                .or_else(|| percent_of(line))
                .map(|(done, total, unit)| ("counter", done, Some(total), None, unit))
        });
        let Some((kind, done, total, time, unit)) = found else {
            return;
        };
        let step = if epoch { done - 1.0 } else { done };
        if step < 0.0 || total.is_some_and(|n| n <= 0.0 || done > n) {
            return;
        }
        let key = format!("{kind}:{total:?}:{}", unit.as_deref().unwrap_or_default());
        if self.detectors.selected.is_some() {
            let (old, of) = self.detectors.previous.unwrap();
            if self.detectors.selected.as_ref() != Some(&key) || done < old {
                if of.is_none_or(|total| old < total) {
                    return;
                }
                self.detectors.selected = None;
                self.detectors.candidate = None;
                self.detectors.previous = None;
                self.detectors.confirmations = 0;
            } else if done == old {
                return;
            }
        }
        let compatible = self.detectors.candidate.as_ref() == Some(&key)
            && self
                .detectors
                .previous
                .is_some_and(|(old, of)| of == total && done >= old);
        self.detectors.confirmations = if compatible {
            self.detectors.confirmations + 1
        } else {
            1
        };
        self.detectors.candidate = Some(key.clone());
        self.detectors.previous = Some((done, total));
        if self.detectors.confirmations < 2 {
            return;
        }
        if self.detectors.selected.is_none()
            && self.phases.last().is_some_and(|p| p.source == "detected")
        {
            self.begin_phase("", "detected", now, observed);
            self.last_canonical = None;
        }
        self.detectors.selected = Some(key);
        let mut record = Map::new();
        record.insert("step".into(), Value::from(step));
        record.insert("of".into(), total.map_or(Value::Null, Value::from));
        // tqdm's own `it` names no unit; a bar that set one shows it only in its rate.
        if let Some(unit) = unit {
            record.insert("unit".into(), Value::from(unit));
        }
        if let Some(time) = time {
            record.insert("t".into(), Value::from(time));
        }
        self.record(record, now, observed, "detected", row_bytes);
    }
}

const LEVELS: [&str; 8] = [
    "TRACE", "DEBUG", "INFO", "WARN", "WARNING", "ERROR", "CRITICAL", "FATAL",
];

const SEPARATORS: [char; 6] = [' ', '-', ':', '|', ']', '['];

fn is_level(token: &str) -> bool {
    LEVELS.contains(&token.trim_matches(&SEPARATORS[..]))
}

fn starts_with_time(token: &str) -> bool {
    let b = token.as_bytes();
    b.len() >= 8
        && [0, 1, 3, 4, 6, 7].iter().all(|&i| b[i].is_ascii_digit())
        && b[2] == b':'
        && b[5] == b':'
}

fn starts_with_date(token: &str) -> bool {
    let b = token.as_bytes();
    b.len() >= 10
        && [0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|&i| b[i].is_ascii_digit())
        && b[4] == b'-'
        && b[7] == b'-'
}

/// What a log record says, past the timestamp, logger and level a logging library puts in front
/// of it: Python (`2026-11-13 13:20:13,057 - name - INFO - msg`, `INFO:name:msg`), glog
/// (`I1113 13:20:42.713 file.py:7] msg`), env_logger and tracing. Anything else is returned whole.
fn message(line: &str) -> &str {
    glog(line)
        .or_else(|| level_first(line))
        .or_else(|| stamped(line))
        .unwrap_or(line)
}

fn glog(line: &str) -> Option<&str> {
    let b = line.as_bytes();
    let record = b.len() > 6
        && b"IWEF".contains(&b[0])
        && b[1..5].iter().all(u8::is_ascii_digit)
        && b[5] == b' '
        && starts_with_time(&line[6..]);
    let (_, message) = line.split_once("] ").filter(|_| record)?;
    Some(message.trim_start())
}

/// Python's default format: `INFO:logger:msg`.
fn level_first(line: &str) -> Option<&str> {
    let (_, rest) = line
        .split_once(':')
        .filter(|(level, _)| LEVELS.contains(level))?;
    match rest.split_once(':') {
        Some((logger, message)) if !logger.contains(char::is_whitespace) => {
            Some(message.trim_start())
        }
        _ => Some(rest.trim_start()),
    }
}

/// A line opening with a timestamp, then a logger and a level within six words.
fn stamped(line: &str) -> Option<&str> {
    let bracketed = line.starts_with('[');
    let body = line.strip_prefix('[').unwrap_or(line);
    let (first, rest) = body.split_once(char::is_whitespace).unwrap_or((body, ""));
    let rest = rest.trim_start();
    let rest = if starts_with_time(first) || (starts_with_date(first) && first.contains('T')) {
        rest
    } else if starts_with_date(first) {
        let (time, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        starts_with_time(time).then_some(after.trim_start())?
    } else {
        return None;
    };
    let mut words = rest;
    for _ in 0..6 {
        let (word, tail) = words.split_once(char::is_whitespace).unwrap_or((words, ""));
        if is_level(word) {
            let tail = match tail.split_once(']') {
                Some((_, message)) if bracketed && !word.contains(']') => message,
                _ => tail,
            };
            let tail = tail.trim_start_matches(&SEPARATORS[..]);
            return Some(match tail.split_once(char::is_whitespace) {
                Some((target, message)) if target.ends_with(':') && target.contains("::") => {
                    message.trim_start()
                }
                _ => tail,
            });
        }
        words = tail.trim_start();
    }
    Some(rest.trim_start_matches(&SEPARATORS[..]))
}

/// Nouns runs count, singular and plural; the plural is the unit.
const COUNTED: [(&str, &str); 39] = [
    ("batch", "batches"),
    ("case", "cases"),
    ("checkpoint", "checkpoints"),
    ("chunk", "chunks"),
    ("config", "configs"),
    ("doc", "docs"),
    ("document", "documents"),
    ("episode", "episodes"),
    ("example", "examples"),
    ("file", "files"),
    ("fold", "folds"),
    ("frame", "frames"),
    ("generation", "generations"),
    ("image", "images"),
    ("item", "items"),
    ("iter", "iters"),
    ("iteration", "iterations"),
    ("job", "jobs"),
    ("merge", "merges"),
    ("model", "models"),
    ("page", "pages"),
    ("part", "parts"),
    ("prompt", "prompts"),
    ("query", "queries"),
    ("question", "questions"),
    ("record", "records"),
    ("rollout", "rollouts"),
    ("round", "rounds"),
    ("row", "rows"),
    ("run", "runs"),
    ("sample", "samples"),
    ("seed", "seeds"),
    ("shard", "shards"),
    ("step", "steps"),
    ("task", "tasks"),
    ("test", "tests"),
    ("token", "tokens"),
    ("trial", "trials"),
    ("video", "videos"),
];

/// A count and total opening a line, in brackets (`[3/10] Compiling`) or after one counted noun
/// (`chunk 3/10 done`), each with an optional colon, and its unit.
fn labelled(line: &str) -> Option<(f64, f64, Option<String>)> {
    let counter = |token: &str| {
        let (done, total) = token.strip_suffix(':').unwrap_or(token).split_once('/')?;
        number(done).zip(number(total))
    };
    let (first, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    if let Some(inner) = first.strip_prefix('[') {
        let inner = inner.strip_suffix(':').unwrap_or(inner).strip_suffix(']')?;
        return counter(inner).map(|(done, total)| (done, total, Some("items".into())));
    }

    let word = first
        .strip_suffix(':')
        .unwrap_or(first)
        .to_ascii_lowercase();
    let &(singular, plural) = COUNTED.iter().find(|(s, p)| word == *s || word == *p)?;
    let unit = (!matches!(singular, "step" | "iter" | "iteration")).then(|| plural.into());
    let token = rest.trim_start().split(char::is_whitespace).next()?;
    counter(token).map(|(done, total)| (done, total, unit))
}

/// A percentage with its count in parentheses, after at most a three-word label that ends in a
/// colon (`Progress: 56% (18202/32503 merges)`); a noun after the count is the unit.
fn percent_of(line: &str) -> Option<(f64, f64, Option<String>)> {
    let rest = match line.split_once(':') {
        Some((label, rest))
            if label.split_whitespace().count() <= 3
                && label
                    .chars()
                    .all(|c| c.is_alphabetic() || c.is_whitespace()) =>
        {
            rest
        }
        _ => line,
    };
    let (percent, rest) = rest.split_once('%')?;
    let percent: f64 = percent.trim().parse().ok()?;
    let inner = rest.trim_start().strip_prefix('(')?.split(')').next()?;
    let mut inner = inner.split_whitespace();
    let (done, total) = inner.next()?.split_once('/')?;
    let (done, total) = number(done).zip(number(total))?;
    let unit = inner
        .next()
        .filter(|noun| noun.len() <= 20 && noun.chars().all(|c| c.is_ascii_alphabetic()))
        .map(str::to_ascii_lowercase);
    let consistent = total > 0.0 && (done / total * 100.0 - percent).abs() <= 1.0;
    (consistent && inner.next().is_none()).then_some((done, total, unit))
}

fn number(text: &str) -> Option<f64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<f64>().ok().filter(|n| n.is_finite())
}

fn elapsed(text: &str) -> Option<f64> {
    let text = text.split('<').next()?;
    let parts: Vec<_> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    parts.into_iter().try_fold(0.0, |n, p| {
        let v = p.parse::<f64>().ok()?;
        (v.is_finite() && v >= 0.0).then_some(n * 60.0 + v)
    })
}

/// Keeps every report of one occurrence that the estimator reads, so its answer from the kept
/// reports equals its answer from all of them: the estimator collapses reports that share a time
/// to the last, interpolates each slice boundary between the collapsed reports around it, and
/// takes cadence from the last 12 positive gaps between the second report and the reports that
/// advance. Slice brackets are kept as collapsed reports; a cadence report needs only its time.
fn retain_timing(points: &mut Vec<(usize, f64, f64)>, occurrence: usize, total: Option<f64>) {
    let start = points.partition_point(|p| p.0 < occurrence);
    if points.len() - start <= MAX_TIMING_REPORTS {
        return;
    }
    let reports = points.split_off(start);
    let n = reports.len();
    let at = |i: usize| reports[i].1;
    let done = |i: usize| reports[i].2;
    let mut keep = vec![false; n];

    keep[0] = true;
    // The newest report brackets the next boundary a later report crosses.
    keep[n - 1] = true;
    if let Some(total) = total {
        let collapsed: Vec<_> = (0..n)
            .filter(|&i| i + 1 == n || at(i + 1) != at(i))
            .collect();
        for k in 0..=slice_count(total) {
            let goal = slice_boundary(total, k);
            let first = collapsed.partition_point(|&i| done(i) < goal);
            if first < collapsed.len() {
                keep[collapsed[first]] = true;
                if first > 0 {
                    keep[collapsed[first - 1]] = true;
                }
            }
        }
    }
    let mut times = 0;
    let mut previous = None;
    for i in (1..n).rev().filter(|&i| i == 1 || done(i) > done(i - 1)) {
        if previous != Some(at(i)) {
            times += 1;
            if times > 13 {
                break;
            }
            previous = Some(at(i));
            keep[i] = true;
        }
    }
    points.extend(
        reports
            .into_iter()
            .zip(keep)
            .filter_map(|(p, keep)| keep.then_some(p)),
    );
}

fn compact(points: &mut Vec<Point>, width: f64) {
    let mut out = Vec::new();
    let mut start = 0;
    while start < points.len() {
        let bucket = (points[start].0 / width).floor();
        let mut end = start + 1;
        while end < points.len() && (width == f64::MAX || (points[end].0 / width).floor() == bucket)
        {
            end += 1;
        }
        let mut keep = vec![start, end - 1];
        let finite: Vec<_> = (start..end).filter(|i| points[*i].1.is_some()).collect();
        if let Some(i) = finite
            .iter()
            .min_by(|a, b| points[**a].1.partial_cmp(&points[**b].1).unwrap())
        {
            keep.push(*i);
        }
        if let Some(i) = finite
            .iter()
            .max_by(|a, b| points[**a].1.partial_cmp(&points[**b].1).unwrap())
        {
            keep.push(*i);
        }
        if let Some(i) = (start..end).find(|i| points[*i].1.is_none()) {
            keep.push(i);
        }
        keep.sort_unstable();
        keep.dedup();
        out.extend(keep.into_iter().map(|i| points[i]));
        start = end;
    }
    *points = out;
}

pub fn identifier(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

#[cfg(test)]
thread_local! {
    static PARSED_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static WORK_LIMIT: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}
#[cfg(test)]
fn track_work(bytes: usize) {
    PARSED_BYTES.with(|total| {
        total.set(total.get() + bytes);
        WORK_LIMIT.with(|limit| {
            assert!(
                total.get() <= limit.get(),
                "frame parsing rescans exceeded the work bound"
            )
        });
    });
}
#[derive(Default)]
struct Escape(u8);
impl Escape {
    fn visible(&mut self, byte: u8) -> bool {
        match self.0 {
            1 => {
                self.0 = match byte {
                    b'[' => 2,
                    b']' => 3,
                    0x20..=0x2f => 5,
                    _ => 0,
                };
                false
            }
            5 => {
                if !(0x20..=0x2f).contains(&byte) {
                    self.0 = 0;
                }
                false
            }
            2 => {
                if (0x40..=0x7e).contains(&byte) {
                    self.0 = 0;
                }
                false
            }
            3 => {
                if byte == 7 {
                    self.0 = 0;
                } else if byte == 27 {
                    self.0 = 4;
                }
                false
            }
            4 => {
                self.0 = if byte == b'\\' { 0 } else { 3 };
                false
            }
            _ if byte == 27 => {
                self.0 = 1;
                false
            }
            _ => true,
        }
    }
}

fn frame_start(bytes: &[u8], bom: bool) -> &[u8] {
    if bom {
        bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)
    } else {
        bytes
    }
}

fn strip_ansi(bytes: &[u8], bom: bool) -> String {
    #[cfg(test)]
    track_work(bytes.len());
    let mut escape = Escape::default();
    let out: Vec<_> = bytes
        .iter()
        .copied()
        .filter(|b| escape.visible(*b))
        .collect();
    String::from_utf8_lossy(frame_start(&out, bom)).into_owned()
}

#[derive(Default)]
struct Framing {
    prefix: Vec<u8>,
    declared: bool,
    body: bool,
    depth: i32,
    quoted: bool,
    escaped: bool,
    ansi: Escape,
    allow_bom: bool,
}
impl Framing {
    fn new(bytes: &[u8], bom: bool) -> Self {
        let mut state = Self {
            allow_bom: bom,
            ..Self::default()
        };
        for &byte in bytes {
            state.push(byte);
        }
        state
    }
    fn push(&mut self, byte: u8) {
        #[cfg(test)]
        track_work(1);
        if !self.ansi.visible(byte) {
            return;
        }
        if self.prefix.len() < 8 {
            self.prefix.push(byte);
            if self.allow_bom && frame_start(&self.prefix, true).is_empty() {
                self.prefix.clear();
                self.allow_bom = false;
            }
            self.declared = self.prefix == b"::orx:: ";
            return;
        }
        if !self.declared {
            return;
        }
        if self.quoted {
            if self.escaped {
                self.escaped = false;
            } else if byte == b'\\' {
                self.escaped = true;
            } else if byte == b'"' {
                self.quoted = false;
            }
        } else {
            if !byte.is_ascii_whitespace() {
                self.body = true;
            }
            match byte {
                b'"' => self.quoted = true,
                b'{' | b'[' => self.depth += 1,
                b'}' | b']' => self.depth -= 1,
                _ => {}
            }
        }
    }
    fn incomplete(&self) -> bool {
        self.declared && (!self.body || self.depth > 0 || self.quoted)
    }
}
fn declared_incomplete(bytes: &[u8]) -> bool {
    Framing::new(bytes, true).incomplete()
}

fn normalize(text: &str) -> Result<String, String> {
    #[cfg(test)]
    track_work(text.len());
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut quoted = false;
    let mut escaped = false;
    while i < bytes.len() {
        let b = bytes[i];
        if quoted {
            out.push(b);
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
            i += 1;
            continue;
        }
        if b == b'"' {
            quoted = true;
        }
        let token = ["-Infinity", "Infinity", "NaN"]
            .into_iter()
            .find(|s| bytes[i..].starts_with(s.as_bytes()));
        if let Some(token) = token {
            let prior = out.iter().rfind(|b| !b.is_ascii_whitespace()).copied();
            let after = bytes.get(i + token.len()).copied();
            if !matches!(prior, Some(b':' | b'[' | b','))
                || !after.is_none_or(|b| b.is_ascii_whitespace() || matches!(b, b',' | b'}' | b']'))
            {
                return Err("invalid non-finite token".into());
            }
            out.push(b'"');
            out.extend_from_slice(token.as_bytes());
            out.push(b'"');
            i += token.len();
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|e| e.to_string())
}

struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON value with unique keys")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Unique, E> {
                Ok(Unique(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Unique, E> {
                Ok(Unique(Value::from(v)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Unique, E> {
                Ok(Unique(Value::from(v)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Unique, E> {
                Ok(Unique(Value::from(v)))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Unique, E> {
                Ok(Unique(Value::from(v)))
            }
            fn visit_none<E: de::Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut v = Vec::new();
                while let Some(x) = a.next_element::<Unique>()? {
                    v.push(x.0);
                }
                Ok(Unique(Value::Array(v)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut v = Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if v.contains_key(&k) {
                        return Err(de::Error::custom(format!("duplicate key {k}")));
                    }
                    v.insert(k, a.next_value::<Unique>()?.0);
                }
                Ok(Unique(Value::Object(v)))
            }
        }
        d.deserialize_any(V)
    }
}

fn parse(text: &str) -> Result<Value, String> {
    let text = normalize(text)?;
    serde_json::from_str::<Unique>(&text)
        .map(|v| v.0)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod framing_cost {
    use super::*;
    #[test]
    fn unfinished_cr_record_uses_linear_parse_work() {
        let mut input = b"::orx:: {\"m\":{\"data\":[".to_vec();
        while input.len() < MAX_FRAME - 10 {
            input.extend_from_slice(b"1,\r");
        }
        input.extend_from_slice(b"0]}}\n");
        PARSED_BYTES.with(|n| n.set(0));
        WORK_LIMIT.with(|limit| limit.set(input.len() * 4));
        Snapshot::new("r1", "1-100", "g1")
            .feed(&input, false, 0, false)
            .unwrap();
        assert!(PARSED_BYTES.with(|n| n.get()) < input.len() * 4);
        WORK_LIMIT.with(|limit| limit.set(usize::MAX));
    }
}
