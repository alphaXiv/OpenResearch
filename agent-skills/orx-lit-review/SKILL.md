---
name: orx-lit-review
description: "Answer from the research literature. Use before explaining or comparing concepts, judging a research claim, or writing related work, even when no paper is named."
---

# Literature retrieval

Use this skill for scientific explanations and comparisons, even without a named
paper. Never delegate retrieval to a sub-agent.

Use enabled literature connectors appropriate to the topic. General web search
is a fallback only when relevant connectors provide no useful evidence. Do not
supplement successful retrieval with a web search for a familiar or preferred
paper; use a focused connector query or `orx paper`. Opening a selected original
PDF to extract evidence is source access; opening its abstract first is unnecessary.

Each `references/<name>.md` below sits beside this skill. If a reference beside
this skill can't be read, `orx skill lit-review/<name>` prints it.

## Scale retrieval to the question

- A set-of-papers request ("find papers", "top papers", "what is out there",
  "what should I read") returns the ranked discovery results and stops.
- Claim-level synthesis, methodological detail or comparison: finish retrieval,
  then read the 3–5 most load-bearing candidates with `orx paper <id>` (or the
  number the user requested). Do not narrow to 3–5 papers before retrieval has
  produced its ranked 5–15 candidate set.
- Explanations and comparisons lead with original figures, tables and diagrams.

Read when:
- `references/library.md`: the user asks about their papers, library, notes,
  highlights or folders, or about a researcher.
- `references/reading.md`: you open a selected paper with `orx paper`.
- `references/visual-evidence.md`: before explaining or comparing, to pick,
  crop and embed original visuals.

## Retrieval loop

```sh
orx discover keyword "<exact keyword query>"
orx discover embedding "<semantic description in the user's terms>"
orx discover openalex "<scholarly search query>"
orx discover biorxiv "<biology preprint query>"
orx discover pubmed "<biomedical query>"
```

alphaXiv's tools are connectors too: `discover_papers` ranks candidates for a
whole request in one call, and `answer_pdf_queries` returns the pages of one
paper that answer specific questions, for claim-level evidence. Without
alphaXiv tools, run them with `orx alphaxiv call <tool> '<json>'`.

1. Keyword terms use only wording from the user or prior tool results; never
   guess an acronym expansion, and add no general-purpose padding. Semantic or
   scholarly-graph retrieval takes one short faithful question in the user's
   terms rather than a padded reformulation.
2. Rate retrieval difficulty 1–10. Follow-up budget: 1–3 gets 0 complete
   rounds, 4–7 gets 1, 8–10 gets 2.
3. Resolve one publication window and priority. Every initial and follow-up
   call inherits those exact controls; never widen the window or change priority
   during the loop. Every candidate already satisfies the window, so rank what
   is available instead of lamenting well-known work the user excluded.
4. Initial round: alphaXiv keyword, embedding or both for arXiv-heavy ML, CS,
   math or physics (exact full-text evidence, semantic coverage, or both);
   OpenAlex for journal, conference, citation or cross-disciplinary coverage;
   bioRxiv for biology and life-science preprints; PubMed for biomedical,
   clinical and life-science journals. Never ritual calls for unrelated topics;
   query several sources concurrently when the corpus is genuinely ambiguous or
   interdisciplinary. If an alphaXiv keyword query mixes other terms with
   acronym tokens (2–10 characters, starting with a letter, only letters,
   digits or hyphens, at least two uppercase), concurrently run one more keyword
   call of exactly those tokens joined by spaces; it belongs to the initial round.
5. Deduplicate every candidate: exact `id`, then a visible DOI or arXiv id, then
   exact normalized title across sources. Prefer the alphaXiv copy of an arXiv
   duplicate; it supports full-text reading.
6. With solid topical coverage, stop immediately and rank 5–15 IDs. Fast and
   slightly less complete beats an exploratory search; prefer fewer strong
   papers over padding. Otherwise spend follow-up rounds up to the budget.
7. Never invent or recall an ID; rank only IDs a successful call returned.

Read when:
- `references/sources.md`: you need what a source searches or returns, PubMed
  printed `PubMed phrases not found`, or a call says a source is disabled.
- `references/ranking.md`: the question bounds dates or asks for new, seminal
  or popular work, and when ordering candidates within or across sources.
- `references/follow-ups.md`: coverage is thin, a call failed, no selected ID
  survives, or the turn needs a second retrieval loop.

## Citation contract

- Read a paper before using it as claim-level support. Discovery lists may link
  candidate titles, but must not imply that methods or findings were verified
  from snippets alone.
- Ground claims in original evidence and distinguish your interpretation. Quote
  only original text you actually read; cite immediately after a quote. Never
  fabricate evidence or present a paraphrase as a quotation.
- The app lists every paper your answer links below the answer, so end without
  a sources or references list.

Read when:
- `references/citations.md`: before writing quotes or citation links, for
  quotation rules, link destinations and labels.
