# ScholarCatalyst offline evaluation

This evaluator scores fixed retrieval runs against the released ScholarCatalyst labels. It does not call a live search service. A live `orx` run can help diagnose a system, but it is not an official benchmark result.

## Prepare a task cohort

The evaluator writes gold-free task records. Each record contains an opaque task ID, the query type, the question, and the publication cutoff. This work tracks [issue 530](https://github.com/alphaXiv/OpenResearch/issues/530).

```sh
python3 scripts/scholarcatalyst_eval.py prepare \
  --bench-dir /path/to/scholarcatalyst-data \
  --query-type all \
  --output /tmp/tasks.jsonl
```

Use the same task file for each system in a comparison. The opaque ID maps to the released query ID inside the evaluator. Do not give an evaluated system the source files, relation files, or that mapping. Give the system only the task file and the filtered corpus that its retrieval method may search.

Use `--limit N` to select the first N tasks after the type filter. A positive integer is required. The evaluator checks the needed relation files before it writes tasks. It prints input hashes and task counts to standard output.

## Score an offline run

Each run row uses the official query ID or the opaque task ID. It has a `ranking` list of objects with `doc_id` fields. A row can also include `seen`, a list of corpus IDs inspected during retrieval.

```sh
python3 scripts/scholarcatalyst_eval.py evaluate \
  --bench-dir /path/to/scholarcatalyst-data \
  --run /path/to/run.jsonl \
  --query-type all \
  --query-ids /tmp/tasks.jsonl \
  --output /tmp/report.json
```

The evaluator reports macro Recall@5, Recall@20, Recall@100, and nDCG@20 for CoreQ and SubQ. The report gives `n_requested`, `n_scored`, and `n_no_positive`. Queries with no positive labels do not enter metric averages. Missing or empty runs for queries with positive labels score zero. The report gives missing-row counts and ranking-depth counts so you can identify incomplete runs.

Candidate trajectory recall measures the share of labeled positives present in `seen`. It is available only for scored queries that provide `seen`; `n_seen_scored` shows its coverage. A missing `seen` field does not mean an empty search trace. The evaluator cannot prove that a supplied `seen` list records every document the system inspected.

The evaluator rejects the source paper by its ID and known title aliases in both `ranking` and `seen`. This strict check prevents source leakage during closed-corpus replay. It differs from the reference scorer, which removes the source from a final ranking after retrieval.

The evaluator matches normalized source titles when the title has at least four words. Released query rows often omit `paper_title`. In that case, the evaluator gets the title from the corpus row whose ID matches `paper_id`. When neither title is available, only the exact source ID can be rejected. The report counts these queries in `source_title_unavailable`.

## Read date and label checks

The cutoff follows the reference evaluator's month-level rule. A candidate from a later month is an error. A candidate from the same month passes. A year-only date uses month zero, so it passes other dates in that year. An unknown date passes. The evaluator uses the arXiv ID as a fallback date for a candidate with no publication date. A missing or unknown query cutoff disables date filtering.

The report counts unknown query, ranking, and trajectory dates. It also reports labeled positives that fall after the query cutoff. These positives remain in the official gold denominator for reference compatibility. `attainable_recall_ceiling` shows the share of a query's positives that pass the date rule. The evaluator does not assume which date or label is wrong.

The pinned data revision has 19 labeled positives after their query cutoff. These positives affect 18 queries: 3 CoreQ queries and 15 SubQ queries. The report counts 3 CoreQ positives and 16 SubQ positives. The scorer cannot determine whether a query date, document date, or label needs correction. Source-title aliases are unavailable for 775 of 894 queries in this snapshot. Those queries use exact source-ID exclusion only.

## Compare retrieval methods

First compare raw embedding retrieval with a hybrid union of sparse and dense candidates. Then apply the same reranker to each candidate pool. This separates candidate recall from reranking quality. Compare an existing agent ranking as a separate end-to-end system. Use one task file and one frozen corpus for all runs. Keep tools, model versions, and compute budgets fixed. Record candidate counts and run files with the reports. These are hypotheses to test. No method improvement is established before the runs finish.

## Dataset license and provenance

The dataset has a non-commercial license. Download [ScholarCatalyst from Hugging Face](https://huggingface.co/datasets/ScholarCatalyst/ScholarCatalyst) at pinned revision `a5a73467500ada90db4e7641e0697a9591e41a4e` under its license terms. Do not vendor corpus, query, label, or run files into this repository. The report records SHA-256 hashes for the corpus, queries, selected relation files, and run.

## Scope

This script is a standalone Python 3.9+ tool. It uses only the standard library and does not change production retrieval defaults. Its formulas follow the official evaluator's binary relevance and DCG definitions. The code is an independent implementation and does not copy the reference implementation.
