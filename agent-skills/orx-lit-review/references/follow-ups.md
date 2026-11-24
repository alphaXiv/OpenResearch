# Follow-up rounds and loop limits

## When the initial round falls short

Treat initial calls independently: retain every successful result set when
another call fails. If none returns results and follow-up budget remains, use a
round only when a focused recovery query is likely to work.

When coverage is not solid, spend at most the difficulty-derived number of
follow-up rounds. One round targets one concrete missing acronym, method,
benchmark, organization, title phrase, venue, or subtopic. Choose one or more
sources based on the gap: alphaXiv keyword for exact/full-text evidence,
alphaXiv embedding for a semantic arXiv angle, OpenAlex for broad scholarly or
citation coverage, bioRxiv for recent biology preprints, and PubMed for
biomedical or clinical journal literature. Later rounds do **not** need to
query all sources. Calls for the same missing angle count together as one
round. Never spend a round merely rephrasing an existing search. Re-evaluate
after each round and stop as soon as coverage is sufficient. The budget is a
hard cap, not a target.

## When no selected ID survives

Drop each selected ID that did not appear in a successful initial or follow-up
result, retaining the surviving IDs in your chosen rank order. If no selected ID
survives, fall back to the first 15 unique IDs in observation order, with
initial results before follow-up results.

## More than one loop in a turn

Batch all facets into one broad retrieval loop and plan against a cap of two
complete loops per user turn. If a genuinely distinct topic still forces a
third or fourth loop, run it in shallow mode: initial searches only, with zero
follow-up rounds. This degradation is a backstop, not permission to plan extra
loops. Refuse a fifth loop and answer from the papers already found.
