//! Structural checks over every text an agent reads: the playbook, the `orx
//! skill` overview, the bundled skills, the slash-skill expansions and the
//! prompts orx installs or sends. `write_agent_surface` exports the same text,
//! with the CLI and MCP tools, for `scripts/agent-surface.mjs`.

use clap::{CommandFactory, Parser};
use serde_json::{json, Value};

use super::agent_skills::{self, SkillSet};
use super::model::LocalProject;
use super::opencode::{playbook_md, ProjectState};

pub(crate) fn sample_project() -> LocalProject {
    LocalProject {
        id: "proj_test".into(),
        name: "Test Project".into(),
        slug: "test-project".into(),
        github_owner: "acme".into(),
        github_repo: "widget".into(),
        github_sync_enabled: true,
        baseline_branch: "main".into(),
        repo_path: "/tmp/nonexistent".into(),
        run_command: None,
        paper_id: None,
        icon: None,
        proposal: None,
        created_at: 0,
        updated_at: 0,
    }
}

fn scratch_project() -> LocalProject {
    let mut project = sample_project();
    project.repo_path = super::projects::scratch_root()
        .join(&project.id)
        .to_string_lossy()
        .into_owned();
    project
}

/// A chat without a project learns how to make one, and is never told its
/// temporary folder is the user's checkout.
#[test]
fn a_chat_without_a_project_is_told_how_to_make_one() {
    let scratch = playbook_md(&scratch_project(), &ProjectState::default());
    assert!(scratch.contains("orx project create --name"));
    assert!(!scratch.contains("Project folder"));
    let project = playbook_md(&sample_project(), &ProjectState::default());
    assert!(project.contains("- Project folder: `/tmp/nonexistent`"));
    assert!(!project.contains("orx project create"));
}

/// Text rendered from a template at runtime, labelled for failure messages.
fn rendered() -> Vec<(String, String)> {
    let mut local = sample_project();
    local.github_sync_enabled = false;
    local.paper_id = Some("2401.12345".into());
    let mut out = vec![
        (
            "playbook".into(),
            playbook_md(&sample_project(), &ProjectState::default()),
        ),
        (
            "local playbook".into(),
            playbook_md(&local, &ProjectState::default()),
        ),
        (
            "scratch playbook".into(),
            playbook_md(&scratch_project(), &ProjectState::default()),
        ),
    ];
    for skill in super::skills::CATALOG {
        for github_enabled in [true, false] {
            for has_request in [true, false] {
                let text = super::skills::instructions(skill.name, has_request, github_enabled);
                let variant = match (github_enabled, has_request) {
                    (true, true) => String::new(),
                    (github, request) => format!(
                        " ({}{})",
                        if github { "GitHub" } else { "local" },
                        if request { "" } else { ", no request" }
                    ),
                };
                out.push((format!("/{}{variant}", skill.name), text.unwrap()));
            }
        }
    }
    out
}

/// Prompts sent or installed as written.
fn fixed_prompts() -> Vec<(String, String)> {
    [
        ("chat-format.md", super::opencode::CHAT_FORMAT),
        ("orx skill", crate::commands::skill::SKILL_MD),
        ("global orx skill", super::harness::CLAUDE_SKILL),
        ("Codex /orx prompt", super::harness::CODEX_PROMPT),
        (
            "alphaXiv MCP instructions",
            crate::commands::alphaxiv_mcp::INSTRUCTIONS,
        ),
        (
            "compaction system prompt",
            super::chat::COMPACT_SYSTEM_PROMPT,
        ),
        ("starter system prompt", super::starter::SYSTEM_PROMPT),
    ]
    .map(|(label, text)| (label.into(), text.into()))
    .into()
}

fn all_text() -> Vec<(String, String)> {
    let mut out = rendered();
    out.extend(fixed_prompts());
    let skills = [SkillSet::Local, SkillSet::Full].map(agent_skills::skills);
    for skill in skills.into_iter().flatten() {
        out.push((skill.name.into(), skill.content.into()));
        out.push((format!("{} description", skill.name), skill.description()));
        for resource in skill.resources {
            out.push((
                format!("{}/{}", skill.name, resource.path),
                resource.content.into(),
            ));
        }
    }
    out
}

/// The same text is served in the desktop app, a terminal and other agents'
/// own UIs, so it may not steer by orx's screens.
#[test]
fn agent_text_is_ui_agnostic() {
    for (label, text) in all_text() {
        let text = text.to_lowercase();
        for phrase in [
            "git tab",
            "settings →",
            "orx up settings",
            "hugging face section",
            "artifacts tab",
            "dashboard artifacts",
            "dashboard's",
            "dashboard injects",
            "invisible to the dashboard",
            "visible in the dashboard",
            "chat ui",
            "plan card",
            "file viewer",
            "web ui",
            "\"test\" button",
            "cancel from web",
            "cancel from the web",
        ] {
            assert!(
                !text.contains(phrase),
                "{label} contains UI navigation: {phrase}"
            );
        }
    }
}

/// Scanned rather than listed: a token a template gains but its renderer does
/// not substitute would reach the agent as a literal `{token}`.
#[test]
fn rendered_text_has_no_unresolved_placeholders() {
    for (label, text) in rendered() {
        let leftover: Vec<&str> = text
            .match_indices('{')
            .filter_map(|(i, _)| {
                let token = &text[i + 1..i + 1 + text[i + 1..].find('}')?];
                let is_token =
                    !token.is_empty() && token.chars().all(|c| c.is_ascii_lowercase() || c == '_');
                is_token.then_some(token)
            })
            .collect();
        assert!(leftover.is_empty(), "{label}: unresolved {leftover:?}");
        assert!(!text.contains("<!--"), "{label}: template comment leaked");
    }
}

/// A bare command name is a reference, so a missing argument passes, as does a
/// placeholder where a typed value belongs; an unknown subcommand or flag does not.
#[test]
fn every_orx_command_in_agent_text_parses() {
    use clap::error::{ContextKind, ContextValue, ErrorKind};

    let mut checked = 0;
    let mut failures = Vec::new();
    for (label, text) in all_text() {
        for span in orx_spans(&text) {
            for args in command_variants(span) {
                checked += 1;
                let argv = std::iter::once("orx".to_owned()).chain(args);
                let Err(e) = crate::Cli::try_parse_from(argv) else {
                    continue;
                };
                let placeholder_value = matches!(
                    e.get(ContextKind::InvalidValue),
                    Some(ContextValue::String(value)) if value == PLACEHOLDER
                );
                let reference = placeholder_value
                    || matches!(
                        e.kind(),
                        ErrorKind::DisplayHelp
                            | ErrorKind::MissingRequiredArgument
                            | ErrorKind::MissingSubcommand
                            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                    );
                if !reference {
                    failures.push(format!("{label}: `orx{span}`: {e}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(checked > 100, "only {checked} commands found");
}

/// An agent copies the worked views, patterns and plans as they are, so each must save as written.
#[test]
fn every_view_pattern_and_plan_in_agent_text_compiles() {
    use crate::engine::view::{self, Refusal, Source};

    let refused = |refusal: Refusal| match refusal {
        Refusal::Invalid(why) => why,
        _ => "refused".to_owned(),
    };
    let source = Source {
        file: None,
        patterns: Some("patterns.json"),
        config: Some("plan.json"),
    };
    let mut checked = 0;
    let mut failures = Vec::new();
    for (label, text) in all_text() {
        for block in text.split("```").skip(1).step_by(2) {
            let (language, body) = block.split_once('\n').unwrap_or((block, ""));
            let problem = match language.trim() {
                "html" if body.contains("<view") => view::compile(body)
                    .err()
                    .map(|problem| problem.text("view.xml", "")),
                "json" => match serde_json::from_str(body) {
                    Err(error) => Some(error.to_string()),
                    Ok(given) if body.contains("\"regex\"") => {
                        view::patterns(&given, &source, "").err().map(refused)
                    }
                    Ok(given)
                        if ["\"plan\"", "\"rules\"", "\"brief\"", "\"watch\""]
                            .iter()
                            .any(|key| body.contains(key)) =>
                    {
                        view::config_of(&given, &source, "").err().map(refused)
                    }
                    Ok(_) => continue,
                },
                _ => continue,
            };
            checked += 1;
            if let Some(problem) = problem {
                failures.push(format!("{label}: {problem}\n{body}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    assert!(
        checked >= 10,
        "only {checked} views, patterns and plans found"
    );
}

/// A run prints the documented `::orx::` lines as they are, so each must fold without a complaint.
#[test]
fn every_orx_line_in_agent_text_folds() {
    let mut checked = 0;
    for (label, text) in all_text() {
        for line in text
            .lines()
            .filter_map(|line| line.trim().strip_prefix("::orx:: "))
        {
            let mut snapshot = orx_signals::Snapshot::new("run", "log", "1");
            snapshot
                .feed(format!("::orx:: {line}\n").as_bytes(), true, 0, false)
                .unwrap();
            assert!(
                snapshot.diagnostics.counts.is_empty(),
                "{label}: `::orx:: {line}` folds with {:?}",
                snapshot.diagnostics.counts
            );
            checked += 1;
        }
    }
    assert!(checked >= 3, "only {checked} ::orx:: lines found");
}

const PLACEHOLDER: &str = "PLACEHOLDER";

/// What follows `orx` in each code span and line that runs it.
fn orx_spans(text: &str) -> impl Iterator<Item = &str> {
    text.split('`')
        .chain(text.lines())
        .filter_map(|s| s.trim().strip_prefix("orx"))
        .filter(|span| span.is_empty() || span.starts_with(char::is_whitespace))
}

/// Expands a documented command line into concrete argument vectors: `<placeholder>`
/// becomes one argument, each `[optional]` or `(choice)` group is included, a
/// `|` inside a group or a `move/archive` word yields one variant per choice,
/// and a trailing `# comment`, `> redirect` or `\` continuation is dropped.
fn command_variants(span: &str) -> Vec<Vec<String>> {
    let mut line = String::new();
    let mut in_placeholder = false;
    for c in span
        .replace("\\|", "|")
        .replace("...", "")
        .replace('…', "")
        .chars()
    {
        match c {
            '<' if !in_placeholder => {
                in_placeholder = true;
                line.push_str(PLACEHOLDER);
            }
            '>' if in_placeholder => in_placeholder = false,
            _ if !in_placeholder => line.push(c),
            _ => {}
        }
    }
    let line = line.split(" #").next().unwrap();
    let line = line.split(" >").next().unwrap();
    let line = line.trim().trim_end_matches('\\');

    let mut variants = vec![line.to_owned()];
    while let Some(open) = variants[0].rfind(['[', '(']) {
        variants = variants
            .iter()
            .flat_map(|v| {
                let close = open + v[open..].find([']', ')']).expect("unclosed group");
                let (head, tail) = (&v[..open], &v[close + 1..]);
                v[open + 1..close]
                    .split('|')
                    .map(move |choice| format!("{head}{}{tail}", choice.trim()))
                    .collect::<Vec<_>>()
            })
            .collect();
    }

    variants
        .iter()
        .flat_map(|v| {
            let words = shell_words(v);
            let alternatives = words.iter().position(|w| {
                w.contains('/')
                    && w.split('/')
                        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase()))
            });
            match alternatives {
                Some(i) => words[i]
                    .split('/')
                    .map(|choice| {
                        let mut words = words.clone();
                        words[i] = choice.to_owned();
                        words
                    })
                    .collect(),
                None => vec![words],
            }
        })
        .collect()
}

fn shell_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut quote = None;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.get_or_insert_default().push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                word.get_or_insert_default();
            }
            (None, c) if c.is_whitespace() => words.extend(word.take()),
            (None, c) => word.get_or_insert_default().push(c),
        }
    }
    words.extend(word);
    words
}

/// Exports what agents are given as JSON at `$ORX_AGENT_SURFACE`.
#[test]
#[ignore = "run by scripts/agent-surface.mjs"]
fn write_agent_surface() {
    use crate::commands::alphaxiv_mcp::{LIBRARY_READ_TOOLS, RESEARCH_TOOLS, WRITE_TOOLS};
    use crate::commands::up::agent_tools;

    let path = std::env::var_os("ORX_AGENT_SURFACE")
        .expect("ORX_AGENT_SURFACE names the output file; run node scripts/agent-surface.mjs");

    let tools = agent_tools::tools("claude");
    let alphaxiv: Vec<Value> = [
        ("research", RESEARCH_TOOLS),
        ("library", LIBRARY_READ_TOOLS),
        ("write", WRITE_TOOLS),
    ]
    .into_iter()
    .flat_map(|(access, names)| {
        names
            .iter()
            .map(move |name| json!({ "name": name, "access": access }))
    })
    .collect();

    let full = agent_skills::skills(SkillSet::Full);
    let playbook = playbook_md(&sample_project(), &ProjectState::default());
    let mut prompts = vec![json!({ "label": "playbook", "content": playbook })];
    let mut texts = vec![("playbook".to_owned(), playbook)];
    for slash in super::skills::CATALOG {
        let label = format!("/{}", slash.name);
        if let Some(skill) = full.iter().find(|skill| skill.content == slash.template) {
            prompts.push(json!({ "label": label, "expands": skill.name }));
            continue;
        }
        let text = super::skills::instructions(slash.name, true, true).unwrap();
        prompts.push(json!({ "label": label, "content": text }));
        texts.push((label, text));
    }
    for (label, text) in fixed_prompts() {
        prompts.push(json!({ "label": label, "content": text }));
        texts.push((label, text));
    }
    texts.push(("orx MCP tools".to_owned(), tools.to_string()));

    let local: Vec<&str> = agent_skills::skills(SkillSet::Local)
        .iter()
        .map(|skill| skill.name)
        .collect();
    let mut skills = Vec::new();
    for skill in full {
        texts.push((skill.name.to_owned(), skill.content.to_owned()));
        texts.push((skill.name.to_owned(), skill.description()));
        // Scripts and templates are run or copied, not read.
        let docs = skill.resources.iter().filter(|r| r.path.ends_with(".md"));
        let mut resources = Vec::new();
        for resource in docs {
            texts.push((
                format!("{}/{}", skill.name, resource.path),
                resource.content.to_owned(),
            ));
            resources.push(json!({ "path": resource.path, "content": resource.content }));
        }
        skills.push(json!({
            "name": skill.name,
            "description": skill.description(),
            "content": skill.content,
            "inLocalSet": local.contains(&skill.name),
            "resources": resources,
        }));
    }

    let root = crate::Cli::command();
    let mut mentions = Vec::new();
    for (label, text) in &texts {
        for span in orx_spans(text) {
            let variants = command_variants(span);
            let Some(first) = variants.first() else {
                continue;
            };

            let mut command = &root;
            let mut path = Vec::new();
            for word in first {
                if let Some(sub) = command.find_subcommand(word) {
                    path.push(sub.get_name().to_owned());
                    command = sub;
                }
            }

            let mut flags: Vec<&str> = variants
                .iter()
                .flatten()
                .filter(|word| word.starts_with('-'))
                .map(|word| word.split('=').next().unwrap())
                .collect();
            flags.sort();
            flags.dedup();
            mentions.push(json!({ "text": label, "command": path.join(" "), "flags": flags }));
        }
    }

    let surface = json!({
        "mcp": {
            "approve": crate::commands::mcp::approve_tool(),
            "tools": tools,
            "antigravityTools": agent_tools::tools("antigravity"),
            "alphaxiv": alphaxiv,
        },
        "cli": command_json(&root, ""),
        "mentions": mentions,
        "prompts": prompts,
        "skills": skills,
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&surface).unwrap()).unwrap();
}

fn command_json(command: &clap::Command, parent: &str) -> Value {
    let path = if command.get_name() == "orx" {
        String::new()
    } else {
        format!("{parent} {}", command.get_name()).trim().to_owned()
    };
    let args: Vec<Value> = command
        .get_arguments()
        .filter(|arg| !matches!(arg.get_id().as_str(), "help" | "version"))
        .map(|arg| {
            let values: Vec<String> = arg
                .get_possible_values()
                .iter()
                .filter(|value| !value.is_hide_set())
                .map(|value| value.get_name().to_owned())
                .collect();
            json!({
                "id": arg.get_id().as_str(),
                "long": arg.get_long(),
                "short": arg.get_short().map(String::from),
                "aliases": arg.get_all_aliases().unwrap_or_default(),
                "help": arg.get_help().map(|help| help.to_string()),
                "hidden": arg.is_hide_set(),
                "required": arg.is_required_set(),
                "positional": arg.is_positional(),
                "values": values,
            })
        })
        .collect();
    let subcommands: Vec<Value> = command
        .get_subcommands()
        .filter(|sub| sub.get_name() != "help")
        .map(|sub| command_json(sub, &path))
        .collect();
    json!({
        "name": command.get_name(),
        "path": path,
        "about": command.get_about().map(|about| about.to_string()),
        "longAbout": command.get_long_about().map(|about| about.to_string()),
        "aliases": command.get_all_aliases().collect::<Vec<_>>(),
        "hidden": command.is_hide_set(),
        "args": args,
        "subcommands": subcommands,
    })
}
