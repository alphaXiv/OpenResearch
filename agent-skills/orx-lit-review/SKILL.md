---
name: orx-lit-review
description: "Search and read research papers for grounded results and explanations. Use for literature reviews, related work, research claims, and scientific or technical explanations, even without a named paper. The main agent searches relevant alphaXiv, OpenAlex, and bioRxiv connectors and selects sources for follow-ups. Scale retrieval to the question; conceptual explanations need no exhaustive review."
---

# Literature retrieval

## Main-agent retrieval loop

Use this workflow for scientific explanations and comparisons, even without a
named paper. Never delegate retrieval to a sub-agent.

1. Retrieve through relevant literature connectors and read selected originals
   with `orx paper`. Remembered titles and IDs are leads to verify.
2. Select original visual evidence for the points being explained. For comparisons,
   inspect sources for the alternatives rather than illustrating only one.
   Download PDFs, inspect figures, and save useful crops; extracted text or a
   page-open call alone does not complete this step.
3. Answer as a guided reading of those visuals. Start with a short takeaway,
   then the figure component, not a generated comparison table or overview list.
   Follow each visual with a compact reading guide:
   which arrows, panels, axes, or rows matter, what they mean, and the caveat.
   Let the figures carry the explanation. Omit standalone background tutorials,
   repeated summaries, and equations unless the question needs them. Ground
   author reasoning in contextual direct quotations.

**Default to showing, not describing.** Supporting prose should be brief and
refer to what the reader can see. If a paragraph could be replaced by an
original diagram, graph, or table, retrieve and show that instead. Text is the
fallback only when the relevant sources contain no useful visual or extraction
remains blocked; explain that gap. Do not turn one unavailable figure into a
text-only answer when other relevant visuals are accessible.

Scale retrieval to the question; no fixed number of papers or images is required.
Discovery-only requests can stop at a ranked list. Before sending, remove prose
and generated tables that repeat the visuals or supply a second explanation.

## Retrieve and read

Use enabled connectors appropriate to the topic: alphaXiv for arXiv, OpenAlex
for broader scholarly coverage, bioRxiv for biology. Do not exhaust unrelated
connectors. General web search is a fallback only when relevant connectors
provide no useful evidence. Do not supplement successful retrieval with a web
search for a familiar or preferred paper. Use a focused connector query or
`orx paper` for a missing source. Opening a selected paper's original PDF to
extract evidence is source access; opening its abstract first is unnecessary.
Respect disabled sources and retain successful results when another call fails.

```sh
orx discover keyword "<exact terms>"
orx discover embedding "<question>"
orx discover openalex "<scholarly query>"
orx discover biorxiv "<biology query>"
orx paper <id>
orx paper <id> --full
```

- `keyword` searches alphaXiv titles, abstracts, and full text with match snippets.
  Use short terms from the user or observed results; do not invent acronym
  expansions. If a mixed keyword query includes an acronym, also try the acronym alone.
- `embedding` searches alphaXiv semantically; use a concise faithful question.
- `openalex` searches across disciplines; `biorxiv` searches its bioRxiv index.
- Optional controls: `--published-after YYYY-MM-DD`, `--published-before YYYY-MM-DD`,
  `--prioritize default|recency|historical|popular`, and `--limit N`.
  Preserve requested date bounds throughout retrieval. Use historical priority
  for foundational work, recency for latest work, popular only when requested.
  Do not invent cutoffs. Narrow historical embedding searches can return few
  results; that is not proof no literature exists. `--limit` cannot expand
  alphaXiv's fixed candidate pool.
- Query independent relevant sources concurrently. Rank by topical fit, deduplicate
  by observed ID/DOI/title, and prefer alphaXiv duplicates for full-text access.
  Do not equate alphaXiv votes with OpenAlex citation counts or rerank popularity twice.
- Stop when evidence covers the question. Allow up to two focused follow-up rounds
  for concrete gaps, not repeated reformulations. Return only verified candidate
  IDs; never fabricate them. For explanation/comparison, read the sources needed
  to support the answer rather than forcing a fixed number of papers.
- `orx paper` accepts arXiv IDs/URLs, DOIs, and OpenAlex IDs. By default it returns
  a report, falling back to extracted text when no report exists. `--full` skips
  the report and requests original text; use it for exact wording and context.
  If text is unavailable, locate the original PDF through the returned source.
  An associated GitHub link is not necessarily the paper's own implementation.

## Original visuals

Crop directly from the verified original PDF, using alphaXiv's linked PDF for
alphaXiv papers. Preserve panel titles, axes, legends, and table headings;
exclude the printed caption and surrounding prose. Render legibly and inspect
the crop. Do not substitute thumbnails, redraw results, or generate lookalikes.
If extraction fails, inspect the error and try available PDF tooling; report
an unresolved obstacle rather than silently omitting the figure.

Save crops durably in the session working tree. Use the figure component with
brief accessible alt text and a contextual caption in the Markdown title:

```markdown
![Architecture overview](paper/figure1.png "Figure 1. What this shows and why it matters. [p. 6](https://www.alphaxiv.org/pdf/PAPER_ID?page=6)")
```

Replace example values with verified paths, IDs, and pages. Base your caption
on the original, tailor it to the question, and preserve important qualifications.
It is your explanation, not an author quote. Do not bake it into the image or
repeat it below the component.

Embed each underlying file once per conversation. Later references use
`[Figure 1](paper/figure1.png)` or `[Table 1](paper/table1.png)` to open the same
local file in the right pane. Different crops/edits may be embedded; renaming an
unchanged image does not make it new. Keep paper-provenance links separate.

## Quotes and citations

Ground claims in original evidence and distinguish your interpretation.
Use direct quotations to ground authors' reasoning, methods, assumptions, and
limitations instead of paraphrasing them all. Choose complete sentences or
self-contained passages. Read surrounding context; isolated
numbers and clipped phrases are not sufficient. Do not quote values already
clear in a displayed visual. Cite immediately after a quote.

Quote only original text you actually read, including full-text snippets with
sufficient context—not generated reports or summaries. Preserve wording and
qualifications; mark omissions and never join separate snippets into a continuous
quote. Respect quotation limits by selecting fewer complete passages, not by
clipping context. If exact evidence is unavailable, say so; never fabricate it
or present a paraphrase as a quotation.

Prefer `https://www.alphaxiv.org/pdf/<paper-id>?page=N` when alphaXiv contains
the cited version and evidence. N is the verified one-based PDF page index;
omit it when unknown. Do not substitute different preprint results for a journal
version. Use another verified paper viewer when alphaXiv lacks that evidence
or is disabled. Raw PDFs are for extraction, not user-facing citations.
Do not cite abstract pages or invent exact-passage highlighting URL parameters.

| Reference | Label | Destination |
| --- | --- | --- |
| One paper, page known | `p. N` | Paper viewer at that PDF page |
| Multiple papers | `Short title, p. N` | Corresponding paper/page |
| Page unknown | `Paper` or consistent short title | Paper viewer without page |
| Embedded visual | `Figure N` / `Table N` | Existing local image file |

Use these labels consistently, not vague labels such as "Source" or descriptions
of the claim. Disambiguate figures by paper when needed. Introductory paper-title links can
retain their titles. Discovery and figure-provenance links provide navigation;
they do not imply every claim in a paper has been verified.
