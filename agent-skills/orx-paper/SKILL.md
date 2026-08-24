---
name: orx-paper
description: "Draft an academic paper or preprint as LaTeX. Create a .tex file in the project working tree, where it renders for the user and compiles to PDF. Use for a paper, preprint, manuscript, arXiv or submission draft, or a section of one; generic reports and result summaries belong to `orx-reports`."
---

Write the paper as a real `.tex` file in the working tree. Do not answer a paper
request with an outline in chat, and keep the `.tex` out of the artifacts
directory: a `.tex` is compiled where it sits in the live checkout, and the PDF
is written beside it.

## Create the file on the first request

Create `paper.tex` at the repo root (`<topic>.tex` when several papers coexist),
write real content into it, then link it in chat using the session playbook's
evidence-and-links contract so the user can open the rendered document. An
outline in chat gives them nothing to render.

## Check for a template before writing a preamble

The user may have uploaded their own LaTeX template — a conference class, a lab
preprint style. Look before you write anything:

```sh
ls .orx/latex-templates/
```

- **Exactly one template** — use it. Do not ask; it is there to be used.
- **Several** — name them and ask which, unless the request already said.
- **None** — use the preamble below.

To use one, copy its class and style files next to your `.tex` so the compiler
finds them, and start from its entry `.tex` rather than the default preamble:

```sh
cp .orx/latex-templates/<name>/*.cls .orx/latex-templates/<name>/*.sty \
   .orx/latex-templates/<name>/*.bst .
cp .orx/latex-templates/<name>/<entry>.tex paper.tex
```

Then fill in the template's own structure — keep its `\documentclass` line, its
package list, and its section skeleton, and replace only the placeholder
content. A conference class encodes margins, fonts, and an anonymization mode
that the submission is checked against; overriding it defeats the point of
uploading it. If the template needs a package the machine lacks, say so rather
than quietly switching to the default preamble.

## A preamble that compiles

When there is no template, start from this. It covers everything the sections
below use:

```latex
\documentclass[11pt]{article}
\usepackage[margin=1in]{geometry}
\usepackage{amsmath,amssymb,amsthm}
\usepackage{graphicx}
\usepackage{booktabs}
\usepackage{listings}
\usepackage{natbib}
\usepackage[hidelinks]{hyperref}
\newtheorem{theorem}{Theorem}
\newtheorem{lemma}[theorem]{Lemma}

\title{...}
\author{...}
\date{\today}

\begin{document}
\maketitle
...
\end{document}
```

**Every environment needs the package that defines it.** This is the most common
way a draft fails to compile, and the failure is fatal, not cosmetic:

| Using | Requires |
| --- | --- |
| `theorem`, `lemma`, `proof` | `amsthm` **and** a `\newtheorem` for each |
| `lstlisting` | `listings` |
| `align`, `equation*`, `\text` | `amsmath` |
| `\toprule`, `\midrule` | `booktabs` |
| `\includegraphics` | `graphicx` |
| `\url`, `\href` | `hyperref` |
| `\citet`, `\citep` | `natbib` (plain `\cite` needs nothing) |

Load only packages you use, and never invent a macro without defining it.

## Say so when the document needs a different engine

Papers compile with pdfLaTeX by default. A document that needs XeLaTeX or
LuaLaTeX — `fontspec`, `unicode-math`, a system OpenType font — must say so on
its first line, or it will be built with the wrong engine and fail:

```latex
% !TeX program = lualatex
```

Do not add the line otherwise: pdfLaTeX is the most widely supported engine, and
`microtype`'s letter tracking works only there.

## Structure

`\begin{abstract}`, then `\section`/`\subsection`. Number equations you refer
to (`\begin{equation}\label{eq:loss}`) and cite them with `\eqref{eq:loss}`;
use the starred form for equations you never reference. Label every float and
refer to it as `Table~\ref{tab:main}`, never as "the table below" — a float
moves.

## Figures and tables

Reference image files that actually exist in the tree, by path relative to the
`.tex`:

```latex
\includegraphics[width=0.8\linewidth]{figs/loss_curve.png}
```

Write the extension out, and confirm the file exists before referencing it — a
missing graphic fails the build.

## Bibliography

Use an inline `thebibliography` block:

```latex
\begin{thebibliography}{9}
\bibitem[Kaplan et al.(2020)]{kaplan2020} Kaplan et al. Scaling laws for
  neural language models. 2020.
\end{thebibliography}
```

It compiles in one pass. A `\bibliography{refs}` with a separate `.bib` needs a
biber round trip and shows as an unresolved reference until then.

The `[Author(Year)]` label is what `\citet` prints; without it natbib has no name
to use. Give every entry one — for a paper whose authors you do not know, use a
short title rather than inventing names.

Pick the citation command by how the sentence reads:

- `\citet{kaplan2020}` — the citation is the subject: *Kaplan et al. (2020) show…*
- `\citep{kaplan2020}` — a parenthetical aside: *…is predictable (Kaplan et al., 2020)*
- Plain `\cite` under natbib behaves like `\citet`, so `GRPO~\cite{x}` comes out as
  *GRPO Shao et al. (2024)*, with no parentheses. Write `\citep` for an aside.

Find real references with the `orx-lit-review` workflow: retrieve candidates
with `orx discover`, then read selected sources with `orx paper`. A fabricated
citation is worse than no citation.

## Results come from runs, not from memory

Every number in a results table must come from an actual run — read it with
`orx logs` (see the `orx-evidence` skill). Never write a placeholder metric that
reads as real. If a number is not measured yet, say so in the text.

## How the file gets compiled

A `.tex` in the working tree is compiled with whatever LaTeX engine the machine
has — tectonic, latexmk, or pdflatex — and the resulting `paper.pdf` is written
next to the source. Saving an edit recompiles it, so the PDF tracks the file.

There is no approximate preview to fall back on: a document that does not
compile has nothing to show. A failed build is therefore not a cosmetic problem
to note and move past — it is the difference between the user having a paper and
having nothing.

When a build fails, the TeX log is the diagnosis. Read the first line starting
with `!` — it names the problem and the source line — fix the source, and let it
build again. Never hand back a document that does not compile. On a machine with
no LaTeX engine, say so plainly instead of pretending the file is finished.
