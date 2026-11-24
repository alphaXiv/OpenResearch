# Date and ranking controls

Retrieval is not date-bounded unless you supply a bound. Add the same controls
to any primitive when the question calls for them:

```sh
orx discover keyword "<query>" --published-after 2024-01-01 --prioritize recency
orx discover embedding "<query>" --published-before 2012-01-01 --prioritize historical
orx discover openalex "<query>" --published-after 2024-01-01 --prioritize recency
orx discover biorxiv "<query>" --limit 20
orx discover pubmed "<query>" --published-after 2020-01-01 --prioritize recency
```

- `--published-after` and `--published-before` are inclusive `YYYY-MM-DD`
  bounds. Do not invent a cutoff merely to favour newer work.
- Older or narrow `--published-before` embedding searches can return a thin or
  empty candidate set because the upper bound is applied after vector retrieval.
  Report what comes back; do not treat an empty set as proof that no literature
  exists or retry the identical query and window.
- `--prioritize` is `default`, `recency`, `historical`, or `popular`. Use
  `recency` for explicitly new/latest work, `historical` for seminal or
  foundational work, and `popular` only when the user asks about votes,
  popularity, or community standing.
- `--limit` can narrow alphaXiv output but cannot widen alphaXiv's fixed
  server-side candidate pools. For OpenAlex, bioRxiv, and PubMed it also
  controls the requested pool size.
- OpenAlex and bioRxiv implement these controls with OpenAlex publication-date
  filters, then rerank the returned relevance pool by date or citations. Their
  ranking is best-effort and is not identical to alphaXiv's semantic,
  vote-aware ranking.
- PubMed applies the window as a publication-date filter and reranks its
  relevance pool by date for `recency` and `historical`. It has no citation
  counts, so `popular` keeps PubMed's relevance order.

## Ranking what comes back

Within each source, the API order already blends topical relevance with the
requested priority:

- With `recency`, freshness is already upranked and old accumulated votes are
  damped. Reorder only for topical fit; do not exclude an older but much better
  match.
- With `popular`, votes or citations dominate among topically plausible
  results. Keep high-impact relevant papers, but drop off-topic ones.
- Otherwise, topical relevance remains primary with freshness and votes already
  nudging the order. Do not apply those preferences a second time.

Do not compare alphaXiv votes numerically with OpenAlex citations; they measure
different things. Topical fit is the cross-source ranking signal.
