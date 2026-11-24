# Discovery sources

Each command performs one search against public endpoints and emits its
structured JSON result. No login is required.

- `keyword` searches alphaXiv title, abstract, and full text. Results include
  the match snippets that explain why each paper was retrieved. Use short exact
  terms: method names, acronyms, benchmarks, authors, or title phrases.
- `embedding` searches alphaXiv titles and abstracts semantically, then reranks
  by similarity and the requested priority. Use the user's actual question or a
  concise description of a genuinely missing facet.
- `openalex` searches the cross-disciplinary OpenAlex scholarly graph. It is
  especially useful for journal/conference papers, citation context, and work
  outside arXiv.
- `biorxiv` searches OpenAlex's bioRxiv source index. bioRxiv has no comparable
  native search API; the bioRxiv API is used later when reading a selected DOI.
- `pubmed` searches PubMed's biomedical and life-science journal literature
  through NCBI E-utilities, ranked by PubMed's Best Match. It accepts PubMed
  query syntax such as field tags (`[ti]`, `[au]`, `[mh]`). Result ids are
  PMIDs. Terms PubMed did not find or ignored are printed to stderr
  (`PubMed phrases not found: …`) while stdout stays the JSON results; fix the
  query when a term you relied on is listed.

Every primitive returns the same JSON shape: `source`, self-routing `id`,
title, abstract, and publication date. alphaXiv results may include votes and
full-text snippets; OpenAlex and bioRxiv results may include citations.
PubMed results carry no votes or citations.

bioRxiv is a subset of OpenAlex, and PubMed journal records largely overlap
OpenAlex, so overlap between those calls is expected.

All discovery and paper commands honor the user's disabled literature-source
settings; do not work around an error saying a source is disabled.
