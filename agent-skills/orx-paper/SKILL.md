---
name: orx-paper
description: "Draft a paper or preprint in LaTeX. Use for a paper, manuscript, arXiv or submission draft, or a section of one; reports belong to orx-reports."
---

Write the paper as a real `.tex` file in the working tree, never as an outline
in chat. Keep it out of the artifacts directory: a `.tex` is compiled where it
sits in the live checkout, and the PDF is written beside it.

## Create the file on the first request

Create `paper.tex` at the repo root (`<topic>.tex` when several papers coexist),
write real content into it, then link it in chat with the playbook's file tag so the user can open the
rendered document.

## Check for a template before writing a preamble

The user may have uploaded a LaTeX template, such as a conference class or a lab
preprint style. Look before you write anything:

```sh
ls .orx/latex-templates/
```

- **Exactly one template**: use it. Do not ask; it is there to be used.
- **Several**: name them and ask which, unless the request already said.
- **None**: use the default preamble.

To add a reusable template to OpenResearch, load `orx-customize`.

## Structure

`\begin{abstract}`, then `\section`/`\subsection`. Number equations you refer
to (`\begin{equation}\label{eq:loss}`) and cite them with `\eqref{eq:loss}`;
use the starred form for equations you never reference. Label every float and
refer to it as `Table~\ref{tab:main}`, never as "the table below": a float
moves.

Load `orx-figures` before making a figure; a default matplotlib plot is the
wrong size, titled where the caption belongs, and rasterized. It also covers
TikZ method diagrams, which compile in this tree like the paper. Reference only
image files that exist, by path relative to the `.tex`, with the extension
written out (`\includegraphics[width=\linewidth]{figs/loss_curve.pdf}`): a
missing graphic fails the build.

## Results come from runs, not from memory

Every number in a results table must come from an actual run: read it from
the file located by `orx logs` (see `orx-results`). Never write a
placeholder metric that reads as real. If a number is not measured yet, say so
in the text.

## How the file gets compiled

A `.tex` in the working tree is compiled with whatever LaTeX engine the machine
has (tectonic, latexmk, or pdflatex), and `paper.pdf` is written next to the
source. Saving an edit recompiles it. There is no approximate preview: a
document that does not compile leaves the user with nothing. When a build
fails, read the first line of the TeX log starting with `!`, which names the
problem and the source line, fix the source, and let it build again. Never hand
back a document that does not compile.

## Read when

- Using an uploaded template: [references/template.md](references/template.md).
- No template, writing the preamble, or a build fails on an undefined environment: [references/preamble.md](references/preamble.md).
- The document uses `fontspec`, `unicode-math` or a system OpenType font: [references/engines.md](references/engines.md).
- Adding citations or a bibliography: [references/bibliography.md](references/bibliography.md).
- Creating a new standalone paper (its own `\documentclass`) in this request: [references/acknowledgment.md](references/acknowledgment.md).
- The machine has no LaTeX engine, or the paper is linked to Overleaf: [references/overleaf.md](references/overleaf.md).

If a reference beside this skill can't be read, `orx skill paper/<name>` prints
it.
