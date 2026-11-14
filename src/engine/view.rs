//! The run view an agent writes: markup orx parses, checks and serves as a tree. The markup is
//! stored as written and the tree is derived from it on every read, never stored beside it.

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// The elements this orx draws; grows as the slices that feed new blocks land.
pub(crate) const ELEMENTS: [&str; 6] = ["signal", "view", "chart", "series", "stats", "table"];

const MAX_BYTES: usize = 64 * 1024;
const MAX_BLOCKS: usize = 32;
const MAX_SIGNALS: usize = 64;
const MAX_SERIES: usize = 8;
const MAX_DEPTH: usize = 4;

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
                Kind::Signal => {
                    seen.signals.contains(name)
                        || seen.tables.values().any(|columns| columns.contains(name))
                }
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
            "table" => ["name", "columns", "last", "vs"]
                .iter()
                .chain(BLOCK.iter())
                .copied()
                .collect(),
            _ => {
                return Err(self.problem(
                    node,
                    format!(
                        "<{kind}> is not a block; a view holds {}",
                        ["chart", "stats", "table"].join(", ")
                    ),
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
                    format!("<{kind}> holds nothing; write it as <{kind} … />"),
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
            .map(|ghost| self.choice(ghost, &["parent"]))
            .transpose()?;
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
                    if ghost.is_none() {
                        return Err(self.attribute_problem(
                            drawn,
                            "ghost on a <series> says whether the parent's run is drawn under it: add ghost=\"parent\" to the chart".into(),
                        ));
                    }
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
        if let Some(ghost) = ghost {
            block.insert("ghost".into(), json!(ghost));
        }
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
        if node.is_text() && node.text().is_some_and(|t| !t.trim().is_empty()) {
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

/// Has every run of the experiment folded with its patterns from now on: the laptop's copy of
/// each run's configuration, which a host is sent at its next collection. Returns the runs.
pub(crate) fn configure_runs(
    store: &crate::store::Store,
    experiment: &str,
) -> crate::error::Result<Vec<crate::store::StoredRun>> {
    let config = stored_patterns(experiment, store.run_monitor_json(experiment)?.as_deref())?;
    let runs = store.list_runs_by_experiment(experiment)?;
    for run in &runs {
        configure_run(store, run, &config)?;
    }
    Ok(runs)
}

/// Writes the run's configuration beside its checkpoint, when it has patterns or had them.
pub(crate) fn configure_run(
    store: &crate::store::Store,
    run: &crate::store::StoredRun,
    config: &orx_signals::Config,
) -> crate::error::Result<()> {
    let path = store
        .data_dir()
        .join("run-signals")
        .join(format!("{}.json", run.id));
    if config.patterns.is_empty() && !path.with_extension("config.json").exists() {
        return Ok(());
    }
    super::adapters::local::signals::configure(&path, config).map_err(|error| {
        crate::error::anyhow!("Run {}'s patterns could not be saved: {error}", run.id)
    })?;
    Ok(())
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
    let seen = match folded {
        Some(folded) => Seen::of(folded),
        None => super::monitor::checkpoint(store, run)?
            .map(|s| Seen::of(&s))
            .unwrap_or_default(),
    };
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
        report["stages"] = folded
            .phases
            .iter()
            .map(|p| json!({"name": p.name, "done": p.done, "of": p.of, "unit": p.unit, "source": p.source}))
            .collect();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
            ("<view><html/></view>", 1, 7, "unknown element \"html\"; allowed: signal, view, chart, series, stats, table"),
            ("<view>\n  <chart y=\"loss\" scale=\"log\"/>\n</view>", 2, 19, "unknown attribute \"scale\" on <chart>; allowed: y, x, from, by, x-scale, y-scale, x-range, mark, ghost, phase, title"),
            ("<signal name=\"loss\" digits=\"13\"/><view/>", 1, 21, "digits=\"13\" must be a whole number from 0 to 12"),
            ("<signal name=\"loss\" better=\"down\"/><view/>", 1, 21, "better=\"down\" is not one of lower, higher"),
            ("<signal name=\"a\"/><signal name=\"a\"/><view/>", 1, 19, "signal \"a\" is declared twice"),
            ("<signal name=\"a\"/>", 1, 1, "a view needs one <view> holding its blocks, after its <signal> declarations"),
            ("<view/><view/>", 1, 8, "a view has exactly one <view>"),
            ("<chart y=\"a\"/><view/>", 1, 1, "<chart> goes inside <view>; only <signal> and <view> stand alone"),
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
            ("<view><chart from=\"t\"><series x=\"b\" y=\"c\" ghost=\"false\"/></chart></view>", 1, 43, "ghost on a <series> says whether the parent's run is drawn under it: add ghost=\"parent\" to the chart"),
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
            "blocks": [{"chart": ["loss"], "x": "elapsed"}, {"table": "eval", "columns": ["task", "score"]}, {"stats": ["lr"]}],
        });
        let text = markup(&default);
        assert_eq!(
            text,
            "<signal name=\"loss\" />\n<signal name=\"lr\" />\n<view headline=\"loss\">\n  <chart y=\"loss\" x=\"elapsed\" />\n  <table name=\"eval\" columns=\"task score\" />\n  <stats of=\"lr\" />\n</view>\n"
        );
        assert_eq!(compile(&text).unwrap().tree, default);
    }
}
