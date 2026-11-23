# Reading selected papers

`orx paper <id>` auto-detects an arXiv id/URL, bioRxiv DOI, other DOI, OpenAlex
`W…` id, or PubMed PMID/URL; `orx paper <id> --source <source>` overrides the
guess.

For alphaXiv it returns a compact structured report; use `orx paper <id> --full`
for exact wording and surrounding context, even when a report exists. Without
`--full`, a missing report automatically falls back to extracted full text in
the same command. `--full` skips the report entirely rather than acting as a
superset of the default. If extracted text is also unavailable, locate the
original PDF through the returned paper link.

`orx paper` prints the alphaXiv link before the content. When alphaXiv has an
associated repository, it then prints `GitHub: <url>`. This is the most-starred
associated repository and can be a framework rather than the paper's own code,
so sanity-check it before treating it as the implementation.
