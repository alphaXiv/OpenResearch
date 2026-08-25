//! Slash-skills for the `orx up` chat — canned prompt templates the user
//! invokes as `/name <args>` from the composer. The UI lists them via
//! `/api/skills`; expansion happens server-side in `ChatHost::send_message`
//! so the transcript keeps the short `/name` form the user typed while the
//! harness receives the full prompt. Works identically across all harnesses.

/// One built-in slash workflow. Its template receives the shared user request.
pub struct Skill {
    pub name: &'static str,
    pub description: &'static str,
    pub template: &'static str,
    /// Substituted for `{request}` when slash tokens are the whole message.
    pub empty_request: &'static str,
}

const LIT_REVIEW_TEMPLATE: &str = include_str!("../../agent-skills/orx-lit-review/SKILL.md");

const REPRODUCE_PAPER_TEMPLATE: &str = r#"Reproduce a research paper claim by claim on the user's compute.

Paper and compute: {request}

Before running anything:
1. Confirm the compute. The user should name where runs execute — a configured `~/.ssh/config` host alias (`orx exp run --backend ssh --host <alias>`), another `orx` backend (`hf` or `modal` with a flavor, `k8s` with a committed manifest), or the local machine. If unspecified, use the configured default compute target when one is set (omit `--backend` to launch there); otherwise ask before launching anything.
2. Read the paper. If the args name no paper, infer it from the current repository — read the README, docs, and code, and if the repo clearly corresponds to an identifiable paper, reproduce that one; only ask the user if none can be identified. If it's on alphaXiv, `orx paper <id>` gives a structured report (`--full` for raw text); use the `orx-lit-review` retrieval workflow to find it. Otherwise ask the user for a PDF or link.
3. Plan to the user's compute window. When the caller supplies an absolute deadline and available accelerator capacity, treat both as authoritative: keep the available GPUs occupied with scientifically useful parallel variants, seeds, ablations, controls, or profiling runs; refill freed capacity after each completion; and stop early when the target claims are adequately evaluated. Interpret capacity by total GPUs across in-flight runs, not by raw run count. Do not invent or maintain a GPU-hour ledger unless the user explicitly asks for one. For vague small-budget language such as "for a little bit," prefer published-checkpoint evaluation and targeted checks. Larger windows may support broader sweeps, added seeds, fine-tuning, or retraining, but they make training eligible, not mandatory.
4. Optional tracking: if the user wants metrics logged, prefer Weights & Biases — check `wandb login` / `WANDB_API_KEY` and log each run to a project named after the paper. Don't require it.

Workflow:
1. Enumerate the paper's main empirical claims (headline table/figure results first). Unless the user specifies, focus on the main illustrative claim of the paper.
2. Reproduce claim by claim on the agreed compute. Simplified setups and toy-scale runs are fine when full scale is out of budget — say so explicitly when you downscale.
3. For each claim, record the paper's result, the observed result, an assessment (aligned / partially aligned / inconclusive under this setup / not attempted), and the compute cost. When results diverge, state that this run did not show the reported effect, quantify the difference, and explain relevant uncertainty or substitutions. Do not characterize the claim as wrong, incorrect, failed, or "not reproduced," and do not infer beyond the tested setup.
4. Finish with a summary: per-claim assessments, where results diverged and why, and what a full-scale reproduction would still need.

Write a visual autoresearch report:
- Write for readers who may not understand the paper. Lead with its central question, then explain the implementation, experiments, and evidence.
- Open with evidence: place the strongest result figure immediately after the title, before the main explanatory prose. It should give readers an immediate visual understanding of the reproduction's central result.
- Aim for four or five distinct, evidence-bearing figures when the results support them—for example, a headline result, mechanism or training curve, robustness comparison, and diagnostic or negative control. Use fewer when the available evidence does not justify them; never pad the report with decorative or redundant plots.
- Make the report implementation-led rather than a run log. Trace the important code path, consequential design choices, and the smallest code or configuration changes used to test them.
- Use figures, compact tables, diagrams, and short code excerpts when they explain the result better than prose. Avoid long uninterrupted text, repeated conclusions, and exhaustive infrastructure histories.
- Clearly separate paper evidence, observed evidence, divergent or inconclusive results, partial runs, and unattempted claims. End with a concise assessment and descriptive links to the relevant experiment branches.
- Use one clear title and normal Markdown hierarchy: H2 for major sections and H3 only for genuine subsections.
- Keep the report self-contained: store figures in an `images/` directory beside `report.md`, reference them as `images/<filename>`, and verify every image renders before publication.
- Perform a final editorial pass for clarity and concision. The result should feel like an illustrated technical article, not an experiment database dump.

Publish a polished GitHub artifact:
- Treat the repository README as the public landing page, not as an afterthought. Add a project-specific reproduction section at the very top, before any upstream README content. It must state which paper claim was tested, what was done, the assessment, the paper number versus the observed number, the downscaling/substitutions, the agreed compute, and links to the detailed report or notebook when present.
- In that top section, add a compact `Experiment log` or provenance table covering the important branches only. Use descriptive links to each branch and include columns for branch/experiment, purpose or change, **exact run command**, assessment/outcome, and compute. Copy the command verbatim from `orx exp status`; do not abbreviate it, replace it with pseudocode, or show only the entrypoint.
- Account for `main` explicitly. If a formal experiment was ever launched from `main`, include `main` in the table with the exact command and result. If `main` is presentation-only, say `Not run as an experiment (publication surface)` rather than inventing a command.
- Publish every reader-facing report on `main`, alongside the README and other small presentation artifacts. A report is not considered published if it exists only in the project artifacts directory, an `orx/*` experiment branch, or an internal run log. Copy or recreate the final report under a clear repository path such as `reports/<topic>/report.md` or `artifacts/<topic>/report.md`, then add a descriptive link to it in the README's top reproduction section. If several reports are produced, link every important one and briefly say what each contains.
- Also publish a self-contained, tutorial-style marimo notebook on `main` that explains the central claim and opens with the already-produced evidence; do not make readers rerun expensive experiments to see the result. Validate it with `marimo check <notebook.py>`, embed small results or fetch them from public URLs instead of assuming repository-relative artifacts exist in Molab, and keep optional interactive work bounded and separate from formal reproduction evidence. If the repository is public—or the user explicitly requested public publication and its history is safe to expose—add and verify `[![Open in molab](https://marimo.io/molab-shield.svg)](https://molab.marimo.io/github/<owner>/<repo>/blob/main/<notebook.py>)` in the README. Otherwise preserve private visibility, omit the unusable Molab link, and include concise local `marimo edit <notebook.py>` and `marimo run <notebook.py>` instructions. Never change repository visibility without explicit authorization.
- Include failed branches only when they explain the lineage to the successful result. Keep raw experiment and run IDs in `orx exp desc`, not in the README.
- Keep the README current whenever another important branch is run or its assessment changes. A reader landing on GitHub should be able to understand what was tried and reproduce the command without inspecting the internal experiment database.
"#;

const REPRODUCE_PAPER_LOCAL_TEMPLATE: &str = r#"Reproduce a research paper claim by claim in this unpublished project.

Paper and compute: {request}

Use the configured local repository and `orx` experiment tree. Read the paper,
enumerate its empirical claims, and choose the smallest honest reproduction
that fits the available compute. Create experiment nodes for meaningful variants,
make each change on its printed local branch, and commit it. Use the configured
default compute target, or the explicit backend and flavor/host the user chose;
source snapshots do not require GitHub. Keep the inherited run command fixed.
Wait with `orx exp wait --project <project-id>`, inspect `orx runs`, and
read every terminal run with `orx logs <run-id>`.

Record the paper number, observed number, scale or substitutions, sample size,
scoring method, runtime, and an honest assessment in `orx exp desc`. Repair a
node in place when a run answers nothing; branch a child after a meaningful
result.

Produce a self-contained local report under the project's Artifacts directory,
with figures in an adjacent `images/` folder. Open with the strongest result,
separate paper evidence from observed evidence, and link experiment branches by
name without assuming hosted URLs. Do not publish, change repository visibility,
or contact a Git hosting service unless they explicitly request publication.
"#;

const WRITE_PAPER_TEMPLATE: &str = r#"Draft a paper or preprint on this project's work, as LaTeX.

Scope: {request}

Load the `orx-paper` skill first (`orx skill paper`) and follow it — it carries a
preamble that compiles, the environment-to-package table that is the usual cause
of a failed build, and the file-citation rule.

Check `.orx/latex-templates/` before writing a preamble. If the user has uploaded
exactly one template, use it; if several, ask which unless the scope above names
one; if none, use the skill's default preamble.

Method:
1. Ground the paper in what actually ran. Read the tree with `orx project view
   <projectId>`, then pull every number you intend to report out of `orx logs`
   (the `orx-evidence` skill covers this). Never write a metric you have not read
   out of a run; if something is not measured yet, say so in the text.
2. Load `orx-lit-review`, retrieve real related work with `orx discover`, and
   read the selected sources with `orx paper <id>`. Cite those. Do not invent
   plausible-looking references.
3. Write the document to `paper.tex` at the repository root, in the working tree
   — a copy in the artifacts directory is not compiled.
4. Structure it as abstract, introduction, method, experiments (a table of
   measured results), related work, conclusion, and an inline `thebibliography`.
5. Link the finished file using the session playbook's evidence-and-links contract
   so the user can open the rendered document.
6. If the build fails, read the first line of the log beginning with `!` — it
   names the problem and the source line — fix the source, and build again. Do
   not hand back a document that does not compile.
"#;

pub const CATALOG: &[Skill] = &[
    Skill {
        name: "lit-review",
        description: "Multi-hop literature review across alphaXiv, OpenAlex, and bioRxiv",
        template: LIT_REVIEW_TEMPLATE,
        empty_request: "(none given — ask the user what topic to review before searching)",
    },
    Skill {
        name: "reproduce-paper",
        description: "Reproduce a paper's headline claims",
        template: REPRODUCE_PAPER_TEMPLATE,
        empty_request: "(none given — reproduce the linked paper named in your instructions (the `Paper:` line, the one this project starts from) when present; otherwise infer the paper from the current repository: read the README, docs, and code, and if the repo clearly corresponds to an identifiable paper, reproduce that one; only ask the user if no paper can be identified. For compute, use the configured default target when one is set, per the rules below; otherwise ask before launching.)",
    },
    Skill {
        name: "write-paper",
        description: "Draft a paper or preprint as LaTeX, grounded in this project's runs",
        template: WRITE_PAPER_TEMPLATE,
        empty_request: "(none given — infer the scope from this project: read the experiment tree and the runs that have finished, and write the paper about the work that is actually there. Ask the user only if the project has no results worth writing up.)",
    },
];

/// Expand one selected workflow. The complete request is appended once by the
/// chat layer, even when several workflows are selected.
pub fn instructions(name: &str, has_request: bool, github_enabled: bool) -> Option<String> {
    let skill = CATALOG
        .iter()
        .find(|skill| skill.name.eq_ignore_ascii_case(name))?;
    if skill.name == "lit-review" {
        let after_open = skill
            .template
            .strip_prefix("---\n")
            .or_else(|| skill.template.strip_prefix("---\r\n"))?;
        let end = after_open.find("\n---")?;
        let mut content = after_open[end + 4..]
            .trim_start_matches(['\r', '\n'])
            .to_string();
        if !has_request {
            content.push_str("\n\nRequest context: ");
            content.push_str(skill.empty_request);
        }
        return Some(content);
    }
    let request = if has_request {
        "the complete user request below"
    } else {
        skill.empty_request
    };
    let template = if github_enabled {
        skill.template
    } else {
        match skill.name {
            "reproduce-paper" => REPRODUCE_PAPER_LOCAL_TEMPLATE,
            _ => skill.template,
        }
    };
    Some(template.replace("{request}", request))
}

#[cfg(test)]
mod tests {
    fn expand(name: &str, has_request: bool) -> Option<String> {
        super::instructions(name, has_request, true)
    }

    #[test]
    fn expands_known_skill_with_shared_request() {
        let out = expand("lit-review", true).unwrap();
        assert!(out.contains("# Literature retrieval"));
        assert!(out.contains("Main-agent retrieval loop"));
        assert!(!out.contains("Load and follow the `orx-lit-review` skill"));
    }

    #[test]
    fn expands_bare_invocation_to_ask() {
        let out = expand("lit-review", false).unwrap();
        assert!(out.contains("ask the user"));
    }

    #[test]
    fn expands_reproduce_paper_skill() {
        let out = expand("reproduce-paper", true).unwrap();
        assert!(out.contains("Paper and compute: the complete user request below"));
        assert!(out.contains("Confirm the compute"));
        assert!(out.contains("before any upstream README content"));
        assert!(out.contains("exact run command"));
        assert!(out.contains("Not run as an experiment (publication surface)"));
        assert!(out.contains("every reader-facing report on `main`"));
        assert!(out.contains("not considered published"));
        assert!(out.contains("strongest result figure immediately after the title"));
        assert!(out.contains("four or five distinct, evidence-bearing figures"));
        assert!(out.contains("implementation-led rather than a run log"));
        assert!(out.contains("images/<filename>"));
        assert!(out.contains("illustrated technical article"));
        assert!(out.contains("inconclusive under this setup"));
        assert!(out.contains("this run did not show the reported effect"));
        assert!(out.contains("Do not characterize the claim as wrong"));
        assert!(out.contains("per-claim assessments"));
        assert!(out.contains("absolute deadline and available accelerator capacity"));
        assert!(out.contains("total GPUs across in-flight runs"));
        assert!(out.contains("Do not invent or maintain a GPU-hour ledger"));
        assert!(out.contains("published-checkpoint evaluation"));
        assert!(out.contains("training eligible, not mandatory"));
        assert!(out.contains("self-contained, tutorial-style marimo notebook"));
        assert!(out.contains("marimo check <notebook.py>"));
        assert!(out.contains("already-produced evidence"));
        assert!(
            out.contains("https://molab.marimo.io/github/<owner>/<repo>/blob/main/<notebook.py>")
        );
        assert!(out.contains("preserve private visibility"));
        assert!(out.contains("marimo edit <notebook.py>"));
        assert!(out.contains("Never change repository visibility without explicit authorization"));
        assert!(!out.contains("trackio"));
        let bare = expand("reproduce-paper", false).unwrap();
        assert!(bare.contains("infer the paper from the current repository"));
        assert!(bare.contains("only ask the user if no paper can be identified"));
    }

    #[test]
    fn passes_through_unknown_or_plain_text() {
        assert!(expand("unknown", true).is_none());
        assert!(expand("icml-repro", true).is_none());
        assert!(expand("paper-to-marimo", true).is_none());
    }

    #[test]
    fn local_reproduction_keeps_artifacts_local() {
        let reproduce = super::instructions("reproduce-paper", true, false).unwrap();
        assert!(reproduce.contains("unpublished"));
        assert!(!reproduce.contains("git push"));
        assert!(!reproduce.contains("public GitHub"));
    }

    #[test]
    fn write_paper_targets_a_compilable_tex_in_the_working_tree() {
        let prompt = super::instructions("write-paper", true, false).unwrap();
        assert!(prompt.contains("the complete user request below"));
        assert!(prompt.contains("paper.tex"));
        assert!(
            prompt.contains("not compiled"),
            "must steer away from artifacts"
        );
        assert!(
            prompt.contains("orx-paper"),
            "must load the skill that has the preamble"
        );
        // No-args form still has to produce a paper, not a question.
        let bare = super::instructions("write-paper", false, false).unwrap();
        assert!(bare.contains("infer the scope from this project"));
    }

    #[test]
    fn expanded_skills_avoid_openresearch_ui_navigation() {
        for skill in super::CATALOG {
            for github_enabled in [false, true] {
                for has_request in [false, true] {
                    let prompt =
                        super::instructions(skill.name, has_request, github_enabled).unwrap();
                    crate::local::assert_agent_guidance_is_ui_agnostic(skill.name, &prompt);
                }
            }
        }
    }
}
