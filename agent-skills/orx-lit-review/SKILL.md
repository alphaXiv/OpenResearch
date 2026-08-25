---
name: orx-lit-review
description: "Search and read research papers. The main agent calls alphaXiv, OpenAlex, and bioRxiv discovery primitives, ranks the combined candidates, and chooses sources for focused follow-ups. Use for literature reviews, related work, prior art, papers, authors, methods, benchmarks, or research claims; never delegate the retrieval loop to a sub-agent."
---

# Literature retrieval

You are the retrieval ranker. Call the alphaXiv, OpenAlex, and bioRxiv
primitives yourself, inspect the returned candidates, and decide which sources
are useful for each focused follow-up. Never delegate this loop to a sub-agent.

Each command performs exactly one public endpoint request and emits its
structured JSON result. No login is required:

```sh
orx discover keyword "<exact keyword query>"
orx discover embedding "<semantic description in the user's terms>"
orx discover openalex "<scholarly search query>"
orx discover biorxiv "<biology preprint query>"
```

- `keyword` searches title, abstract, and full text. Results include the match
  snippets that explain why each paper was retrieved. Use short exact terms:
  method names, acronyms, benchmarks, authors, or title phrases. Use only terms
  stated by the user or observed in results; never invent an acronym expansion.
- `embedding` searches titles and abstracts semantically, then reranks by
  similarity and the requested priority. Use the user's actual question or a
  concise description of a genuinely missing facet.
- `openalex` searches the cross-disciplinary OpenAlex scholarly graph. It is
  especially useful for journal/conference papers, citation context, and work
  outside arXiv.
- `biorxiv` searches OpenAlex's bioRxiv source index. bioRxiv has no comparable
  native search API; the bioRxiv API is used later when reading a selected DOI.
- Every primitive returns the same JSON shape: `source`, self-routing `id`,
  title, abstract, and publication date. alphaXiv results may include votes and
  full-text snippets; OpenAlex and bioRxiv results may include citations.

## Date and ranking controls

Retrieval is not date-bounded unless you supply a bound. Add the same controls
to any primitive when the question calls for them:

```sh
orx discover keyword "<query>" --published-after 2024-01-01 --prioritize recency
orx discover embedding "<query>" --published-before 2012-01-01 --prioritize historical
orx discover openalex "<query>" --published-after 2024-01-01 --prioritize recency
orx discover biorxiv "<query>" --limit 20
```

- `--published-after` and `--published-before` are inclusive `YYYY-MM-DD`
  bounds. Do not invent a cutoff merely to favour newer work.
- Older or narrow `--published-before` embedding searches can return a thin or
  empty candidate set because the upper bound is applied after vector retrieval.
  Report what comes back; do not treat an empty set as proof that no literature
  exists or retry the identical query and window.
- `--prioritize` is `default`, `recency`, `historical`, or `popular`.
- `--limit` can narrow alphaXiv output but cannot widen alphaXiv's fixed
  server-side candidate pools. For OpenAlex and bioRxiv it also controls the
  requested pool size.
- OpenAlex and bioRxiv implement these controls with OpenAlex publication-date
  filters, then rerank the returned relevance pool by date or citations. Their
  ranking is best-effort and is not identical to alphaXiv's semantic,
  vote-aware ranking.
- Use `recency` for explicitly new/latest work. Use `historical` for seminal or
  foundational work. Use `popular` only when the user asks about votes,
  popularity, or community standing.

## Main-agent retrieval loop

You are the low-latency retrieval ranker. Run the loop below yourself.

### Set up the retrieval query

1. If using keyword retrieval, build focused terms using only wording from the
   user or prior tool results. Never guess an acronym expansion. General-purpose
   padding reduces result quality.
2. For semantic or scholarly-graph retrieval, build one short faithful question
   in the user's terms rather than a padded reformulation.
3. Estimate retrieval difficulty from 1–10. This controls a budget of complete
   follow-up rounds: difficulty 1–3 gets 0 rounds, 4–7 gets 1, and 8–10 gets 2.
4. Resolve one publication window and priority for the request. Every initial
   and follow-up call must inherit those exact controls; never widen a window or
   change priority during the loop. Every returned candidate already satisfies
   that window, so rank what is available instead of lamenting well-known work
   that the user excluded.

### Run and rank

1. Choose the initial sources and strategies that fit the query. For arXiv-heavy
   ML, CS, math, or physics questions, use alphaXiv keyword, embedding, or both
   according to whether exact full-text evidence, semantic coverage, or both are
   useful. Add OpenAlex for broader journal, conference, citation, or
   cross-disciplinary coverage. Use bioRxiv for biology and adjacent
   life-science preprints, not as a ritual call for unrelated topics. When the
   corpus is genuinely ambiguous or interdisciplinary, query multiple relevant
   sources concurrently. If the initial round includes alphaXiv keyword and its
   terms mix other terms with one or more 2–10 character tokens
   that start with a letter, contain only letters, digits, or hyphens, and have
   at least two uppercase letters, concurrently run one additional keyword call
   whose query is exactly those acronym tokens joined by spaces and nothing
   else. This recovery call is part of the initial round.
2. Treat initial calls independently: retain every successful result set when
   another call fails. If none returns results and follow-up budget remains,
   use a round only when a focused recovery query is likely to work.
3. Inspect and deduplicate every candidate. Match exact `id` first, then a DOI
   or arXiv id visible in the metadata, then exact normalized title as a
   cross-source fallback. Prefer the alphaXiv representation of an arXiv
   duplicate because it supports full-text reading. bioRxiv is a subset of
   OpenAlex, so overlap between those calls is expected. Within each source,
   the API order already blends topical relevance with the requested priority:
   - With `recency`, freshness is already upranked and old accumulated votes
     are damped. Reorder only for topical fit; do not exclude an older but much
     better match.
   - With `popular`, votes or citations dominate among topically plausible
     results. Keep high-impact relevant papers, but drop off-topic ones.
   - Otherwise, topical relevance remains primary with freshness and votes
     already nudging the order. Do not apply those preferences a second time.
4. If the initial candidates provide solid topical coverage, stop immediately
   and rank 5–15 IDs. Fast and slightly less complete is better than an
   exploratory search. Prefer fewer strong papers over padding.
5. Otherwise, spend at most the difficulty-derived number of follow-up rounds.
   One round targets one concrete missing acronym, method, benchmark,
   organization, title phrase, venue, or subtopic. Choose one or more sources
   based on the gap: alphaXiv keyword for exact/full-text evidence, alphaXiv
   embedding for a semantic arXiv angle, OpenAlex for broad scholarly or
   citation coverage, and bioRxiv for recent biology preprints. Later rounds do
   **not** need to query all sources. Calls for the same missing angle count
   together as one round. Never spend a round merely rephrasing an existing
   search. Re-evaluate after each round and stop as soon as coverage is
   sufficient. The budget is a hard cap, not a target.
6. Drop each selected ID that did not appear in a successful initial or
   follow-up result, retaining the surviving IDs in your chosen rank order. If
   no selected ID survives, fall back to the first 15 unique IDs in observation
   order, with initial results before follow-up results. Never invent or recall
   an ID.

Batch all facets into one broad retrieval loop and plan against a cap of two
complete loops per user turn. If a genuinely distinct topic still forces a
third or fourth loop, run it in shallow mode: initial searches only, with zero
follow-up rounds. This degradation is a backstop, not permission to plan extra
loops. Refuse a fifth loop and answer from the papers already found.

For a set-of-papers request such as “find papers,” “top papers,” “what is out
there,” or “what should I read,” return the ranked discovery results and stop.
Depth on individual papers is not part of the discovery loop. When the request
instead needs claim-level synthesis, methodological details, or comparison,
finish retrieval first and then read the 3–5 most load-bearing candidates with
`orx paper <id>` (or the number the user requested). Do not narrow to 3–5
papers before retrieval has produced its ranked 5–15 candidate set.

Do not compare alphaXiv votes numerically with OpenAlex citations; they measure
different things. Topical fit is the cross-source ranking signal.

In the final answer, link every alphaXiv/arXiv paper title or paper ID to
`https://www.alphaxiv.org/abs/<versionless-paperId>`. Never return an
`arxiv.org` link for those papers. Link a DOI result to `https://doi.org/<doi>`
and a bare OpenAlex `W…` id to `https://openalex.org/<id>`.

For claim-level synthesis, place the supporting source link immediately after
each substantive scholarly claim, and use a paper as claim-level support only
after reading it. A discovery-only result list may link candidate titles, but
must not imply that their methods or findings were verified from snippets alone.

## Reading selected papers

`orx paper` auto-detects an arXiv id/URL, bioRxiv DOI, other DOI, or OpenAlex
`W…` id. For alphaXiv it returns a compact structured report; use `--full` only
when you explicitly need raw text even if a report exists. Without `--full`, a
missing report automatically falls back to extracted full text in the same
command. `--full` skips the report entirely rather than acting as a superset of
the default. If extracted text is also unavailable, use the alphaXiv paper link
it returns.

`orx paper` prints the alphaXiv link before the content. When alphaXiv has an
associated repository, it then prints `GitHub: <url>`. This is the most-starred
associated repository and can be a framework rather than the paper's own code,
so sanity-check it before treating it as the implementation.

All discovery and paper commands honor the user's disabled literature-source
settings; do not work around an error saying a source is disabled.
