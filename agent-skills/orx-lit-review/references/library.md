# The user's library and researchers

For "my papers", "my library", "my notes", highlights or folders, use the
alphaXiv tools `list_library`, `list_annotated_papers` and `get_paper_notes`,
under whatever server prefix this agent gives them. Library tools need alphaXiv
sign-in on the computer running this chat. Without alphaXiv tools, run them
with `orx alphaxiv call <tool> '<json>'`. Save papers or create folders only
when requested, with `save_papers_to_folder` and `create_folder`.

For researcher questions, use `find_researchers`, `resolve_researchers`,
`get_researcher` and `get_researcher_papers`; these work signed out too.

General paper retrieval still follows the retrieval loop in the skill.
