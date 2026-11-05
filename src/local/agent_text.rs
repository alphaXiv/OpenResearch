//! Structural checks over every text an agent reads: the playbook, the `orx
//! skill` overview, the bundled skills and the slash-skill expansions.

use clap::Parser;

use super::agent_skills::{self, SkillSet};
use super::model::LocalProject;
use super::opencode::{playbook_md, ProjectState};

pub(super) fn sample_project() -> LocalProject {
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
        created_at: 0,
        updated_at: 0,
    }
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
    ];
    for skill in super::skills::CATALOG {
        for github_enabled in [false, true] {
            for has_request in [false, true] {
                let text = super::skills::instructions(skill.name, has_request, github_enabled);
                out.push((format!("/{}", skill.name), text.unwrap()));
            }
        }
    }
    out
}

fn all_text() -> Vec<(String, String)> {
    let mut out = rendered();
    out.push(("SKILL.md".into(), crate::commands::skill::SKILL_MD.into()));
    let skills = [SkillSet::Local, SkillSet::Full].map(agent_skills::skills);
    for skill in skills.into_iter().flatten() {
        out.push((skill.name.into(), skill.content.into()));
        out.push((
            format!("{} description", skill.name),
            skill.description.into(),
        ));
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
        let spans = text.split('`').chain(text.lines());
        for span in spans.filter_map(|s| s.trim().strip_prefix("orx")) {
            if !(span.is_empty() || span.starts_with(char::is_whitespace)) {
                continue;
            }
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

const PLACEHOLDER: &str = "PLACEHOLDER";

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
