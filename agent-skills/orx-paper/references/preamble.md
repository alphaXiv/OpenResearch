# A preamble that compiles

When there is no template, start from this. It covers everything the skill's
sections use:

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
