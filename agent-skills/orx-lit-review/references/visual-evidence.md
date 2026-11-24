# Explaining with visual evidence

## Lead with original visuals

- Prefer explaining through original figures, tables, and diagrams. For
  comparisons, choose visuals that cover the relevant alternatives; let the
  reader see the architecture, relationship, or result being explained.
- Answer as a guided reading of those visuals, presenting them early. Keep
  prose brief: what to notice, why it matters, and the caveats, grounded in
  contextual author quotations.

Visual evidence should replace standalone tutorials, generated comparison
tables, and recitations of data or structure. Choose visuals for explanatory
value, without a fixed image count. Text is the fallback when relevant sources
contain no useful visual or extraction remains blocked; explain that gap.
One unavailable figure is not a reason to omit other accessible visuals.

## Crop from the original PDF

Crop directly from the verified original PDF, using alphaXiv's linked PDF for
alphaXiv papers. For a verified arXiv ID, download the source PDF from
`https://arxiv.org/pdf/<id>`. The alphaXiv `/pdf/` URL serves a viewer page,
not the PDF asset; keep user-facing citations pointed at that viewer.
Preserve panel titles, axes, legends, and table headings; exclude the printed
caption and surrounding prose. Render legibly and inspect the crop. Do not
substitute thumbnails, redraw results, or generate lookalikes. If extraction
fails, inspect the error and try available PDF tooling; report an unresolved
obstacle rather than silently omitting the figure.

## Embed the crop

Save crops durably in the session working tree. Use the figure component with
brief accessible alt text and a contextual caption in the Markdown title:

```markdown
![Architecture overview](paper/figure1.png "Figure 1. What this shows and why it matters. [p. 6](https://www.alphaxiv.org/pdf/PAPER_ID?page=6)")
```

Replace example values with verified paths, IDs, and pages. Base your caption
on the original, tailor it to the question, and preserve important
qualifications. It is your explanation, not an author quote. Do not bake it
into the image or repeat it below the component.

Embed each underlying file once per conversation. Later references use
`[Figure 1](paper/figure1.png)` or `[Table 1](paper/table1.png)` to open the
same local file in the right pane. Different crops/edits may be embedded;
renaming an unchanged image does not make it new. Keep paper-provenance links
separate. Disambiguate figures by paper when needed.
