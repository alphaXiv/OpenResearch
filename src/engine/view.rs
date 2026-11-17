//! The run view an agent writes: markup orx parses, checks and serves as a tree. The markup is
//! stored as written and the tree is derived from it on every read, never stored beside it.

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// The elements this orx draws; grows as the slices that feed new blocks land.
pub(crate) const ELEMENTS: &[&str] = &[
    "signal", "view", "chart", "series", "stats", "table", "text", "cells", "bars",
];

const MAX_BYTES: usize = 64 * 1024;
const MAX_BLOCKS: usize = 32;
const MAX_SIGNALS: usize = 64;
const MAX_SERIES: usize = 8;
const MAX_DEPTH: usize = 4;
const MAX_TEXT: usize = 2048;
const CELLS_WINDOW: usize = 256;

const SIGNAL: [&str; 12] = [
    "name", "label", "unit", "digits", "better", "smooth", "as", "scale", "sign", "pick", "values",
    "vs",
];
const LABELS: &str = "labels";
const BLOCK: [&str; 2] = ["phase", "title"];
const CHART: [&str; 9] = [
    "y", "x", "from", "by", "x-scale", "y-scale", "x-range", "mark", "ghost",
];
/// How a stats block or a signal compares with the parent's run: at the same stage and step, or
/// with the value the parent ended with.
const VS: [&str; 2] = ["step", "final"];

pub(super) fn outcome(value: &Value) -> &str {
    match value.as_str() {
        Some(word @ ("correct" | "wrong" | "error")) => word,
        _ => "unknown",
    }
}

fn text_signals(text: &str) -> Result<Vec<String>, String> {
    use super::watch::reply::{tokens, Token};

    let mut names = Vec::new();
    let mut end = 0;
    for (range, token) in tokens(text, false)? {
        if text[end..range.start].contains('}') {
            return Err("a closing } needs a matching { in <text>".into());
        }
        match token {
            Token::Signal(None, name) if orx_signals::identifier(&name) => names.push(name),
            _ => {
                return Err(format!(
                    "{} is not a signal token; use {{signal}} in <text>",
                    &text[range]
                ))
            }
        }
        end = range.end;
    }
    if text[end..].contains('}') {
        return Err("a closing } needs a matching { in <text>".into());
    }
    Ok(names)
}

/// Where in the markup something is wrong, and what.
#[derive(Debug)]
pub(crate) struct Problem {
    pub line: u32,
    pub column: u32,
    pub message: String,
}

impl Problem {
    /// The whole error an agent reads: where, what, and the command that fixes it.
    pub(crate) fn text(&self, source: &str, fix: &str) -> String {
        format!(
            "View {source}:{}:{}: {}. {fix}",
            self.line, self.column, self.message
        )
    }
}

#[derive(Debug, PartialEq)]
enum Kind {
    Signal,
    Table,
    Column(String),
}

/// A name the view draws, to be checked against what runs report.
#[derive(Debug)]
struct Reference {
    path: String,
    line: u32,
    column: u32,
    kind: Kind,
    name: String,
}

/// A view as parsed: its tree and every name it refers to.
#[derive(Debug)]
pub(crate) struct Compiled {
    pub tree: Value,
    references: Vec<Reference>,
    declared: Vec<String>,
}

/// What a run, or the runs of an experiment, reported: measures, and tables with their columns.
#[derive(Debug, Default)]
pub(crate) struct Seen {
    pub signals: Vec<String>,
    pub tables: BTreeMap<String, Vec<String>>,
}

impl Seen {
    fn has_signal(&self, name: &str) -> bool {
        self.signals.iter().any(|signal| signal == name)
            || self
                .tables
                .values()
                .any(|columns| columns.iter().any(|column| column == name))
    }

    pub(crate) fn of(snapshot: &orx_signals::Snapshot) -> Self {
        let columns: Vec<&String> = snapshot.row_columns.values().flatten().collect();
        Self {
            signals: snapshot
                .signal_order
                .iter()
                .filter(|name| !columns.contains(name))
                .cloned()
                .collect(),
            tables: snapshot
                .row_columns
                .iter()
                .filter(|(name, _)| snapshot.rows.contains_key(*name))
                .map(|(name, columns)| (name.clone(), columns.clone()))
                .collect(),
        }
    }

    pub(crate) fn add(&mut self, other: Seen) {
        for name in other.signals {
            if !self.signals.contains(&name) {
                self.signals.push(name);
            }
        }
        for (table, columns) in other.tables {
            let known = self.tables.entry(table).or_default();
            for column in columns {
                if !known.contains(&column) {
                    known.push(column);
                }
            }
        }
    }
}

fn listed<'a>(names: impl IntoIterator<Item = &'a String>) -> String {
    let names: Vec<&str> = names.into_iter().map(String::as_str).collect();
    match names.len() {
        0 => "nothing yet".into(),
        n if n > 24 => format!("{} and {} more", names[..24].join(", "), n - 24),
        _ => names.join(", "),
    }
}

impl Compiled {
    /// Every name the view draws that the runs did not report. A name is pending while nothing of
    /// its kind was reported (or, for a declared signal, until it is): it may arrive in a later
    /// stage. Returns the pending names, or the first name that cannot be right.
    pub(crate) fn check(&self, seen: &Seen) -> Result<Vec<String>, Problem> {
        let mut pending = Vec::new();
        for reference in &self.references {
            let problem = |message: String| Problem {
                line: reference.line,
                column: reference.column,
                message: format!("{}: {message}", reference.path),
            };
            let name = &reference.name;
            let found = match &reference.kind {
                Kind::Signal => seen.has_signal(name),
                Kind::Table => seen.tables.contains_key(name),
                Kind::Column(table) => seen
                    .tables
                    .get(table)
                    .is_some_and(|columns| columns.contains(name)),
            };
            if found {
                continue;
            }
            let reported = match &reference.kind {
                Kind::Signal => !seen.signals.is_empty() && !self.declared.contains(name),
                Kind::Table => !seen.tables.is_empty(),
                Kind::Column(table) => seen.tables.contains_key(table),
            };
            if !reported {
                if !pending.contains(name) {
                    pending.push(name.clone());
                }
                continue;
            }
            return Err(problem(match &reference.kind {
                Kind::Signal => format!("no signal \"{name}\"; seen: {}", listed(&seen.signals)),
                Kind::Table => format!("no table \"{name}\"; seen: {}", listed(seen.tables.keys())),
                Kind::Column(table) => format!(
                    "table \"{table}\" has no column \"{name}\"; seen: {}",
                    listed(&seen.tables[table])
                ),
            }));
        }
        Ok(pending)
    }
}

struct Parser<'a> {
    document: &'a roxmltree::Document<'a>,
    references: Vec<Reference>,
}

/// The markup is wrapped in this element, on the first line, to parse it as one document.
const WRAPPER: &str = "<orx>";

impl Parser<'_> {
    fn at(&self, position: usize) -> (u32, u32) {
        let at = self.document.text_pos_at(position);
        let column = if at.row == 1 {
            at.col.saturating_sub(WRAPPER.len() as u32)
        } else {
            at.col
        };
        (at.row, column)
    }

    fn problem(&self, node: roxmltree::Node, message: impl Into<String>) -> Problem {
        let (line, column) = self.at(node.range().start);
        Problem {
            line,
            column,
            message: message.into(),
        }
    }

    fn attribute_problem(&self, attribute: &roxmltree::Attribute, message: String) -> Problem {
        let (line, column) = self.at(attribute.range().start);
        Problem {
            line,
            column,
            message,
        }
    }

    fn refer(&mut self, node: roxmltree::Node, path: String, kind: Kind, name: &str) {
        let (line, column) = self.at(node.range().start);
        self.references.push(Reference {
            path,
            line,
            column,
            kind,
            name: name.to_owned(),
        });
    }

    /// The element's attributes, each one this element accepts.
    fn attributes<'n>(
        &self,
        node: roxmltree::Node<'n, 'n>,
        allowed: &[&str],
    ) -> Result<BTreeMap<&'n str, roxmltree::Attribute<'n, 'n>>, Problem> {
        let mut found = BTreeMap::new();
        for attribute in node.attributes() {
            if attribute.namespace().is_some() || !allowed.contains(&attribute.name()) {
                return Err(self.attribute_problem(
                    &attribute,
                    format!(
                        "unknown attribute \"{}\" on <{}>; allowed: {}",
                        attribute.name(),
                        node.tag_name().name(),
                        allowed.join(", ")
                    ),
                ));
            }
            found.insert(attribute.name(), attribute);
        }
        Ok(found)
    }

    fn names(&self, attribute: &roxmltree::Attribute, most: usize) -> Result<Vec<String>, Problem> {
        let names: Vec<String> = attribute
            .value()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        if names.is_empty() {
            return Err(self.attribute_problem(
                attribute,
                format!("{} is empty; name at least one", attribute.name()),
            ));
        }
        if names.len() > most {
            return Err(self.attribute_problem(
                attribute,
                format!(
                    "{} names {} signals; at most {most}",
                    attribute.name(),
                    names.len()
                ),
            ));
        }
        self.each_identifier(attribute, &names)?;
        Ok(names)
    }

    fn each_identifier(
        &self,
        attribute: &roxmltree::Attribute,
        names: &[String],
    ) -> Result<(), Problem> {
        match names.iter().find(|name| !orx_signals::identifier(name)) {
            Some(bad) => Err(self.attribute_problem(
                attribute,
                format!(
                    "\"{bad}\" in {} is not a name: use letters, digits and _, starting with a letter or _ (at most 64)",
                    attribute.name()
                ),
            )),
            None => Ok(()),
        }
    }

    fn name(&self, attribute: &roxmltree::Attribute) -> Result<String, Problem> {
        let name = attribute.value().trim().to_owned();
        self.each_identifier(attribute, std::slice::from_ref(&name))?;
        Ok(name)
    }

    fn text(&self, attribute: &roxmltree::Attribute, most: usize) -> Result<String, Problem> {
        let text = attribute.value().trim();
        if text.is_empty() || text.chars().count() > most {
            return Err(self.attribute_problem(
                attribute,
                format!("{} must be 1 to {most} characters", attribute.name()),
            ));
        }
        Ok(text.to_owned())
    }

    fn choice(
        &self,
        attribute: &roxmltree::Attribute,
        choices: &[&str],
    ) -> Result<String, Problem> {
        let value = attribute.value();
        if choices.contains(&value) {
            return Ok(value.to_owned());
        }
        Err(self.attribute_problem(
            attribute,
            format!(
                "{}=\"{value}\" is not one of {}",
                attribute.name(),
                choices.join(", ")
            ),
        ))
    }

    fn number(&self, attribute: &roxmltree::Attribute) -> Result<f64, Problem> {
        let value = attribute.value().trim();
        let parsed = value
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && !value.contains(['i', 'I', 'n', 'N']));
        parsed.ok_or_else(|| {
            self.attribute_problem(
                attribute,
                format!("{}=\"{value}\" is not a finite number", attribute.name()),
            )
        })
    }

    fn integer(
        &self,
        attribute: &roxmltree::Attribute,
        range: std::ops::RangeInclusive<u64>,
    ) -> Result<u64, Problem> {
        let value = attribute.value().trim();
        value
            .parse::<u64>()
            .ok()
            .filter(|n| range.contains(n))
            .ok_or_else(|| {
                self.attribute_problem(
                    attribute,
                    format!(
                        "{}=\"{value}\" must be a whole number from {} to {}",
                        attribute.name(),
                        range.start(),
                        range.end()
                    ),
                )
            })
    }

    fn object(&self, attribute: &roxmltree::Attribute) -> Result<Map<String, Value>, Problem> {
        let bad = |why: &str| {
            self.attribute_problem(
                attribute,
                format!(
                    "{} must be a JSON object of strings, like '{{\"1\":\"valid\"}}' ({why})",
                    attribute.name()
                ),
            )
        };
        let object = match serde_json::from_str::<Value>(attribute.value()) {
            Ok(Value::Object(object)) => object,
            Ok(_) => return Err(bad("not an object")),
            Err(error) => return Err(bad(&error.to_string())),
        };
        if object.len() > 64 {
            return Err(bad("more than 64 entries"));
        }
        if object.iter().any(|(key, value)| {
            key.len() > 64 || value.as_str().is_none_or(|v| v.chars().count() > 64)
        }) {
            return Err(bad("each key and word at most 64 characters"));
        }
        Ok(object)
    }

    fn signal(&self, node: roxmltree::Node) -> Result<(String, Value), Problem> {
        let mut allowed = SIGNAL.to_vec();
        allowed.push(LABELS);
        let attributes = self.attributes(node, &allowed)?;
        let Some(name) = attributes.get("name") else {
            return Err(self.problem(node, "<signal> needs name=\"…\", the name the run reports"));
        };
        let name = self.name(name)?;
        let mut spec = Map::new();
        spec.insert("label".into(), json!(name));
        for key in SIGNAL.iter().skip(1).copied().chain([LABELS]) {
            let Some(attribute) = attributes.get(key) else {
                continue;
            };
            let value = match key {
                "label" => json!(self.text(attribute, 80)?),
                "unit" => json!(self.text(attribute, 16)?),
                "digits" => json!(self.integer(attribute, 0..=12)?),
                "better" => json!(self.choice(attribute, &["lower", "higher"])?),
                "smooth" => {
                    let smooth = self.number(attribute)?;
                    if !(smooth > 0.0 && smooth <= 1.0) {
                        return Err(self.attribute_problem(
                            attribute,
                            format!(
                                "smooth=\"{}\" must be above 0 and at most 1",
                                attribute.value()
                            ),
                        ));
                    }
                    json!(smooth)
                }
                "as" => json!(self.choice(attribute, &["duration", "compact"])?),
                "scale" => json!(self.number(attribute)?),
                "sign" => json!(self.choice(attribute, &["true", "false"])? == "true"),
                "pick" => json!(self.choice(attribute, &["latest", "best"])?),
                "vs" => json!(self.choice(attribute, &VS)?),
                "values" => {
                    let values = self.object(attribute)?;
                    let outcomes = ["correct", "wrong", "error"];
                    if let Some((raw, word)) = values
                        .iter()
                        .find(|(_, word)| !outcomes.contains(&word.as_str().unwrap_or_default()))
                    {
                        return Err(self.attribute_problem(
                            attribute,
                            format!(
                                "values maps \"{raw}\" to {word}, but values says which outcome each value is: correct, wrong or error; to show a word for a value, use labels"
                            ),
                        ));
                    }
                    Value::Object(values)
                }
                _ => Value::Object(self.object(attribute)?),
            };
            spec.insert(key.into(), value);
        }
        if spec.get("pick").is_some_and(|pick| pick == "best") && !spec.contains_key("better") {
            return Err(self.problem(
                node,
                format!("signal \"{name}\" picks its best value but declares no better=\"lower|higher\""),
            ));
        }
        Ok((name, Value::Object(spec)))
    }

    fn block(&mut self, node: roxmltree::Node, index: usize) -> Result<Value, Problem> {
        let kind = node.tag_name().name();
        let path = format!("blocks[{index}]");
        let allowed: Vec<&str> = match kind {
            "chart" => CHART.iter().chain(BLOCK.iter()).copied().collect(),
            "stats" => ["of", "vs"].iter().chain(BLOCK.iter()).copied().collect(),
            "text" => BLOCK.to_vec(),
            "cells" => ["from"].into_iter().chain(BLOCK).collect(),
            "bars" => ["from", "by"].into_iter().chain(BLOCK).collect(),
            "table" => ["name", "columns", "last", "vs"]
                .iter()
                .chain(BLOCK.iter())
                .copied()
                .collect(),
            "signal" => return Err(self.problem(
                node,
                "<signal> goes before <view>, not inside it: move it above <view> and keep it, \
                     since its label, unit and better reach every block",
            )),
            "series" => return Err(self.problem(node, "<series> goes inside a <chart>")),
            _ => {
                return Err(self.problem(
                    node,
                    format!("<{kind}> is not a block; a view holds chart, stats, table, text, cells or bars"),
                ))
            }
        };
        let attributes = self.attributes(node, &allowed)?;
        let mut block = Map::new();
        let children: Vec<roxmltree::Node> = node.children().filter(|n| n.is_element()).collect();
        if kind != "chart" {
            if let Some(child) = children.first() {
                return Err(self.problem(
                    *child,
                    if kind == "text" {
                        "<text> holds plain text; remove the nested element".into()
                    } else {
                        format!("<{kind}> holds nothing; write it as <{kind} … />")
                    },
                ));
            }
        }
        match kind {
            "chart" => self.chart(node, &attributes, &children, &path, &mut block)?,
            "stats" => {
                let Some(of) = attributes.get("of") else {
                    return Err(self.problem(node, "<stats> needs of=\"…\", the signals it shows"));
                };
                let names = self.names(of, 32)?;
                for (i, name) in names.iter().enumerate() {
                    self.refer(node, format!("{path}.stats[{i}]"), Kind::Signal, name);
                }
                block.insert("stats".into(), json!(names));
                if let Some(vs) = attributes.get("vs") {
                    block.insert("vs".into(), json!(self.choice(vs, &VS)?));
                }
            }
            "text" => {
                let content: String = node
                    .children()
                    .filter(|child| child.is_text())
                    .filter_map(|child| child.text())
                    .collect();
                let text = content.trim();
                if text.is_empty() || text.len() > MAX_TEXT {
                    return Err(self.problem(
                        node,
                        format!("<text> needs 1 to {MAX_TEXT} bytes of plain text"),
                    ));
                }
                let names = text_signals(text).map_err(|message| self.problem(node, message))?;
                for name in names {
                    self.refer(node, format!("{path}.text"), Kind::Signal, &name);
                }
                block.insert("text".into(), json!(text));
            }
            "cells" | "bars" => {
                let Some(from) = attributes.get("from") else {
                    return Err(self.problem(
                        node,
                        format!("<{kind}> needs from=\"…\", the row table the run reports"),
                    ));
                };
                let table = self.name(from)?;
                self.refer(node, format!("{path}.{kind}"), Kind::Table, &table);
                self.refer(
                    node,
                    format!("{path}.outcome"),
                    Kind::Column(table.clone()),
                    "outcome",
                );
                if kind == "bars" {
                    let Some(by) = attributes.get("by") else {
                        return Err(self.problem(
                            node,
                            "<bars> needs by=\"…\", the column holding its categories",
                        ));
                    };
                    let column = self.name(by)?;
                    self.refer(
                        node,
                        format!("{path}.by"),
                        Kind::Column(table.clone()),
                        &column,
                    );
                    block.insert("by".into(), json!(column));
                }
                block.insert(kind.into(), json!(table));
            }
            _ => {
                let Some(name) = attributes.get("name") else {
                    return Err(
                        self.problem(node, "<table> needs name=\"…\", the row the run reports")
                    );
                };
                let Some(columns) = attributes.get("columns") else {
                    return Err(
                        self.problem(node, "<table> needs columns=\"…\", the columns it shows")
                    );
                };
                let name = self.name(name)?;
                let columns = self.names(columns, 64)?;
                self.refer(node, format!("{path}.table"), Kind::Table, &name);
                for (i, column) in columns.iter().enumerate() {
                    self.refer(
                        node,
                        format!("{path}.columns[{i}]"),
                        Kind::Column(name.clone()),
                        column,
                    );
                }
                block.insert("table".into(), json!(name));
                block.insert("columns".into(), json!(columns));
                if let Some(last) = attributes.get("last") {
                    block.insert("last".into(), json!(self.integer(last, 1..=2048)?));
                }
                if let Some(vs) = attributes.get("vs") {
                    block.insert(
                        "vs".into(),
                        json!(self.choice(vs, &["parent", "step", "final"])?),
                    );
                }
            }
        }
        if let Some(phase) = attributes.get("phase") {
            block.insert("phase".into(), json!(self.text(phase, 128)?));
        }
        if let Some(title) = attributes.get("title") {
            block.insert("title".into(), json!(self.text(title, 120)?));
        }
        Ok(Value::Object(block))
    }

    fn chart(
        &mut self,
        node: roxmltree::Node,
        attributes: &BTreeMap<&str, roxmltree::Attribute>,
        children: &[roxmltree::Node],
        path: &str,
        block: &mut Map<String, Value>,
    ) -> Result<(), Problem> {
        let from = attributes.get("from").map(|a| self.name(a)).transpose()?;
        let ghost = attributes
            .get("ghost")
            .map(|ghost| self.choice(ghost, &["parent", "false"]))
            .transpose()?
            .unwrap_or_else(|| "parent".into());
        let column = |table: &Option<String>| match table {
            Some(table) => Kind::Column(table.clone()),
            None => Kind::Signal,
        };
        let y = attributes
            .get("y")
            .map(|y| self.names(y, MAX_SERIES))
            .transpose()?;
        if y.is_some() == !children.is_empty() {
            return Err(self.problem(
                node,
                if y.is_some() {
                    "a chart takes y=\"…\" or <series> inside it, not both"
                } else {
                    "<chart> needs y=\"…\", the signals it draws, or <series x=\"…\" y=\"…\"/> inside it"
                },
            ));
        }
        if let Some(table) = &from {
            self.refer(node, format!("{path}.from"), Kind::Table, table);
        }
        for (i, name) in y.iter().flatten().enumerate() {
            self.refer(node, format!("{path}.chart[{i}]"), column(&from), name);
        }
        block.insert("chart".into(), json!(y.unwrap_or_default()));
        if let Some(x) = attributes.get("x") {
            let name = self.name(x)?;
            if from.is_none() && !matches!(name.as_str(), "step" | "elapsed") {
                return Err(self.attribute_problem(
                    x,
                    format!(
                        "x=\"{name}\" is not the run's step: a chart over another x draws from a table, so add from=\"TABLE\"; or leave x out to draw over the run's steps"
                    ),
                ));
            }
            if !children.is_empty() {
                return Err(self.attribute_problem(
                    x,
                    "each <series> names its own x, so the chart takes none".into(),
                ));
            }
            if from.is_some() {
                self.refer(node, format!("{path}.x"), column(&from), &name);
            }
            block.insert("x".into(), json!(name));
        } else if from.is_some() && children.is_empty() {
            return Err(self.problem(
                node,
                "a chart from a table needs x=\"COLUMN\", the column across the bottom",
            ));
        }
        if let Some(table) = &from {
            block.insert("from".into(), json!(table));
        }
        if !children.is_empty() {
            if from.is_none() {
                return Err(self.problem(
                    children[0],
                    "<series> draws columns of a table: add from=\"TABLE\" to the chart",
                ));
            }
            if children.len() > MAX_SERIES {
                return Err(self.problem(
                    children[MAX_SERIES],
                    format!("a chart draws at most {MAX_SERIES} series"),
                ));
            }
            let mut series = Vec::new();
            for (i, child) in children.iter().enumerate() {
                if child.tag_name().name() != "series" {
                    return Err(self.problem(
                        *child,
                        format!(
                            "<{}> cannot go inside <chart>; only <series>",
                            child.tag_name().name()
                        ),
                    ));
                }
                if let Some(inner) = child.children().find(|n| n.is_element()) {
                    return Err(
                        self.problem(inner, "<series> holds nothing; write it as <series … />")
                    );
                }
                let attributes = self.attributes(*child, &["x", "y", "label", "ghost"])?;
                let (Some(x), Some(y)) = (attributes.get("x"), attributes.get("y")) else {
                    return Err(
                        self.problem(*child, "<series> needs x=\"COLUMN\" and y=\"COLUMN\"")
                    );
                };
                let (x, y) = (self.name(x)?, self.name(y)?);
                self.refer(*child, format!("{path}.series[{i}].x"), column(&from), &x);
                self.refer(*child, format!("{path}.series[{i}].y"), column(&from), &y);
                let mut one = json!({"x": x, "y": y});
                if let Some(label) = attributes.get("label") {
                    one["label"] = json!(self.text(label, 80)?);
                }
                if let Some(drawn) = attributes.get("ghost") {
                    one["ghost"] = json!(self.choice(drawn, &["true", "false"])? == "true");
                }
                series.push(one);
            }
            block.insert("series".into(), json!(series));
        }
        if let Some(by) = attributes.get("by") {
            let Some(table) = &from else {
                return Err(self.attribute_problem(
                    by,
                    "by=\"COLUMN\" splits a table's rows: add from=\"TABLE\"".into(),
                ));
            };
            if !children.is_empty() {
                return Err(self.attribute_problem(
                    by,
                    "a chart splits its rows by a column or names its <series>, not both".into(),
                ));
            }
            let name = self.name(by)?;
            self.refer(
                node,
                format!("{path}.by"),
                Kind::Column(table.clone()),
                &name,
            );
            block.insert("by".into(), json!(name));
        }
        for (attribute, key) in [("x-scale", "xScale"), ("y-scale", "yScale")] {
            if let Some(scale) = attributes.get(attribute) {
                if self.choice(scale, &["linear", "log"])? == "log" {
                    block.insert(key.into(), json!("log"));
                }
            }
        }
        if let Some(range) = attributes.get("x-range") {
            if from.is_some() {
                return Err(self.attribute_problem(
                    range,
                    "x-range sets how far a chart over the run's steps reaches; a chart from a table spans its points".into(),
                ));
            }
            if self.choice(range, &["run", "parent"])? == "parent" {
                block.insert("xRange".into(), json!("parent"));
            }
        }
        if let Some(mark) = attributes.get("mark") {
            block.insert(
                "mark".into(),
                json!(self.choice(mark, &["line", "points", "bars"])?),
            );
        }
        block.insert("ghost".into(), json!(ghost));
        Ok(())
    }
}

/// Parses and checks the markup's own rules; [`Compiled::check`] then checks its names against
/// what runs reported.
pub(crate) fn compile(markup: &str) -> Result<Compiled, Problem> {
    let first = |message: String| Problem {
        line: 1,
        column: 1,
        message,
    };
    if markup.len() > MAX_BYTES {
        return Err(first(format!(
            "the view is {} bytes; at most {MAX_BYTES}",
            markup.len()
        )));
    }
    if let Some(at) = ["<!DOCTYPE", "<!ENTITY", "<?xml"]
        .iter()
        .filter_map(|token| markup.find(token))
        .min()
    {
        let line = markup[..at].matches('\n').count() as u32 + 1;
        let column = markup[..at]
            .rsplit('\n')
            .next()
            .map_or(0, |l| l.chars().count()) as u32
            + 1;
        return Err(Problem {
            line,
            column,
            message: "a view has no XML declaration, DTD or entities".into(),
        });
    }
    let text = format!("{WRAPPER}{markup}</orx>");
    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: 4096,
        ..Default::default()
    };
    let document = match roxmltree::Document::parse_with_options(&text, options) {
        Ok(document) => document,
        Err(error) => {
            let at = error.pos();
            let column = if at.row == 1 {
                at.col.saturating_sub(WRAPPER.len() as u32).max(1)
            } else {
                at.col
            };
            let message = error.to_string();
            let message = message
                .strip_suffix(&format!(" at {at}"))
                .unwrap_or(&message);
            return Err(Problem {
                line: at.row,
                column,
                message: format!("the markup is not well formed: {message}"),
            });
        }
    };
    let mut parser = Parser {
        document: &document,
        references: Vec::new(),
    };
    for node in document.descendants() {
        let depth = node.ancestors().filter(|n| n.is_element()).count();
        if node.is_pi() {
            return Err(parser.problem(node, "processing instructions are not allowed in a view"));
        }
        if node.is_text()
            && node.text().is_some_and(|t| !t.trim().is_empty())
            && node
                .parent_element()
                .is_none_or(|parent| !parent.has_tag_name("text"))
        {
            return Err(parser.problem(
                node,
                "text is not allowed in a view; put words in label=\"…\" or title=\"…\"",
            ));
        }
        if !node.is_element() || node == document.root_element() {
            continue;
        }
        let name = node.tag_name().name();
        if node.tag_name().namespace().is_some() || !ELEMENTS.contains(&name) {
            return Err(parser.problem(
                node,
                format!(
                    "unknown element \"{name}\"; allowed: {}",
                    ELEMENTS.join(", ")
                ),
            ));
        }
        if depth > MAX_DEPTH {
            return Err(parser.problem(node, format!("a view nests at most {MAX_DEPTH} deep")));
        }
    }

    let top: Vec<roxmltree::Node> = document
        .root_element()
        .children()
        .filter(|n| n.is_element())
        .collect();
    let mut signals = Map::new();
    let mut view = None;
    for node in &top {
        match node.tag_name().name() {
            "signal" => {
                if let Some(child) = node.children().find(|n| n.is_element()) {
                    return Err(
                        parser.problem(child, "<signal> holds nothing; write it as <signal … />")
                    );
                }
                let (name, spec) = parser.signal(*node)?;
                if signals.contains_key(&name) {
                    return Err(
                        parser.problem(*node, format!("signal \"{name}\" is declared twice"))
                    );
                }
                if signals.len() == MAX_SIGNALS {
                    return Err(parser.problem(
                        *node,
                        format!("a view declares at most {MAX_SIGNALS} signals"),
                    ));
                }
                signals.insert(name, spec);
            }
            "view" if view.is_some() => {
                return Err(parser.problem(*node, "a view has exactly one <view>"));
            }
            "view" => view = Some(*node),
            other => {
                return Err(parser.problem(
                    *node,
                    format!("<{other}> goes inside <view>; only <signal> and <view> stand alone"),
                ))
            }
        }
    }
    let Some(view) = view else {
        return Err(first(
            "a view needs one <view> holding its blocks, after its <signal> declarations".into(),
        ));
    };
    let attributes = parser.attributes(view, &["headline"])?;
    let headline = match attributes.get("headline") {
        Some(headline) => parser.names(headline, 8)?,
        None => Vec::new(),
    };
    for (i, name) in headline.iter().enumerate() {
        parser.refer(view, format!("headline[{i}]"), Kind::Signal, name);
    }
    let blocks: Vec<roxmltree::Node> = view.children().filter(|n| n.is_element()).collect();
    if blocks.len() > MAX_BLOCKS {
        return Err(parser.problem(
            blocks[MAX_BLOCKS],
            format!("a view holds at most {MAX_BLOCKS} blocks"),
        ));
    }
    let blocks = blocks
        .iter()
        .enumerate()
        .map(|(index, node)| parser.block(*node, index))
        .collect::<Result<Vec<_>, _>>()?;

    let declared = signals.keys().cloned().collect();
    Ok(Compiled {
        tree: json!({"signals": signals, "headline": headline, "blocks": blocks}),
        references: parser.references,
        declared,
    })
}

/// The markup that draws `tree`, for a view orx derived rather than one an agent wrote.
pub(crate) fn markup(tree: &Value) -> String {
    let escape = |text: &str| {
        text.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
    };
    let names = |value: &Value| {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut lines = Vec::new();
    for (name, spec) in tree["signals"].as_object().into_iter().flatten() {
        let mut line = format!("<signal name=\"{name}\"");
        for (key, value) in spec.as_object().into_iter().flatten() {
            if key == "label" && value == name {
                continue;
            }
            let text = match value {
                Value::String(text) => text.clone(),
                Value::Object(_) => value.to_string(),
                other => other.to_string(),
            };
            line.push_str(&format!(" {key}=\"{}\"", escape(&text)));
        }
        lines.push(format!("{line} />"));
    }
    let headline = names(&tree["headline"]);
    lines.push(if headline.is_empty() {
        "<view>".into()
    } else {
        format!("<view headline=\"{headline}\">")
    });
    for block in tree["blocks"].as_array().into_iter().flatten() {
        let mut line = String::new();
        let attribute = |line: &mut String, key: &str, value: &Value| {
            let text = match value {
                Value::String(text) => escape(text),
                Value::Array(_) => names(value),
                other => other.to_string(),
            };
            line.push_str(&format!(" {key}=\"{text}\""));
        };
        if let Some(chart) = block.get("chart") {
            line.push_str("  <chart");
            if chart.as_array().is_some_and(|y| !y.is_empty()) {
                attribute(&mut line, "y", chart);
            }
            for (key, name) in [
                ("x", "x"),
                ("from", "from"),
                ("by", "by"),
                ("xScale", "x-scale"),
                ("yScale", "y-scale"),
                ("xRange", "x-range"),
                ("mark", "mark"),
                ("ghost", "ghost"),
            ] {
                if let Some(value) = block.get(key) {
                    attribute(&mut line, name, value);
                }
            }
        } else if let Some(stats) = block.get("stats") {
            line.push_str("  <stats");
            attribute(&mut line, "of", stats);
            if let Some(vs) = block.get("vs") {
                attribute(&mut line, "vs", vs);
            }
        } else if let Some(table) = block.get("table") {
            line.push_str("  <table");
            attribute(&mut line, "name", table);
            for key in ["columns", "last", "vs"] {
                if let Some(value) = block.get(key) {
                    attribute(&mut line, key, value);
                }
            }
        }
        for key in BLOCK {
            if let Some(value) = block.get(key) {
                attribute(&mut line, key, value);
            }
        }
        match block["series"].as_array() {
            Some(series) => {
                lines.push(format!("{line}>"));
                for one in series {
                    let mut child = "    <series".to_owned();
                    for key in ["x", "y", "label", "ghost"] {
                        if let Some(value) = one.get(key) {
                            attribute(&mut child, key, value);
                        }
                    }
                    lines.push(format!("{child} />"));
                }
                lines.push("  </chart>".into());
            }
            None => lines.push(format!("{line} />")),
        }
    }
    if lines.last().is_some_and(|line| line.starts_with("<view")) {
        let open = lines.pop().unwrap_or_default();
        lines.push(open.replace('>', " />"));
    } else {
        lines.push("</view>".into());
    }
    lines.join("\n") + "\n"
}

/// The experiment's monitor settings' version, for `If-Match`: changes with every write.
pub(crate) fn etag(raw: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(raw.unwrap_or_default().as_bytes());
    format!("\"{}\"", &format!("{digest:x}")[..16])
}

/// The view's markup in the experiment's stored monitor settings.
pub(crate) fn stored(experiment: &str, raw: Option<&str>) -> crate::error::Result<Option<String>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let settings: Value = serde_json::from_str(raw).map_err(|error| {
        crate::error::anyhow!("Experiment {experiment}'s run view settings are unreadable: {error}")
    })?;
    Ok(settings["view"].as_str().map(str::to_owned))
}

/// The patterns in the experiment's stored monitor settings.
pub(crate) fn stored_patterns(
    experiment: &str,
    raw: Option<&str>,
) -> crate::error::Result<orx_signals::Config> {
    let Some(raw) = raw else {
        return Ok(orx_signals::Config::default());
    };
    let settings: Value = serde_json::from_str(raw).map_err(|error| {
        crate::error::anyhow!("Experiment {experiment}'s run view settings are unreadable: {error}")
    })?;
    let patterns = serde_json::from_value(settings.get("patterns").cloned().unwrap_or(json!([])))
        .map_err(|error| {
        crate::error::anyhow!("Experiment {experiment}'s patterns are unreadable: {error}")
    })?;
    Ok(orx_signals::Config {
        patterns,
        ..Default::default()
    })
}

/// The stage plan in the experiment's stored monitor settings; empty when none is declared.
pub(crate) fn stored_plan(
    experiment: &str,
    raw: Option<&str>,
) -> crate::error::Result<Vec<orx_signals::Stage>> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let settings: Value = serde_json::from_str(raw).map_err(|error| {
        crate::error::anyhow!("Experiment {experiment}'s run view settings are unreadable: {error}")
    })?;
    match settings.get("plan") {
        None => Ok(Vec::new()),
        Some(plan) => orx_signals::plan(plan, false).map_err(|error| {
            crate::error::anyhow!("Experiment {experiment}'s plan is unreadable: {error}")
        }),
    }
}

/// The keys `--config` takes; this orx reads no other.
const CONFIG_KEYS: [&str; 4] = ["plan", "rules", "brief", "watch"];

/// What the watcher's brief may say, by label: each label's text at most a few sentences.
pub(crate) const BRIEF_LABELS: [&str; 6] = [
    "question",
    "healthy",
    "noise",
    "worth_a_flag",
    "from_parent",
    "decided",
];
/// A brief's whole text, in UTF-8 bytes: about 800 tokens.
const MAX_BRIEF: usize = 3200;
/// The bounds of the watcher's gaps, in seconds: `minGapS` between looks a change brings,
/// `maxGapS` the longest silence while a run keeps printing.
const GAPS: [(&str, u64, u64); 2] = [("minGapS", 30, 3600), ("maxGapS", 60, 86_400)];

/// What a configuration an agent gave sets, by key; a key left out is `None`.
#[derive(Debug, Default)]
pub(crate) struct Config {
    pub plan: Option<Vec<orx_signals::Stage>>,
    pub rules: Option<Vec<super::rules::Rule>>,
    /// An empty brief clears it.
    pub brief: Option<Map<String, Value>>,
    /// An empty object clears it.
    pub watch: Option<Map<String, Value>>,
}

/// A configuration as an agent gave it, or why it cannot be used.
pub(crate) fn config_of(given: &Value, source: &Source, fix: &str) -> Result<Config, Refusal> {
    let file = source.config.unwrap_or("<config>");
    let refuse = |why: String| Refusal::Invalid(format!("Config {file}: {why}. {fix}"));
    let keys = format!("{} and {}", CONFIG_KEYS[..3].join(", "), CONFIG_KEYS[3]);
    let fields = given
        .as_object()
        .filter(|fields| !fields.is_empty())
        .ok_or_else(|| refuse(format!("give a JSON object with {keys}")))?;
    if let Some(key) = fields
        .keys()
        .find(|key| !CONFIG_KEYS.contains(&key.as_str()))
    {
        return Err(refuse(format!(
            "\"{key}\" is not something this orx reads; a config takes {keys}"
        )));
    }
    let plan = fields
        .get("plan")
        .map(|plan| orx_signals::plan(plan, true).map_err(&refuse))
        .transpose()?;
    let rules = fields
        .get("rules")
        .map(super::rules::parse)
        .transpose()
        .map_err(&refuse)?;
    let brief = fields
        .get("brief")
        .map(|brief| {
            let lines = brief.as_object().ok_or_else(|| {
                refuse(format!(
                    "a brief is an object of text by label: {}",
                    BRIEF_LABELS.join(", ")
                ))
            })?;
            if let Some(label) = lines
                .keys()
                .find(|label| !BRIEF_LABELS.contains(&label.as_str()))
            {
                return Err(refuse(format!(
                    "the brief has no label \"{label}\"; it takes {}",
                    BRIEF_LABELS.join(", ")
                )));
            }
            if let Some((label, _)) = lines.iter().find(|(_, text)| !text.is_string()) {
                return Err(refuse(format!("the brief's \"{label}\" is text")));
            }
            let size: usize = lines.values().filter_map(Value::as_str).map(str::len).sum();
            if size > MAX_BRIEF {
                return Err(refuse(format!(
                    "the brief is {size} bytes; keep it to {MAX_BRIEF}, about 800 tokens"
                )));
            }
            Ok(lines.clone())
        })
        .transpose()?;
    let watch = fields
        .get("watch")
        .map(|watch| {
            let gaps = watch.as_object().ok_or_else(|| {
                refuse("watch is an object: {\"minGapS\": N, \"maxGapS\": N}".into())
            })?;
            for (key, value) in gaps {
                let (_, low, high) =
                    GAPS.iter().find(|(name, ..)| name == key).ok_or_else(|| {
                        refuse(format!(
                            "watch has no \"{key}\"; it takes minGapS and maxGapS"
                        ))
                    })?;
                if !value.as_u64().is_some_and(|n| (*low..=*high).contains(&n)) {
                    return Err(refuse(format!(
                        "watch's {key} is a whole number of seconds from {low} to {high}"
                    )));
                }
            }
            if let (Some(min), Some(max)) = (gaps.get("minGapS"), gaps.get("maxGapS")) {
                if min.as_u64() > max.as_u64() {
                    return Err(refuse("watch's minGapS is longer than its maxGapS".into()));
                }
            }
            Ok(gaps.clone())
        })
        .transpose()?;
    Ok(Config {
        plan,
        rules,
        brief,
        watch,
    })
}

/// Patterns as an agent gave them, compiled, or why they cannot be used.
pub(crate) fn patterns(
    given: &Value,
    source: &Source,
    fix: &str,
) -> Result<
    (
        orx_signals::Config,
        Option<std::sync::Arc<orx_signals::Patterns>>,
    ),
    Refusal,
> {
    let file = source.patterns.unwrap_or("<patterns>");
    let patterns = serde_json::from_value(given.clone()).map_err(|error| {
        Refusal::Invalid(format!(
            "Pattern {file}: give a JSON list of {{\"regex\", \"phase\", \"unit\", \"row\"}} objects ({error}). {fix}"
        ))
    })?;
    let config = orx_signals::Config {
        patterns,
        ..Default::default()
    };
    let compiled = orx_signals::Patterns::compile(&config).map_err(|error| {
        let at = match error.column {
            Some(column) => format!("invalid regex at column {column}: "),
            None => String::new(),
        };
        Refusal::Invalid(format!(
            "Pattern {file}: pattern {}: {at}{}. {fix}",
            error.index + 1,
            error.reason
        ))
    })?;
    let compiled = (!compiled.is_empty()).then(|| std::sync::Arc::new(compiled));
    Ok((config, compiled))
}

/// Why a view request was refused, each with the status the API answers it with.
#[derive(Debug)]
pub(crate) enum Refusal {
    Missing(String),
    Invalid(String),
    Changed(String),
    Failed(String),
}

impl From<crate::error::Error> for Refusal {
    fn from(error: crate::error::Error) -> Self {
        Self::Failed(format!("{error:#}"))
    }
}

/// How an error names the markup and the command that fixes it: the file it came from, else
/// standard input.
pub(crate) struct Source<'a> {
    pub file: Option<&'a str>,
    /// The file the patterns came from.
    pub patterns: Option<&'a str>,
    /// The file the configuration came from.
    pub config: Option<&'a str>,
}

impl Source<'_> {
    fn name(&self) -> &str {
        self.file.unwrap_or("<stdin>")
    }

    fn flag(&self) -> String {
        match self.file {
            Some(file) => format!("--file {file}"),
            None => "--stdin".into(),
        }
    }
}

const NEWEST_RUNS: usize = 16;

/// What the experiment's newest runs reported, with the newest run that reported anything.
fn seen_on(
    store: &crate::store::Store,
    experiment: &str,
) -> crate::error::Result<(Seen, Option<String>)> {
    let mut seen = Seen::default();
    let mut newest = None;
    for run in store
        .list_runs_by_experiment(experiment)?
        .iter()
        .take(NEWEST_RUNS)
    {
        if let Some(snapshot) = super::monitor::checkpoint(store, &run.id)? {
            newest.get_or_insert_with(|| run.id.clone());
            seen.add(Seen::of(&snapshot));
        }
    }
    Ok((seen, newest))
}

fn experiment_of(
    store: &crate::store::Store,
    id: &str,
) -> Result<crate::local::model::LocalExperiment, Refusal> {
    store
        .get_local_experiment(id)?
        .ok_or_else(|| Refusal::Missing(format!("Experiment {id} not found.")))
}

/// `GET /api/experiments/:id/view`: the saved markup and its tree, or the default orx draws
/// from the newest run's first signal.
pub(crate) fn described(store: &crate::store::Store, id: &str) -> Result<Value, Refusal> {
    let mut described = view_of(store, id)?;
    let raw = store.run_monitor_json(id)?;
    described["patterns"] = json!(stored_patterns(id, raw.as_deref())?.patterns);
    described["plan"] = json!(stored_plan(id, raw.as_deref())?);
    let rules = super::rules::stored(id, raw.as_deref())?;
    described["rules"] = json!(rules
        .iter()
        .map(super::rules::Rule::json)
        .collect::<Vec<_>>());
    let settings: Value = raw
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or(Value::Null);
    for key in ["brief", "watch"] {
        if settings[key].is_object() {
            described[key] = settings[key].clone();
        }
    }
    Ok(described)
}

fn view_of(store: &crate::store::Store, id: &str) -> Result<Value, Refusal> {
    experiment_of(store, id)?;
    let raw = store.run_monitor_json(id)?;
    let etag = etag(raw.as_deref());
    if let Some(markup) = stored(id, raw.as_deref())? {
        let tree = compile(&markup)
            .map_err(|problem| {
                Refusal::Failed(problem.text(
                    &format!("of {id}"),
                    &format!("Run `orx exp view set {id} --file PATH` with a view this orx reads."),
                ))
            })?
            .tree;
        return Ok(
            json!({"markup": markup, "tree": tree, "etag": etag, "fromExperimentId": id, "isDefault": false}),
        );
    }
    let mut tree = super::monitor::default_view(None);
    for run in store.list_runs_by_experiment(id)?.iter().take(NEWEST_RUNS) {
        if let Some(snapshot) = super::monitor::checkpoint(store, &run.id)? {
            tree = super::monitor::default_view(Some(&snapshot));
            break;
        }
    }
    Ok(
        json!({"markup": null, "derived": markup(&tree), "tree": tree, "etag": etag, "fromExperimentId": id, "isDefault": true}),
    )
}

/// `PUT /api/experiments/:id/view`: saves the view's `markup` once it parses and every name it
/// draws is one the experiment's runs reported or may still report, and `patterns` once each
/// compiles; what is not given is kept. Its runs are drawn again from it.
pub(crate) fn set(
    store: &crate::store::Store,
    id: &str,
    expected: &str,
    markup: Option<&str>,
    given: Option<&Value>,
    configured: Option<&Value>,
    source: &Source,
) -> Result<Value, Refusal> {
    experiment_of(store, id)?;
    let (seen, newest) = seen_on(store, id)?;
    let check_fix = |flag: String| match &newest {
        Some(run) => format!("Run `orx exp view check {run} {flag}`."),
        None => format!("Run `orx exp view set {id} {flag}`."),
    };
    let mut pending = Vec::new();
    if let Some(markup) = markup {
        let compiled = compile(markup).map_err(|problem| {
            Refusal::Invalid(problem.text(source.name(), &check_fix(source.flag())))
        })?;
        pending = compiled.check(&seen).map_err(|problem| {
            Refusal::Invalid(problem.text(
                source.name(),
                &format!("Run `orx exp view set {id} {}`.", source.flag()),
            ))
        })?;
    }
    let config = match given {
        Some(given) => {
            let file = source.patterns.unwrap_or("PATH");
            Some(patterns(given, source, &check_fix(format!("--pattern {file}")))?.0)
        }
        None => None,
    };
    let configured = match configured {
        Some(given) => {
            let file = source.config.unwrap_or("PATH");
            config_of(
                given,
                source,
                &format!("Run `orx exp view set {id} --config {file}`."),
            )?
        }
        None => Config::default(),
    };
    let saved = store.patch_run_monitor_json(id, expected, |raw| {
        let mut settings = match raw.map(serde_json::from_str::<Value>).transpose() {
            Ok(Some(Value::Object(settings))) => settings,
            Ok(None) => Map::new(),
            Ok(Some(_)) | Err(_) => {
                return Err(crate::error::anyhow!(
                    "Experiment {id}'s run view settings are unreadable; they are left as they are"
                ))
            }
        };
        if let Some(markup) = markup {
            settings.insert("view".into(), json!(markup));
        }
        if let Some(config) = &config {
            settings.insert("patterns".into(), json!(config.patterns));
        }
        match &configured.plan {
            Some(plan) if plan.is_empty() => {
                settings.remove("plan");
            }
            Some(plan) => {
                settings.insert("plan".into(), json!(plan));
            }
            None => {}
        }
        match &configured.rules {
            Some(rules) if rules.is_empty() => {
                settings.remove("rules");
            }
            Some(rules) => {
                let rules: Vec<Value> = rules.iter().map(super::rules::Rule::json).collect();
                settings.insert("rules".into(), json!(rules));
            }
            None => {}
        }
        for (key, value) in [("brief", &configured.brief), ("watch", &configured.watch)] {
            match value {
                Some(value) if value.is_empty() => {
                    settings.remove(key);
                }
                Some(value) => {
                    settings.insert(key.into(), json!(value));
                }
                None => {}
            }
        }
        Ok(Value::Object(settings).to_string())
    })?;
    if saved.is_none() {
        return Err(Refusal::Changed(format!(
            "Experiment {id} changed while saving its view. Run `orx exp view get {id}` and retry."
        )));
    }
    let mut answer = described(store, id)?;
    answer["pending"] = json!(pending);
    Ok(answer)
}

/// `POST /api/runs/:id/view/check`: checks `markup`, or the view its experiment saved, against
/// what the run reported, and says what it reported.
pub(crate) fn check(
    store: &crate::store::Store,
    run: &str,
    markup: Option<&str>,
    folded: Option<&orx_signals::Snapshot>,
    source: &Source,
) -> Result<Value, Refusal> {
    let row = store
        .get_run(run)?
        .ok_or_else(|| Refusal::Missing(format!("Run {run} not found.")))?;
    let experiment = &row.experiment_id;
    let saved = stored(experiment, store.run_monitor_json(experiment)?.as_deref())?;
    let (markup, name, fix) = match markup {
        Some(markup) => (
            Some(markup.to_owned()),
            source.name().to_owned(),
            format!("Run `orx exp view check {run} {}`.", source.flag()),
        ),
        None => (
            saved,
            format!("of {experiment}"),
            format!("Run `orx exp view set {experiment} --file PATH`."),
        ),
    };
    let kept = if folded.is_none() {
        super::monitor::checkpoint(store, run)?
    } else {
        None
    };
    let folded = folded.or(kept.as_deref());
    let seen = folded.map(Seen::of).unwrap_or_default();
    let mut report = json!({
        "runId": run,
        "experimentId": experiment,
        "status": row.status,
        "seen": {"signals": seen.signals, "tables": seen.tables},
        "markup": markup,
        "tree": null,
        "pending": [],
    });
    if let Some((folded, tally)) = folded.and_then(|f| Some((f, f.tally.as_ref()?))) {
        let regexes = folded
            .patterns
            .as_ref()
            .map(|p| p.regexes())
            .unwrap_or_default();
        report["lines"] = json!(tally.lines);
        report["read"] =
            json!({"bytes": folded.cursor.offset, "complete": folded.coverage.complete});
        report["patterns"] = tally
            .hits
            .iter()
            .zip(regexes)
            .map(|(hit, regex)| {
                let first = hit
                    .first
                    .as_ref()
                    .map(|(line, text)| json!({"line": line, "text": text}));
                json!({"regex": regex, "matched": hit.matched, "first": first})
            })
            .collect();
    }
    if let Some(folded) = folded {
        report["stages"] = folded.phases.iter()
            .map(|p| json!({"name": p.name, "done": p.done, "of": p.of, "unit": p.unit, "source": p.source}))
            .collect();
    }
    report["disagrees"] = super::monitor::monitor(store, &row)?.1["disagrees"].take();
    if let Some(markup) = report["markup"].as_str().map(str::to_owned) {
        let compiled =
            compile(&markup).map_err(|problem| Refusal::Invalid(problem.text(&name, &fix)))?;
        let pending = compiled
            .check(&seen)
            .map_err(|problem| Refusal::Invalid(problem.text(&name, &fix)))?;
        report["tree"] = compiled.tree;
        report["pending"] = json!(pending);
    }
    Ok(report)
}

pub(crate) fn shown(
    run: &crate::store::StoredRun,
    monitor: &Value,
    summary: &Value,
    declared: &[orx_signals::Stage],
    replay: Option<&orx_signals::Snapshot>,
    pattern_error: Option<&str>,
) -> String {
    use super::monitor::value_text;

    fn names(value: &Value) -> Vec<&str> {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect()
    }

    fn pairs<'a>(
        body: &'a Value,
        from: Option<&str>,
        x: &str,
        y: &str,
    ) -> Vec<(&'a Value, &'a Value)> {
        if let Some(from) = from {
            body["rows"][from]
                .as_array()
                .into_iter()
                .flatten()
                .map(|row| (&row[x], &row[y]))
                .collect()
        } else {
            body["series"][y]
                .as_array()
                .into_iter()
                .flatten()
                .map(|point| (&point[0], &point[1]))
                .collect()
        }
    }

    fn chart_series<'a>(block: &'a Value, default_x: &'a str) -> Vec<(&'a str, &'a str, bool)> {
        let mut series: Vec<_> = block["series"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|one| {
                Some((
                    one["x"].as_str()?,
                    one["y"].as_str()?,
                    one["ghost"] != false,
                ))
            })
            .collect();
        if series.is_empty() {
            let x = if block["from"].is_string() {
                block["x"].as_str().unwrap_or("step")
            } else {
                default_x
            };
            series = names(&block["chart"])
                .into_iter()
                .map(|y| (x, y, true))
                .collect();
        }
        series
    }

    let number = |value: &Value| {
        value
            .as_f64()
            .or_else(|| value.as_str()?.parse::<f64>().ok())
            .filter(|n| n.is_finite())
    };
    let points = |pairs: Vec<(&Value, &Value)>, block: &Value| {
        let mut xs = Vec::new();
        let mut categories = Vec::new();
        let mut count = 0;
        for (x, y) in pairs {
            if number(y).is_none() {
                continue;
            }
            if block["mark"] == "bars" {
                if !x.is_null() {
                    categories.push(value_text(x, &Value::Null));
                    count += 1;
                }
            } else if let Some(x) = number(x) {
                xs.push(x);
                count += 1;
            }
        }
        let range = if let (Some(first), Some(last)) = (categories.first(), categories.last()) {
            format!("{first} .. {last} (categories)")
        } else if xs.is_empty() {
            "none".into()
        } else {
            let lo = xs.iter().copied().min_by(f64::total_cmp).unwrap();
            let hi = xs.iter().copied().max_by(f64::total_cmp).unwrap();
            format!("{lo} .. {hi}")
        };
        (count, range)
    };
    let signals = &monitor["view"]["signals"];
    let value = |name: &str, best: bool| {
        let raw = if best {
            monitor["best"].get(name)
        } else {
            None
        }
        .unwrap_or(&monitor["latest"][name]);
        let mut text = value_text(raw, &signals[name]);
        if let Some(unit) = signals[name]["unit"].as_str().filter(|unit| *unit != "%") {
            text.push_str(&format!(" {unit}"));
        }
        format!(
            "{}{} ({name}) = {text}",
            if best && monitor["best"].get(name).is_some() {
                "best "
            } else {
                ""
            },
            signals[name]["label"].as_str().unwrap_or(name)
        )
    };
    let mut lines = vec![format!(
        "Run {} · {} · experiment {}",
        run.id, run.status, run.experiment_id
    )];
    lines.push(format!(
        "Data: {}; as of {}.",
        monitor["availability"]["state"]
            .as_str()
            .unwrap_or("unknown"),
        crate::telemetry::iso8601_utc(monitor["asOf"].as_i64().unwrap_or(run.updated_at))
    ));
    if let Some(reason) = monitor["availability"]["reason"].as_str() {
        lines.push(format!("Availability: {reason}"));
    }
    lines.push("Above the chat".into());
    lines.push(format!("Line parts: lifecycle={}; stage/count, whole-run time, watcher sentence and open flag follow.", run.status));
    let stage = &summary["stage"];
    let order = &monitor["stageOrder"];
    let first = order["first"].as_u64();
    let index = order["index"].as_u64();
    let setup = monitor["setup"].is_object();
    let position = index.zip(first).map(|(index, first)| {
        if setup || index < first {
            1
        } else {
            index - first + 2
        }
    });
    let count = order["count"]
        .as_u64()
        .zip(first)
        .map(|(count, first)| count.saturating_sub(first) + 1);
    lines.push(format!(
        "Line stage: {}; position={}; stages={}.",
        if setup {
            "Setup"
        } else {
            stage["name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .unwrap_or(&run.status)
        },
        position
            .filter(|_| count.is_some())
            .map_or_else(|| "unknown".into(), |n| n.to_string()),
        count.map_or_else(|| "unknown".into(), |n| n.to_string())
    ));

    if stage.is_object() {
        let field = |name: &str| {
            if stage[name].is_null() {
                "unknown".into()
            } else {
                value_text(&stage[name], &Value::Null)
            }
        };
        lines.push(format!(
            "Stage: {}; done={}; of={}; unit={}.",
            stage["name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .unwrap_or("(unnamed)"),
            field("done"),
            field("of"),
            field("unit")
        ));
    } else {
        lines.push("Stage: none reported yet.".into());
    }
    let eta = &summary["etaMs"];
    let duration = |key: &str| {
        eta[key]
            .as_f64()
            .map(|ms| value_text(&json!(ms / 1000.0), &json!({"as":"duration"})))
            .unwrap_or_else(|| "unknown".into())
    };
    lines.push(if eta.is_object() {
        format!(
            "Time left: {}{} ({} .. {}; estimate at {}).",
            if summary["etaFloor"] == true {
                "at least "
            } else {
                ""
            },
            duration("mid"),
            duration("lo"),
            duration("hi"),
            eta["at"]
                .as_i64()
                .map(crate::telemetry::iso8601_utc)
                .unwrap_or_else(|| "unknown".into())
        )
    } else if summary["pending"] == true {
        "Time left: estimating.".into()
    } else {
        "Time left: unknown (summary has no estimate).".into()
    });
    if !summary["etaUntimed"].is_null() {
        lines.push(format!("Untimed work: {}", summary["etaUntimed"]));
    }
    if let Some(at) = summary["limitEndsAt"].as_i64() {
        lines.push(format!(
            "Time limit ends at: {}.",
            crate::telemetry::iso8601_utc(at)
        ));
    }
    let empty = Map::new();
    let terminal = super::Phase::parse(&run.status).is_some_and(super::Phase::is_terminal);
    let versus = monitor["versus"][if terminal { "final" } else { "step" }]
        .as_object()
        .unwrap_or(&empty);
    let comparison = |name: &str, by: Option<&str>| {
        let by = by
            .or_else(|| signals[name]["vs"].as_str())
            .unwrap_or(if terminal { "final" } else { "step" });
        let comparisons = monitor["versus"][by].as_object().unwrap_or(&empty);
        let Some(vs) = comparisons.get(name) else {
            return format!("{name}; vs={by}; unavailable");
        };
        let parent = value_text(&vs["parent"], &signals[name]);
        let step = vs["step"]
            .as_f64()
            .map(|step| format!("; step={step}"))
            .unwrap_or_default();
        let difference = super::watch::fill_values(
            &format!("{{{name}.vs}}"),
            &Value::Null,
            signals,
            comparisons,
        );
        format!("{name}; vs={by}; parent={parent}{step}; difference={difference}")
    };
    let fill = |text: &str| {
        super::watch::fill_values(text, &summary["bindings"], signals, versus).replace("**", "")
    };
    lines.push(match summary["sentence"]["template"].as_str() {
        Some(template) => format!("Watcher: {}", fill(template)),
        None => "Watcher: no sentence yet.".into(),
    });
    let flags = monitor["flags"].as_array().map_or(&[][..], Vec::as_slice);
    lines.push(match super::flags::open(flags) {
        Some(flag) => format!("Open {}", super::flags::status_line(flag, fill)),
        None => "Open flag: none.".into(),
    });
    let phase = monitor["progress"]["phase"]
        .as_u64()
        .and_then(|index| monitor["phases"].get(index as usize))
        .and_then(|phase| phase["name"].as_str());
    let chart = monitor["view"]["blocks"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|block| {
            !terminal
                && block["chart"].is_array()
                && block["phase"]
                    .as_str()
                    .is_none_or(|name| Some(name) == phase)
                && chart_series(block, monitor["x"]["name"].as_str().unwrap_or("step"))
                    .iter()
                    .any(|(x, y, _)| {
                        let from = block["from"].as_str();
                        if let (Some(table), Some(by)) = (
                            from,
                            block["by"]
                                .as_str()
                                .filter(|_| block["series"].as_array().is_none_or(Vec::is_empty)),
                        ) {
                            let mut groups = std::collections::BTreeMap::<String, usize>::new();
                            for row in monitor["rows"][table].as_array().into_iter().flatten() {
                                if number(&row[y]).is_some()
                                    && (number(&row[x]).is_some()
                                        || (block["mark"] == "bars" && !row[x].is_null()))
                                {
                                    let count = groups.entry(row[by].to_string()).or_default();
                                    *count += 1;
                                    if *count >= 2 {
                                        return true;
                                    }
                                }
                            }
                            false
                        } else {
                            points(pairs(monitor, from, x, y), block).0 >= 2
                        }
                    })
        });
    let open_flag = super::flags::open(flags);
    if open_flag.is_some() {
        lines.push("Open card: flag evidence and actions.".into());
    } else if let Some(block) = chart {
        lines.push("Open card: headline and one chart.".into());
        lines.push(
            match monitor["headline"]
                .as_str()
                .filter(|name| !monitor["latest"][name].is_null())
            {
                Some(name) => format!("Card headline: {}", value(name, false)),
                None => "Card headline: none.".into(),
            },
        );
        lines.push(format!(
            "Card chart: {}; parent ghost={}",
            names(&block["chart"])
                .iter()
                .map(|name| format!(
                    "{} ({name})",
                    signals[name]["label"].as_str().unwrap_or(name)
                ))
                .collect::<Vec<_>>()
                .join(", "),
            if block["ghost"] == "parent" {
                "on"
            } else {
                "off"
            }
        ));
        for (x, y, _) in chart_series(block, monitor["x"]["name"].as_str().unwrap_or("step")) {
            let from = block["from"].as_str();
            if !signals[y]["smooth"].is_null() {
                lines.push(format!(
                    "  {y}: retained readings before smoothing {}.",
                    signals[y]["smooth"]
                ));
            }
            let retained: Vec<_> = pairs(monitor, from, x, y)
                .into_iter()
                .filter(|(x, y)| {
                    number(y).is_some()
                        && (number(x).is_some() || (block["mark"] == "bars" && !x.is_null()))
                })
                .collect();
            let values = retained
                .iter()
                .enumerate()
                .filter_map(|(index, (at, raw))| {
                    if retained.len() > 12 && index >= 6 && index < retained.len() - 6 {
                        return (index == 6)
                            .then(|| format!("… {} points omitted …", retained.len() - 12));
                    }
                    Some(format!(
                        "{x} {} = {}",
                        value_text(at, &Value::Null),
                        value_text(raw, &signals[y])
                    ))
                })
                .collect::<Vec<_>>();
            lines.push(format!(
                "  {y}: {} points: {}",
                retained.len(),
                values.join(", ")
            ));
        }
    } else {
        lines.push(
            if terminal {
                "Card chart: none (run ended)."
            } else {
                "Card chart: none (no active chart has two points)."
            }
            .into(),
        );
    }
    lines.push("On the run page".into());
    let plan: Vec<_> = declared
        .iter()
        .enumerate()
        .map(|(index, phase)| {
            format!(
                "{}{} {} (of={})",
                if monitor["progress"]["phase"] == json!(index) && stage["name"] == phase.phase {
                    "* "
                } else {
                    ""
                },
                index + 1,
                phase.phase,
                phase
                    .of
                    .as_ref()
                    .map_or_else(|| "unknown".into(), ToString::to_string)
            )
        })
        .collect();
    lines.push(format!(
        "Declared plan (* current): {}",
        if plan.is_empty() {
            "none".into()
        } else {
            plan.join("; ")
        }
    ));
    lines.push(
        "The UI folds stages before its first real point into Setup and numbers stages from there."
            .into(),
    );

    let headline = names(&monitor["view"]["headline"]);
    if !headline.is_empty() {
        lines.push(format!(
            "Headline: {} (selected: {}).",
            headline
                .iter()
                .map(|name| value(name, false))
                .collect::<Vec<_>>()
                .join("; "),
            monitor["headline"].as_str().unwrap_or("none")
        ));
    }
    let mut missing = std::collections::BTreeSet::new();
    for name in &headline {
        if monitor["latest"].get(name).is_none() {
            missing.insert((*name).to_owned());
        }
    }
    let mut chart = 0;
    for block in monitor["view"]["blocks"].as_array().into_iter().flatten() {
        let scope = block["phase"]
            .as_str()
            .map(|phase| {
                format!(
                    "; phase={phase}{}",
                    if stage["name"] == phase {
                        ""
                    } else {
                        " (inactive)"
                    }
                )
            })
            .unwrap_or_default();
        for kind in ["table", "media"] {
            if block.get(kind).is_some() {
                lines.push(format!("{kind}{scope}: {block}"));
                if let Some(table) = block[kind].as_str().filter(|_| kind == "table") {
                    lines.push(format!("  rows: {}", monitor["rows"][table]));
                }
            }
        }
        if let Some(template) = block["text"].as_str() {
            let text =
                super::watch::fill_values(template, &monitor["latest"], signals, &Map::new());
            lines.push(format!("Text{scope}: {}", text.replace("**", "")));
            for name in text_signals(template).expect("compiled view text") {
                if monitor["latest"][&name].is_null() {
                    missing.insert(name);
                }
            }
        }
        if let Some(table) = block["cells"].as_str() {
            let rows = monitor["rows"][table]
                .as_array()
                .map_or(&[][..], Vec::as_slice);
            let recent = &rows[rows.len().saturating_sub(CELLS_WINDOW)..];
            let mut tally = BTreeMap::<&str, usize>::new();
            for row in recent {
                *tally.entry(outcome(&row["outcome"])).or_default() += 1;
            }
            lines.push(format!("Cells from {table}{scope}: {} retained rows; {} correct, {} wrong, {} errors, {} unknown.",
                recent.len(), tally.get("correct").unwrap_or(&0), tally.get("wrong").unwrap_or(&0),
                tally.get("error").unwrap_or(&0), tally.get("unknown").unwrap_or(&0)));
            if recent.is_empty() {
                missing.insert(format!("table {table}"));
            }
        }
        if let (Some(table), Some(by)) = (block["bars"].as_str(), block["by"].as_str()) {
            let groups = monitor["groups"][table][by]
                .as_array()
                .map_or(&[][..], Vec::as_slice);
            let cap = super::monitor::MAX_OUTCOME_GROUPS;
            lines.push(format!(
                "Bars from {table} by {by}{scope}: retained rows, up to {cap} categories."
            ));
            if groups.is_empty() {
                missing.insert(format!("{table}.{by}"));
            }
            for group in groups {
                let key = group["key"].as_str().expect("group key");
                let ok = group["ok"].as_u64().expect("group correct count");
                let n = group["n"].as_u64().expect("group classified count");
                let rate = if n == 0 {
                    "—".into()
                } else {
                    format!("{}%", ((ok * 100) as f64 / n as f64).round())
                };
                lines.push(format!("  {key}: {rate} ({ok} of {n})"));
            }
        }
        if block["chart"].is_array() {
            chart += 1;
            let from = block["from"].as_str();
            let series = chart_series(block, monitor["x"]["name"].as_str().unwrap_or("step"));
            for (x, y, ghost) in series {
                let (count, range) = points(pairs(monitor, from, x, y), block);
                let (parent_count, parent_range) =
                    points(pairs(&monitor["parent"], from, x, y), block);
                let ghost = if block["ghost"] != "parent" || !ghost {
                    "off".into()
                } else {
                    format!("on (finite points={parent_count}; x={parent_range})")
                };
                let total = if from.is_some() {
                    "not a progress axis".into()
                } else {
                    value_text(&monitor["x"]["max"], &Value::Null)
                };
                let label = signals[y]["label"].as_str().unwrap_or(y);
                let mode = block["xRange"].as_str().unwrap_or("run");
                lines.push(format!("Chart {chart}: {label}; {}y={y}; x={x}; observed x={range}; total={total}; range={mode}; finite retained points={count}; parent ghost={ghost}{scope}{}.", from.map(|table| format!("table={table}; ")).unwrap_or_default(), block["by"].as_str().map(|by| format!("; grouped by {by}")).unwrap_or_default()));
                if from.is_none() && signals[y]["better"].is_string() {
                    lines.push(format!(
                        "Chart {chart} comparison: {}.",
                        comparison(y, None)
                    ));
                }
                for key in ["title", "xScale", "yScale", "mark"] {
                    if let Some(setting) = block[key].as_str() {
                        lines.push(format!("  {key}: {setting}"));
                    }
                }
                if !signals[y]["smooth"].is_null() {
                    lines.push(format!("  smoothing: {}", signals[y]["smooth"]));
                }
                let reported = if let Some(from) = from {
                    monitor["rows"][from]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|row| row.get(y).is_some())
                } else {
                    monitor["latest"].get(y).is_some() || monitor["series"].get(y).is_some()
                };
                if !reported {
                    missing
                        .insert(from.map_or_else(|| y.to_owned(), |table| format!("{table}.{y}")));
                }
            }
        }
        if block["stats"].is_array() {
            lines.push(format!(
                "Stats{scope}: {}",
                names(&block["stats"])
                    .into_iter()
                    .map(|name| value(name, signals[name]["pick"] == "best"))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            for name in names(&block["stats"]) {
                if signals[name]["better"].is_string()
                    && !(signals[name]["pick"] == "best" && monitor["best"].get(name).is_some())
                {
                    lines.push(format!(
                        "Stats comparison: {}.",
                        comparison(name, block["vs"].as_str())
                    ));
                }
            }
        }
    }
    if chart == 0 {
        lines.push("Charts: none configured.".into());
    }
    lines.push(format!(
        "Headlined / charted names not observed in collected data: {}.",
        if missing.is_empty() {
            "none".into()
        } else {
            missing.into_iter().collect::<Vec<_>>().join(", ")
        }
    ));
    if let Some(error) = pattern_error {
        lines.push(format!("Pattern matches: unknown. {error}"));
    } else if let Some(replay) = replay {
        let complete = replay.coverage.reasons.is_empty();
        lines.push(format!(
            "Patterns checked through byte {} ({}).",
            replay.cursor.offset,
            if complete {
                "all available log bytes"
            } else {
                "incomplete log coverage"
            }
        ));
        if let (Some(tally), Some(patterns)) = (&replay.tally, &replay.patterns) {
            for (index, (hit, regex)) in tally.hits.iter().zip(patterns.regexes()).enumerate() {
                lines.push(format!(
                    "Pattern {} ({regex}): {} matches{}.",
                    index + 1,
                    hit.matched,
                    if hit.matched == 0 && complete {
                        "; never matched in the checked log"
                    } else if hit.matched == 0 {
                        "; matches outside the checked bytes are unknown"
                    } else {
                        ""
                    }
                ));
            }
        }
    } else {
        lines.push("Patterns: none configured.".into());
    }
    lines.join("\n")
}

pub(crate) fn instrumentation(
    monitor: &Value,
    snapshot: Option<&orx_signals::Snapshot>,
    patterns: &orx_signals::Config,
    replay: Option<&orx_signals::Snapshot>,
    ended: bool,
) -> Vec<String> {
    let mut problems = Vec::new();
    let seen = snapshot.map(Seen::of).unwrap_or_default();
    let phases = snapshot.map_or(&[][..], |s| s.phases.as_slice());
    let started = |name: &str| phases.iter().any(|phase| phase.name == name);
    let view = &monitor["view"];
    let ready = |phase: Option<&str>| {
        let Some(now) = monitor["asOf"].as_i64() else {
            return false;
        };
        let first_reporting = snapshot.and_then(|s| {
            s.series_stages
                .values()
                .min()
                .and_then(|occurrence| s.phases.iter().find(|p| p.occurrence == *occurrence))
                .or_else(|| (!s.rows.is_empty()).then(|| s.phases.last()).flatten())
        });
        let phase = phase.or_else(|| first_reporting.map(|p| p.name.as_str()));
        monitor["phases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|stage| phase.is_some_and(|name| stage["name"] == name))
            .filter_map(|stage| stage["startedAt"].as_i64())
            .next_back()
            .is_some_and(|start| now.saturating_sub(start) >= 120_000)
    };
    if let Some(replay) = replay {
        // A live writer can advance beyond the fold whose view is being checked.
        let complete = replay.coverage.complete
            || (snapshot.is_some_and(|known| replay.cursor.offset >= known.cursor.offset)
                && replay
                    .coverage
                    .reasons
                    .iter()
                    .all(|reason| reason == "check_limit"));
        if let Some(tally) = replay.tally.as_ref().filter(|_| complete) {
            for (index, (pattern, hit)) in patterns.patterns.iter().zip(&tally.hits).enumerate() {
                let regex = regex::Regex::new(&pattern.regex).expect("stored patterns are checked");
                let captures: Vec<_> = regex.capture_names().flatten().collect();
                let mut required = Vec::new();
                if let Some(phase) = pattern.phase.as_deref() {
                    required.push(phase);
                } else {
                    for block in view["blocks"].as_array().into_iter().flatten() {
                        let used = block["chart"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .chain(block["stats"].as_array().into_iter().flatten())
                            .filter_map(Value::as_str)
                            .chain(
                                block["series"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(|series| series["y"].as_str()),
                            );
                        let text = block["text"]
                            .as_str()
                            .map(|text| text_signals(text).expect("compiled view text"));
                        if pattern.row.as_deref().is_some_and(|row| {
                            ["from", "table", "cells", "bars"]
                                .iter()
                                .any(|kind| block[*kind] == row)
                        }) || used.into_iter().any(|name| captures.contains(&name))
                            || text
                                .iter()
                                .flatten()
                                .any(|name| captures.contains(&name.as_str()))
                        {
                            if let Some(phase) = block["phase"].as_str() {
                                required.push(phase);
                            }
                        }
                    }
                    if required.is_empty() {
                        if let Some(phase) = monitor["phases"]
                            .as_array()
                            .and_then(|phases| phases.last())
                            .and_then(|stage| stage["name"].as_str())
                        {
                            required.push(phase);
                        }
                    }
                }
                let due = ended
                    || if required.is_empty() {
                        ready(None)
                    } else {
                        required
                            .into_iter()
                            .any(|phase| started(phase) && ready(Some(phase)))
                    };
                if hit.matched == 0 && due {
                    problems.push(format!("Pattern {} matched no lines in the checked log. Correct its regex or phase with `orx exp view set EXP --pattern PATH`.", index + 1));
                }
            }
        }
    }
    if ended {
        return problems;
    }
    let headline: Vec<&str> = view["headline"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if (ready(None)
        || view["blocks"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|block| ready(block["phase"].as_str())))
        && (!seen.signals.is_empty() || !seen.tables.is_empty())
        && !headline.is_empty()
        && !headline.iter().any(|name| seen.has_signal(name))
    {
        problems.push(format!("No headline signal has appeared ({}). Report one of these names under `m`, or add a reported fallback to `headline` with `orx exp view set EXP --file PATH`.", headline.join(", ")));
    }
    let mut missing = std::collections::BTreeSet::new();
    let mut missing_block = std::collections::BTreeSet::new();
    for block in view["blocks"].as_array().into_iter().flatten() {
        if !ready(block["phase"].as_str())
            || block["phase"].as_str().is_some_and(|name| !started(name))
        {
            continue;
        }
        if let Some(text) = block["text"].as_str() {
            missing_block.extend(
                text_signals(text)
                    .expect("compiled view text")
                    .into_iter()
                    .filter(|name| !seen.has_signal(name)),
            );
        } else if let Some(table) = block["cells"].as_str().or(block["bars"].as_str()) {
            if let Some(columns) = seen.tables.get(table) {
                for column in ["outcome"].into_iter().chain(block["by"].as_str()) {
                    if !columns.iter().any(|seen| seen == column) {
                        missing_block.insert(format!("{table}.{column}"));
                    }
                }
            } else {
                missing_block.insert(format!("table {table}"));
            }
        } else if let Some(table) = block["from"].as_str() {
            if let Some(columns) = seen.tables.get(table) {
                let names = block["chart"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .chain(block["x"].as_str())
                    .chain(block["by"].as_str())
                    .chain(
                        block["series"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .flat_map(|series| [series["x"].as_str(), series["y"].as_str()])
                            .flatten(),
                    );
                for name in names {
                    if !columns.iter().any(|column| column == name) {
                        missing.insert(format!("{table}.{name}"));
                    }
                }
            } else {
                missing.insert(format!("table {table}"));
            }
        } else {
            for name in block["chart"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !seen.has_signal(name) {
                    missing.insert(name.to_owned());
                }
            }
        }
    }
    if !missing.is_empty() {
        problems.push(format!("Chart data has not appeared: {}. Report these names, or correct the chart with `orx exp view set EXP --file PATH`.", missing.into_iter().collect::<Vec<_>>().join(", ")));
    }
    if !missing_block.is_empty() {
        problems.push(format!("Run view data has not appeared: {}. Report these names, or correct the view with `orx exp view set EXP --file PATH`.", missing_block.into_iter().collect::<Vec<_>>().join(", ")));
    }
    for (index, phase) in phases.iter().enumerate() {
        let occurrence = phases[..index]
            .iter()
            .filter(|earlier| earlier.name == phase.name)
            .count();
        let planned = monitor["phases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|entry| entry["name"] == phase.name)
            .nth(occurrence);
        let has_planned_total = match snapshot.and_then(|snapshot| snapshot.plan.as_ref()) {
            Some(plan) => plan
                .iter()
                .filter(|stage| stage.phase == phase.name)
                .nth(occurrence)
                .is_some_and(|stage| stage.of.is_some()),
            None => planned.is_some_and(|entry| entry["of"].is_number()),
        };
        if phase.of.is_none() && !has_planned_total {
            let name = if phase.name.is_empty() {
                "(unnamed)"
            } else {
                &phase.name
            };
            problems.push(format!("Stage {name:?} has no total. Report its actual total with `orx_run.progress(total=...)` or `::orx:: {{\"of\":...}}`; if the total is in a log line, capture it as `of` in the pattern."));
        }
    }
    if phases.is_empty() {
        problems.push("No stage or progress total has appeared. Add `orx_run.progress(total=..., phase=...)` to the entry point.".into());
    }
    problems
}

/// The run's log as it reads now, folded afresh with `patterns`, counting what each matched.
pub(crate) async fn fold_for_check<F, Fut>(
    id: &str,
    patterns: std::sync::Arc<orx_signals::Patterns>,
    max_bytes: u64,
    mut read: F,
) -> crate::error::Result<orx_signals::Snapshot>
where
    F: FnMut(super::ByteRange, Option<super::FileIdent>) -> Fut,
    Fut: std::future::Future<Output = crate::error::Result<super::LogPage>>,
{
    use super::{ByteRange, LogPage};
    use crate::error::anyhow;
    let mut folded: Option<orx_signals::Snapshot> = None;
    let mut ident = None;
    loop {
        let start = folded.as_ref().map_or(0, |f| f.cursor.offset);
        let range = ByteRange {
            start,
            end: (start + orx_host_proto::MAX_PAGE_LEN).min(max_bytes),
        };
        let page = read(range, ident.clone()).await?;
        let LogPage::Data {
            ident: page_ident,
            bytes,
            eof,
            start: at,
            ..
        } = page
        else {
            let reason = if matches!(page, LogPage::Reset { info: None }) {
                "has no log yet"
            } else {
                "changed while it was being checked"
            };
            return Err(anyhow!(
                "Run {id}'s log {reason}. Retry `orx exp view check {id}`."
            ));
        };
        let snapshot = folded.get_or_insert_with(|| {
            let mut snapshot =
                orx_signals::Snapshot::new(id, &String::from(page_ident.clone()), "check");
            snapshot.tally = Some(Default::default());
            snapshot.configure(Some(patterns.clone()));
            snapshot
        });
        ident = Some(page_ident);
        if at > start {
            snapshot.coverage.add_reason("missing_middle");
            snapshot.cursor.offset = at;
        }
        let last = eof || bytes.is_empty() || at + bytes.len() as u64 >= max_bytes;
        snapshot
            .feed(&bytes, eof, crate::store::now_ms(), false)
            .map_err(|error| anyhow!(error))?;
        if last {
            if !eof && !bytes.is_empty() {
                snapshot.coverage.add_reason("check_limit");
            }
            return Ok(folded.unwrap());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_text_checks_and_reads_its_latest_signals() {
        let markup = r#"<signal name="loss" digits="2"/><view><text title="Validation">Loss is **{loss}** &amp; measured.</text></view>"#;
        let compiled = compile(markup).unwrap();
        assert_eq!(
            compiled.tree["blocks"][0],
            json!({
                "title": "Validation", "text": "Loss is **{loss}** & measured."
            })
        );
        let seen = Seen {
            signals: vec!["rate".into()],
            ..Seen::default()
        };
        assert_eq!(compiled.check(&seen).unwrap(), ["loss"]);
        let typo = compile(&markup.replace("{loss}", "{los}")).unwrap();
        assert!(typo.check(&seen).unwrap_err().message.contains("los"));
        for text in [
            "",
            " ",
            "{loss",
            "loss}",
            "{{loss}}",
            "{}",
            "{loss.vs}",
            "{loss rate}",
            "<stats of='loss' />",
        ] {
            assert!(
                compile(&format!("<view><text>{text}</text></view>")).is_err(),
                "{text:?}"
            );
        }
        let mut monitor = json!({"view": compiled.tree});
        let show = |monitor: &Value| shown(&shown_run(), monitor, &Value::Null, &[], None, None);
        assert!(show(&monitor).contains("Loss is — & measured."));
        monitor["latest"]["loss"] = json!(2.3);
        assert!(show(&monitor).contains("Loss is 2.30 & measured."));
    }

    #[test]
    fn authored_outcomes_check_the_table_and_its_columns() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/contracts/run-view-outcomes.json"))
                .unwrap();
        let compiled = compile(fixture["markup"].as_str().unwrap()).unwrap();
        assert_eq!(compiled.tree, fixture["tree"]);
        let cells = compile(r#"<view><cells from="items" /></view>"#).unwrap();
        let bars = compile(r#"<view><bars from="items" by="category" /></view>"#).unwrap();
        assert_eq!(cells.check(&Seen::default()).unwrap(), ["items", "outcome"]);
        assert_eq!(
            bars.check(&Seen::default()).unwrap(),
            ["items", "outcome", "category"]
        );
        let mut seen = Seen::default();
        seen.tables.insert("cases".into(), vec!["outcome".into()]);
        assert!(cells
            .check(&seen)
            .unwrap_err()
            .message
            .contains("no table \"items\""));
        seen.tables.insert("items".into(), vec!["score".into()]);
        assert!(cells
            .check(&seen)
            .unwrap_err()
            .message
            .contains("no column \"outcome\""));
        seen.tables.get_mut("items").unwrap().push("outcome".into());
        assert!(cells.check(&seen).unwrap().is_empty());
        assert!(bars
            .check(&seen)
            .unwrap_err()
            .message
            .contains("no column \"category\""));
        for block in [
            "<cells />",
            "<bars from='items' />",
            "<bars from='bad-table' by='category' />",
        ] {
            assert!(compile(&format!("<view>{block}</view>")).is_err());
        }
    }

    #[test]
    fn authored_blocks_wait_for_their_stage_and_name_the_missing_inputs() {
        let config = orx_signals::Config::default();
        let snapshot = check_snapshot(
            "::orx:: {\"phase\":\"eval\",\"of\":8,\"row\":\"items\",\"m\":{\"category\":\"proofs\"}}\n",
            &config,
        );
        let view = compile(r#"<view><text phase="eval">Loss is {loss}.</text><cells from="items" phase="eval" /><bars from="items" by="split" phase="eval" /><text phase="later">Rate is {rate}.</text></view>"#).unwrap().tree;
        let mut monitor =
            json!({"view":view,"asOf":120999,"phases":[{"name":"eval","startedAt":1000}]});
        assert!(instrumentation(&monitor, Some(&snapshot), &config, None, false).is_empty());
        monitor["asOf"] = json!(121000);
        let problems = instrumentation(&monitor, Some(&snapshot), &config, None, false);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("items.outcome, items.split, loss"));
        assert!(!problems[0].contains("rate"));
    }

    #[test]
    fn charts_draw_the_parent_unless_the_view_turns_it_off() {
        let default = compile(r#"<view><chart y="loss"/></view>"#).unwrap().tree;
        assert_eq!(default["blocks"][0]["ghost"], "parent");
        let off = compile(r#"<view><chart y="loss" ghost="false"/></view>"#)
            .unwrap()
            .tree;
        assert_eq!(off["blocks"][0]["ghost"], "false");
        let series = compile(r#"<view><chart from="scores"><series x="step" y="score" ghost="false"/></chart></view>"#).unwrap().tree;
        assert_eq!(series["blocks"][0]["ghost"], "parent");
        assert_eq!(series["blocks"][0]["series"][0]["ghost"], false);
    }

    fn shown_run() -> crate::store::StoredRun {
        crate::store::StoredRun {
            id: "run-1".into(),
            experiment_id: "exp-1".into(),
            project_id: "project-1".into(),
            status: "running".into(),
            backend_json: "{}".into(),
            command: "train".into(),
            created_at: 0,
            updated_at: 1000,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            cancel_reason: None,
            chat_session_id: None,
        }
    }

    #[test]
    fn view_show_separates_the_compact_card_from_the_run_page() {
        let view = compile(r#"<signal name="val_bpb" label="Base val bpb" digits="3"/><view headline="val_bpb"><chart y="val_bpb" phase="train"/><chart y="loss" phase="train"/><stats of="loss"/></view>"#).unwrap().tree;
        let mut monitor = json!({
            "view":view, "headline":"val_bpb", "latest":{"val_bpb":1.941,"loss":2},
            "progress":{"phase":0}, "phases":[{"name":"train"}],
            "series":{"val_bpb":[[0,3.196],[100,1.941]],"loss":[[0,3],[100,2]]}
        });
        let summary = json!({"stage":{"name":"train","done":100,"of":1000}});
        let text = shown(&shown_run(), &monitor, &summary, &[], None, None);
        let (above, page) = text.split_once("On the run page").unwrap();
        assert!(above.contains("Above the chat"));
        assert!(above.contains("Card headline: Base val bpb (val_bpb) = 1.941"));
        assert!(
            above.contains("2 points: step 0 = 3.196, step 100 = 1.941"),
            "{above}"
        );
        assert!(!above.contains("Chart 2"));
        assert!(!above.contains("Declared plan"));
        assert!(page.contains("Chart 2"));
        monitor["series"]["val_bpb"] = json!([[0, 3.196]]);
        let text = shown(&shown_run(), &monitor, &summary, &[], None, None);
        let (above, _) = text.split_once("On the run page").unwrap();
        assert!(above.contains("Card chart: loss (loss)"));
        monitor["series"]["loss"] = json!([]);
        let text = shown(&shown_run(), &monitor, &summary, &[], None, None);
        assert!(text.contains("Card chart: none (no active chart has two points)"));
    }

    #[test]
    fn view_show_reads_summary_parts_chart_data_flags_stats_and_missing_patterns() {
        let config: orx_signals::Config =
            serde_json::from_value(json!({"patterns":[{"regex":"^never (?P<missing>\\d+)$"}]}))
                .unwrap();
        let replay = check_snapshot("a log line\n", &config);
        let view = compile(r#"<signal name="loss" label="Training loss" digits="3" better="lower" pick="best"/><signal name="missing"/><view headline="loss missing"><chart y="loss missing" ghost="parent"/><stats of="loss"/></view>"#).unwrap().tree;
        let monitor = json!({
            "view":view,"availability":{"state":"live"},"asOf":1000,
            "phases":[{"name":"setup","of":1},{"name":"train","of":20},{"name":"eval","of":1}],
            "progress":{"phase":1},"x":{"name":"step","max":20},
            "latest":{"loss":1.25},"best":{"loss":1.0},"headline":"loss",
            "series":{"loss":[[0,2],[5,null],[10,1.25]]},
            "parent":{"series":{"loss":[[0,3],[20,1]]}},
            "flags":[{"id":"f1","state":"waking","detail":"Loss **{loss}** needs review."}],
        });
        let summary = json!({"stage":{"name":"train","done":10,"of":20,"unit":"steps"},"etaMs":{"lo":5000,"mid":10000,"hi":15000,"at":1000},"sentence":{"template":"Loss **{loss}**."},"bindings":{"loss":1.25}});
        let plan = orx_signals::plan(
            &json!([{"phase":"setup","of":1},{"phase":"train","of":20},{"phase":"eval","of":1}]),
            true,
        )
        .unwrap();
        let text = shown(&shown_run(), &monitor, &summary, &plan, Some(&replay), None);
        for expected in [
            "Stage: train; done=10; of=20; unit=steps",
            "* 2 train (of=20)",
            "UI folds stages",
            "Time left: 10s",
            "Watcher: Loss 1.250.",
            "Open flag",
            "flag f1",
            "observed x=0 .. 10; total=20",
            "finite retained points=2",
            "parent ghost=on (finite points=2",
            "Stats: best Training loss (loss) = 1.000",
            "names not observed in collected data: missing",
            "never matched in the checked log",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
    }

    #[test]
    fn view_show_reports_parent_comparisons_for_stats_and_charts() {
        let view = compile(r#"<signal name="loss" digits="2" better="lower" vs="final"/><signal name="rate" better="higher"/><signal name="best" pick="best" better="lower"/><view headline="loss"><chart y="loss rate"/><stats of="loss rate best" vs="step"/><stats of="loss"/></view>"#).unwrap().tree;
        let monitor = json!({
            "view": view, "latest":{"loss":2,"rate":10,"best":1}, "best":{"best":0.5},
            "versus":{
                "step":{
                    "loss":{"parent":3,"step":10,"d":-1,"mag":1,"good":true,"flat":false},
                    "rate":{"parent":10,"step":10,"d":0,"mag":0,"good":false,"flat":true},
                    "best":{"parent":2,"step":10,"d":-1,"mag":1,"good":true,"flat":false}
                },
                "final":{"loss":{"parent":1,"d":1,"mag":1,"good":false,"flat":false}}
            }
        });
        let text = shown(&shown_run(), &monitor, &json!({}), &[], None, None);
        for expected in [
            "Chart 1 comparison: loss; vs=final; parent=1.00; difference=1.00 worse than the parent",
            "Chart 1 comparison: rate; vs=step; parent=10; step=10; difference=level with the parent",
            "Stats comparison: loss; vs=step; parent=3.00; step=10; difference=1.00 better than the parent",
            "Stats comparison: loss; vs=final; parent=1.00; difference=1.00 worse than the parent",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        assert!(!text.contains("Stats comparison: best"), "{text}");

        let mut ended = shown_run();
        ended.status = "done".into();
        let ended_text = shown(&ended, &monitor, &json!({}), &[], None, None);
        assert!(
            ended_text.contains("Chart 1 comparison: rate; vs=final; unavailable"),
            "{ended_text}"
        );
        assert!(
            ended_text.contains("Stats comparison: loss; vs=step; parent=3.00; step=10"),
            "{ended_text}"
        );
    }

    #[test]
    fn compact_table_chart_needs_two_points_in_one_line_and_reads_explicit_axes() {
        let view =
            compile(r#"<view><chart from="scores" x="width" y="score" by="method"/></view>"#)
                .unwrap()
                .tree;
        let mut monitor = json!({"view":view,"rows":{"scores":[
            {"width":1,"score":0.8,"method":"a"}, {"width":2,"score":0.7,"method":"b"}
        ]}});
        let text = shown(&shown_run(), &monitor, &json!({}), &[], None, None);
        assert!(text
            .split("On the run page")
            .next()
            .unwrap()
            .contains("Card chart: none"));
        monitor["rows"]["scores"][1]["method"] = json!("a");
        let text = shown(&shown_run(), &monitor, &json!({}), &[], None, None);
        assert!(
            text.contains("2 points: width 1 = 0.8, width 2 = 0.7"),
            "{text}"
        );
        monitor["view"] =
            compile(r#"<view><chart from="scores"><series x="width" y="score"/></chart></view>"#)
                .unwrap()
                .tree;
        let text = shown(&shown_run(), &monitor, &json!({}), &[], None, None);
        assert!(
            text.contains("2 points: width 1 = 0.8, width 2 = 0.7"),
            "{text}"
        );
    }

    #[test]
    fn view_show_handles_table_series_disabled_ghosts_and_unknown_reads() {
        let view = compile(r#"<view headline="score"><chart from="scores" ghost="parent"><series x="width" y="score" ghost="false"/></chart></view>"#).unwrap().tree;
        let monitor = json!({"view":view,"availability":{"state":"stale","reason":"host disconnected"},"asOf":1000,"latest":{"score":0.5},"headline":"score","rows":{"scores":[{"width":"10","score":"0.5"},{"width":20,"score":null}]},"parent":{"rows":{"scores":[{"width":1,"score":0.1}]}}});
        let text = shown(
            &shown_run(),
            &monitor,
            &json!({}),
            &[],
            None,
            Some("the log is unavailable"),
        );
        for expected in [
            "Data: stale",
            "host disconnected",
            "Stage: none reported",
            "Time left: unknown",
            "table=scores; y=score; x=width; observed x=10 .. 10",
            "finite retained points=1; parent ghost=off",
            "names not observed in collected data: none",
            "Pattern matches: unknown. the log is unavailable",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        assert!(!text.contains("never matched"));
    }

    fn check_snapshot(log: &str, config: &orx_signals::Config) -> orx_signals::Snapshot {
        let mut snapshot = orx_signals::Snapshot::new("check", "1-7", "check");
        snapshot.tally = Some(Default::default());
        snapshot.configure(Some(std::sync::Arc::new(
            orx_signals::Patterns::compile(config).unwrap(),
        )));
        snapshot.feed(log.as_bytes(), true, 1000, true).unwrap();
        snapshot
    }

    #[test]
    fn instrumentation_gives_chart_stages_two_minutes_to_report() {
        let config = orx_signals::Config::default();
        let snapshot = check_snapshot(
            "::orx:: {\"phase\":\"train\",\"step\":1,\"m\":{\"loss\":2}}\n",
            &config,
        );
        let view = compile(r#"<signal name="loss"/><signal name="val_bpb"/><view headline="val_bpb"><chart y="val_bpb" phase="train"/></view>"#).unwrap().tree;
        let mut monitor =
            json!({"view":view,"asOf":1000,"phases":[{"name":"train","of":10,"startedAt":1000}]});
        assert!(instrumentation(&monitor, Some(&snapshot), &config, None, false).is_empty());
        monitor["asOf"] = json!(121000);
        let problems = instrumentation(&monitor, Some(&snapshot), &config, None, false);
        assert!(problems.iter().any(|p| p.contains("val_bpb")));
        monitor["phases"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":"train","of":10,"startedAt":121000}));
        assert!(instrumentation(&monitor, Some(&snapshot), &config, None, false).is_empty());
        monitor["phases"].as_array_mut().unwrap().truncate(1);
        monitor["view"]["blocks"][0]
            .as_object_mut()
            .unwrap()
            .remove("phase");
        monitor["asOf"] = json!(120999);
        assert!(instrumentation(&monitor, Some(&snapshot), &config, None, false).is_empty());
        monitor["asOf"] = json!(121000);
        assert!(!instrumentation(&monitor, Some(&snapshot), &config, None, false).is_empty());
        let setup = check_snapshot(
            "::orx:: {\"phase\":\"download\",\"step\":1,\"of\":9}\n",
            &config,
        );
        monitor["phases"] = json!([{"name":"download","of":9,"startedAt":0}]);
        monitor["asOf"] = json!(600000);
        assert!(instrumentation(&monitor, Some(&setup), &config, None, false).is_empty());
    }

    #[test]
    fn instrumentation_waits_for_patterns_view_phase_and_stage_grace() {
        let config: orx_signals::Config = serde_json::from_value(json!({"patterns":[
            {"regex":"^summary (?P<accuracy>\\d+)$","row":"summary"}
        ]}))
        .unwrap();
        for markup in [
            r#"<view headline="loss"><chart y="loss" phase="train"/><chart y="accuracy" phase="summary"/></view>"#,
            r#"<view headline="loss"><chart y="loss" phase="train"/><table name="summary" columns="accuracy" phase="summary"/></view>"#,
        ] {
            let mut snapshot = check_snapshot(
                "::orx:: {\"phase\":\"train\",\"step\":1,\"of\":10,\"m\":{\"loss\":2}}\n",
                &config,
            );
            let view = compile(markup).unwrap().tree;
            let mut monitor = json!({"view":view,"asOf":600000,"phases":[{"name":"train","startedAt":1000,"of":10},{"name":"summary"},{"name":"export"}]});
            assert!(
                instrumentation(&monitor, Some(&snapshot), &config, Some(&snapshot), false)
                    .is_empty()
            );
            snapshot = check_snapshot(
                concat!(
                    "::orx:: {\"phase\":\"train\",\"step\":1,\"of\":10,\"m\":{\"loss\":2}}\n",
                    "::orx:: {\"phase\":\"summary\",\"step\":0,\"of\":1}\n"
                ),
                &config,
            );
            monitor["phases"][1]["startedAt"] = json!(599000);
            assert!(
                instrumentation(&monitor, Some(&snapshot), &config, Some(&snapshot), false)
                    .is_empty()
            );
            monitor["asOf"] = json!(719000);
            assert!(
                instrumentation(&monitor, Some(&snapshot), &config, Some(&snapshot), false)
                    .iter()
                    .any(|p| p.contains("Pattern 1"))
            );
            monitor["asOf"] = json!(600000);

            assert!(
                instrumentation(&monitor, Some(&snapshot), &config, Some(&snapshot), true)
                    .iter()
                    .any(|p| p.contains("Pattern 1"))
            );
        }
    }

    #[test]
    fn instrumentation_reports_each_missing_part_and_its_fix() {
        let config: orx_signals::Config = serde_json::from_value(json!({"patterns":[
            {"regex":"^loss (?P<loss>\\d+)$","phase":"train"},
            {"regex":"^accuracy (?P<accuracy>\\d+)$","phase":"eval"}
        ]}))
        .unwrap();
        let snapshot = check_snapshot(
            "::orx:: {\"phase\":\"train\",\"step\":1,\"m\":{\"loss\":2}}\n",
            &config,
        );
        let view = compile(r#"<signal name="accuracy"/><signal name="missing"/><view headline="accuracy"><chart y="missing"/></view>"#).unwrap().tree;
        let monitor =
            json!({"view":view,"asOf":121000,"phases":[{"name":"train","startedAt":1000}]});
        let problems = instrumentation(&monitor, Some(&snapshot), &config, Some(&snapshot), false);
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("--pattern PATH")));
        assert!(problems
            .iter()
            .any(|p| p.contains("accuracy") && p.contains("headline")));
        assert!(problems
            .iter()
            .any(|p| p.contains("missing") && p.contains("--file PATH")));
        assert!(problems
            .iter()
            .any(|p| p.contains("train") && p.contains("total=")));
        assert!(!problems.iter().any(|p| p.contains("Pattern 2")));
    }

    #[test]
    fn instrumentation_accepts_planned_totals_and_a_headline_fallback_until_eval() {
        let config: orx_signals::Config = serde_json::from_value(json!({"patterns":[
            {"regex":"^accuracy (?P<accuracy>\\d+)$","phase":"eval"}
        ]}))
        .unwrap();
        let snapshot = check_snapshot(
            "::orx:: {\"phase\":\"train\",\"step\":1,\"m\":{\"loss\":2}}\n",
            &config,
        );
        let view = compile(r#"<signal name="accuracy"/><signal name="loss"/><view headline="accuracy loss"><chart y="loss" phase="train"/><chart y="accuracy" phase="eval"/></view>"#).unwrap().tree;
        let monitor = json!({"view":view,"asOf":121000,"phases":[{"name":"train","startedAt":1000,"of":10},{"name":"eval","startedAt":1000,"of":5}]});
        assert!(
            instrumentation(&monitor, Some(&snapshot), &config, Some(&snapshot), false).is_empty()
        );
    }

    #[tokio::test]
    async fn instrumentation_does_not_spend_warning_on_a_capped_replay() {
        let config: orx_signals::Config = serde_json::from_value(json!({"patterns":[
            {"regex":"^accuracy (?P<accuracy>\\d+) of (?P<of>\\d+)$","phase":"eval"}
        ]}))
        .unwrap();
        let mut log =
            "::orx:: {\"phase\":\"train\",\"step\":1,\"of\":2,\"m\":{\"loss\":2}}\n".to_owned();
        log.push_str(&"training output\n".repeat(80_000));
        log.push_str("::orx:: {\"phase\":\"eval\",\"step\":1,\"of\":2}\naccuracy 1 of 2\n");
        let snapshot = check_snapshot(&log, &config);
        let patterns = std::sync::Arc::new(orx_signals::Patterns::compile(&config).unwrap());
        let replay = fold_for_check("check", patterns, 1024 * 1024, |range, _| {
            let end = (range.end as usize).min(log.len());
            std::future::ready(Ok(super::super::LogPage::Data {
                len: Some(log.len() as u64),
                ident: super::super::FileIdent { dev: 1, ino: 7 },
                start: range.start,
                bytes: log.as_bytes()[range.start as usize..end].to_vec(),
                eof: end == log.len(),
            }))
        })
        .await
        .unwrap();
        assert!(replay.cursor.offset < snapshot.cursor.offset);
        assert_eq!(replay.tally.as_ref().unwrap().hits[0].matched, 0);
        assert_eq!(snapshot.tally.as_ref().unwrap().hits[0].matched, 1);
        let view = compile(r#"<view headline="loss"><chart y="loss" phase="train"/><chart y="accuracy" phase="eval"/></view>"#).unwrap().tree;
        let monitor = json!({"view":view,"asOf":121000,"phases":[{"name":"train","startedAt":1000},{"name":"eval","startedAt":1000}]});
        let problems = instrumentation(&monitor, Some(&snapshot), &config, Some(&replay), false);
        assert!(problems.is_empty(), "{problems:?}");

        let mut missing_total = snapshot.clone();
        missing_total.phases.last_mut().unwrap().of = None;
        let problems = instrumentation(
            &monitor,
            Some(&missing_total),
            &config,
            Some(&replay),
            false,
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("eval") && problems[0].contains("total="));
    }

    #[test]
    fn instrumentation_accepts_headlines_and_curves_from_reported_rows() {
        let config = orx_signals::Config::default();
        let snapshot = check_snapshot("::orx:: {\"phase\":\"eval\",\"step\":1,\"of\":2,\"row\":\"scores\",\"m\":{\"item\":1,\"accuracy\":0.5}}\n", &config);
        for view in [
            super::super::monitor::default_view(Some(&snapshot)),
            compile(r#"<view headline="accuracy"><chart y="accuracy"/><chart from="scores" x="item" y="accuracy"/></view>"#).unwrap().tree,
        ] {
            let monitor = json!({"view":view,"asOf":121000,"phases":[{"name":"eval","startedAt":1000}]});
            assert!(instrumentation(&monitor, Some(&snapshot), &config, None, false).is_empty());
        }
    }

    #[test]
    fn instrumentation_checks_chart_columns_and_each_stage_occurrence() {
        let config = orx_signals::Config::default();
        let snapshot = check_snapshot(concat!(
            "::orx:: {\"phase\":\"train\",\"step\":1}\n",
            "::orx:: {\"phase\":\"eval\",\"of\":1,\"row\":\"scores\",\"m\":{\"task\":\"a\",\"other\":2}}\n",
            "::orx:: {\"phase\":\"train\",\"step\":1}\n"
        ), &config);
        let view = compile(r#"<view><chart from="scores" x="task" y="accuracy" by="arm"/></view>"#)
            .unwrap()
            .tree;
        let monitor = json!({"view":view,"asOf":121000,"phases":[{"name":"train","startedAt":1000,"of":10},{"name":"eval","startedAt":1000,"of":1},{"name":"train","startedAt":1000}]});
        let problems = instrumentation(&monitor, Some(&snapshot), &config, None, false);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("scores.accuracy")));
        assert!(problems.iter().any(|p| p.contains("scores.arm")));
        assert!(problems
            .iter()
            .any(|p| p.contains("train") && p.contains("total=")));
    }

    #[test]
    fn instrumentation_uses_the_runs_plan_instead_of_an_older_experiment_plan() {
        let config = orx_signals::Config::default();
        let snapshot = check_snapshot("::orx:: {\"plan\":[{\"phase\":\"train\",\"of\":5}],\"phase\":\"train\",\"step\":1,\"m\":{\"loss\":2}}\n", &config);
        let monitor = json!({"view":compile("<view><chart y=\"loss\"/></view>").unwrap().tree,"asOf":121000,"phases":[{"name":"train","startedAt":1000,"of":10}]});
        assert!(instrumentation(&monitor, Some(&snapshot), &config, None, false).is_empty());
        let snapshot = check_snapshot("::orx:: {\"plan\":[{\"phase\":\"train\"}],\"phase\":\"train\",\"step\":1,\"m\":{\"loss\":2}}\n", &config);
        let problems = instrumentation(&monitor, Some(&snapshot), &config, None, false);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("total="));
    }

    #[tokio::test]
    async fn checking_a_bounded_or_replaced_log_never_claims_complete_evidence() {
        let page = super::super::LogPage::Data {
            len: None,
            ident: super::super::FileIdent { dev: 1, ino: 7 },
            start: 0,
            bytes: b"loss 2\n".to_vec(),
            eof: false,
        };
        let patterns = std::sync::Arc::new(
            orx_signals::Patterns::compile(&orx_signals::Config::default()).unwrap(),
        );
        let prefix = fold_for_check("bounded", patterns.clone(), 7, |_, _| {
            std::future::ready(Ok(page.clone()))
        })
        .await
        .unwrap();
        assert!(!prefix.coverage.complete);
        let mut pages =
            std::collections::VecDeque::from([page, super::super::LogPage::Reset { info: None }]);
        let failed = fold_for_check("replaced", patterns, 1024, |_, _| {
            std::future::ready(Ok(pages.pop_front().unwrap()))
        })
        .await;
        assert!(failed.is_err());
    }

    /// A codec benchmark: a frontier against a reference, from a table the run prints at its end.
    pub(crate) const FRONTIER: &str = r#"<signal name="front_gain" label="Frontier gain" better="higher" sign="true" />
<signal name="valid" label="Bit-exact" labels='{"1":"valid","0":"invalid"}' />
<signal name="cand_kb" label="Size" unit="KB" better="lower" />
<signal name="cand_ms" label="Compress time" unit="ms" better="lower" />

<view headline="front_gain">
  <chart from="levels" x-scale="log" ghost="parent" title="Size against compress time">
    <series x="ref_ms" y="ref_kb" label="Reference build" ghost="false" />
    <series x="cand_ms" y="cand_kb" label="Candidate" />
  </chart>
  <stats of="front_gain speedup_mean size_gain_mean valid" vs="final" />
  <table name="levels" columns="level speedup size_ratio cand_ms cand_kb" />
</view>
"#;

    fn problem(markup: &str) -> Problem {
        compile(markup).expect_err("the markup should be refused")
    }

    #[test]
    fn the_frontier_parses_to_the_tree_the_page_draws() {
        let tree = compile(FRONTIER).unwrap().tree;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/contracts/run-view-frontier.json");
        let fixture = json!({"markup": FRONTIER, "tree": tree});
        if std::env::var_os("ORX_WRITE_CONTRACTS").is_some() {
            std::fs::write(
                &path,
                serde_json::to_string_pretty(&fixture).unwrap() + "\n",
            )
            .unwrap();
        }
        let expected: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(fixture, expected);
        assert_eq!(
            tree["blocks"][0],
            json!({
                "chart": [], "from": "levels",
                "series": [
                    {"x": "ref_ms", "y": "ref_kb", "label": "Reference build", "ghost": false},
                    {"x": "cand_ms", "y": "cand_kb", "label": "Candidate"},
                ],
                "xScale": "log", "ghost": "parent", "title": "Size against compress time",
            })
        );
        assert_eq!(
            tree["signals"]["valid"]["labels"],
            json!({"1": "valid", "0": "invalid"})
        );
        assert_eq!(tree["headline"], json!(["front_gain"]));
    }

    #[test]
    fn each_mistake_says_where_and_what() {
        let cases = [
            ("<view>\n  <chart y=\"loss\" scale=\"log\"/>\n</view>", 2, 19, "unknown attribute \"scale\" on <chart>; allowed: y, x, from, by, x-scale, y-scale, x-range, mark, ghost, phase, title"),
            ("<signal name=\"loss\" digits=\"13\"/><view/>", 1, 21, "digits=\"13\" must be a whole number from 0 to 12"),
            ("<signal name=\"loss\" better=\"down\"/><view/>", 1, 21, "better=\"down\" is not one of lower, higher"),
            ("<signal name=\"a\"/><signal name=\"a\"/><view/>", 1, 19, "signal \"a\" is declared twice"),
            ("<signal name=\"a\"/>", 1, 1, "a view needs one <view> holding its blocks, after its <signal> declarations"),
            ("<view/><view/>", 1, 8, "a view has exactly one <view>"),
            ("<chart y=\"a\"/><view/>", 1, 1, "<chart> goes inside <view>; only <signal> and <view> stand alone"),
            ("<view><signal name=\"a\"/><chart y=\"a\"/></view>", 1, 7, "<signal> goes before <view>, not inside it: move it above <view> and keep it, since its label, unit and better reach every block"),
            ("<view><series x=\"a\" y=\"b\"/></view>", 1, 7, "<series> goes inside a <chart>"),
            ("<view><chart/></view>", 1, 7, "<chart> needs y=\"…\", the signals it draws, or <series x=\"…\" y=\"…\"/> inside it"),
            ("<view><chart y=\"loss\" x=\"epoch\"/></view>", 1, 23, "x=\"epoch\" is not the run's step: a chart over another x draws from a table, so add from=\"TABLE\"; or leave x out to draw over the run's steps"),
            ("<view><chart from=\"t\" y=\"a\"/></view>", 1, 7, "a chart from a table needs x=\"COLUMN\", the column across the bottom"),
            ("<view><chart y=\"a b c d e f g h i\"/></view>", 1, 14, "y names 9 signals; at most 8"),
            ("<view><chart y=\"val-loss\"/></view>", 1, 14, "\"val-loss\" in y is not a name: use letters, digits and _, starting with a letter or _ (at most 64)"),
            ("<view>loss</view>", 1, 7, "text is not allowed in a view; put words in label=\"…\" or title=\"…\""),
            ("<view><chart y=\"a\"></view>", 1, 20, "the markup is not well formed: expected 'chart' tag, not 'view'"),
            ("<!DOCTYPE x [<!ENTITY a \"b\">]><view/>", 1, 1, "a view has no XML declaration, DTD or entities"),
            ("<signal name=\"v\" values='{\"1\":\"valid\"}'/><view/>", 1, 18, "values maps \"1\" to \"valid\", but values says which outcome each value is: correct, wrong or error; to show a word for a value, use labels"),
            ("<signal name=\"v\" pick=\"best\"/><view/>", 1, 1, "signal \"v\" picks its best value but declares no better=\"lower|higher\""),
            ("<view><chart y=\"a\"><series x=\"b\" y=\"c\"/></chart></view>", 1, 7, "a chart takes y=\"…\" or <series> inside it, not both"),
            ("<view><chart><series x=\"b\" y=\"c\"/></chart></view>", 1, 14, "<series> draws columns of a table: add from=\"TABLE\" to the chart"),
            ("<view><chart from=\"t\" ghost=\"parent\"><series x=\"b\" y=\"c\" ghost=\"no\"/></chart></view>", 1, 58, "ghost=\"no\" is not one of true, false"),
            ("<view><chart from=\"t\" x=\"a\" y=\"b\" x-range=\"run\"/></view>", 1, 35, "x-range sets how far a chart over the run's steps reaches; a chart from a table spans its points"),
            ("<view><chart y=\"loss\" x-range=\"all\"/></view>", 1, 23, "x-range=\"all\" is not one of run, parent"),
            ("<view><stats of=\"loss\" vs=\"parent\"/></view>", 1, 24, "vs=\"parent\" is not one of step, final"),
            ("<signal name=\"loss\" vs=\"best\"/><view/>", 1, 21, "vs=\"best\" is not one of step, final"),
            ("<view><table name=\"t\" columns=\"a b\" vs=\"here\"/></view>", 1, 37, "vs=\"here\" is not one of parent, step, final"),
        ];
        for (markup, line, column, message) in cases {
            let problem = problem(markup);
            assert_eq!(
                (problem.line, problem.column, problem.message.as_str()),
                (line, column, message),
                "{markup}"
            );
        }
        assert!(problem(&format!("<view/>{}", " ".repeat(MAX_BYTES)))
            .message
            .starts_with("the view is"));
    }

    #[test]
    fn a_name_is_checked_against_what_the_runs_reported() {
        let view = compile(
            "<signal name=\"val_loss\" />\n<view headline=\"loss\">\n  <chart y=\"loss val_los\" />\n  <table name=\"eval\" columns=\"task score\" />\n</view>",
        )
        .unwrap();
        assert_eq!(
            view.check(&Seen::default()).unwrap(),
            vec!["loss", "val_los", "eval", "task", "score"],
            "nothing reported yet: every name may still arrive"
        );
        let seen = Seen {
            signals: vec!["loss".into(), "val_loss".into(), "lr".into()],
            tables: BTreeMap::new(),
        };
        let problem = view.check(&seen).unwrap_err();
        assert_eq!(
            (problem.line, problem.column, problem.message.as_str()),
            (
                3,
                3,
                "blocks[0].chart[1]: no signal \"val_los\"; seen: loss, val_loss, lr"
            )
        );
        assert_eq!(
            problem.text("view.xml", "Run `orx exp view set e1 --file view.xml`."),
            "View view.xml:3:3: blocks[0].chart[1]: no signal \"val_los\"; seen: loss, val_loss, lr. Run `orx exp view set e1 --file view.xml`."
        );

        let fixed = compile("<signal name=\"val_loss\" />\n<view>\n  <chart y=\"loss val_loss\" />\n  <table name=\"eval\" columns=\"task scor\" />\n</view>").unwrap();
        assert_eq!(fixed.check(&seen).unwrap(), vec!["eval", "task", "scor"]);
        let seen = Seen {
            tables: [(
                "eval".to_owned(),
                vec!["task".to_owned(), "score".to_owned()],
            )]
            .into(),
            ..seen
        };
        assert_eq!(
            fixed.check(&seen).unwrap_err().message,
            "blocks[1].columns[1]: table \"eval\" has no column \"scor\"; seen: task, score"
        );
    }

    #[test]
    fn a_derived_view_reads_back_as_itself() {
        let tree = compile(FRONTIER).unwrap().tree;
        assert_eq!(compile(&markup(&tree)).unwrap().tree, tree);
        let over_steps = compile("<signal name=\"loss\" vs=\"step\" />\n<view><chart y=\"loss\" ghost=\"parent\" x-range=\"parent\" /><table name=\"eval\" columns=\"task score\" vs=\"final\" /></view>").unwrap().tree;
        assert_eq!(over_steps["blocks"][0]["xRange"], "parent");
        assert_eq!(compile(&markup(&over_steps)).unwrap().tree, over_steps);
        let default = json!({
            "signals": {"loss": {"label": "loss"}, "lr": {"label": "lr"}},
            "headline": ["loss"],
            "blocks": [{"chart": ["loss"], "x": "elapsed", "ghost": "parent"}, {"table": "eval", "columns": ["task", "score"]}, {"stats": ["lr"]}],
        });
        let text = markup(&default);
        assert_eq!(
            text,
            "<signal name=\"loss\" />\n<signal name=\"lr\" />\n<view headline=\"loss\">\n  <chart y=\"loss\" x=\"elapsed\" ghost=\"parent\" />\n  <table name=\"eval\" columns=\"task score\" />\n  <stats of=\"lr\" />\n</view>\n"
        );
        assert_eq!(compile(&text).unwrap().tree, default);
    }
}
