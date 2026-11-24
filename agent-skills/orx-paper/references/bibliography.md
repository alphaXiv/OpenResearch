# Bibliography

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
to use. Give every entry one; for a paper whose authors you do not know, use a
short title rather than inventing names.

Pick the citation command by how the sentence reads:

- `\citet{kaplan2020}`: the citation is the subject: *Kaplan et al. (2020) show…*
- `\citep{kaplan2020}`: a parenthetical aside: *…is predictable (Kaplan et al., 2020)*
- Plain `\cite` under natbib behaves like `\citet`, so `GRPO~\cite{x}` comes out as
  *GRPO Shao et al. (2024)*, with no parentheses. Write `\citep` for an aside.

Find real references with the `orx-lit-review` workflow: retrieve candidates
with `orx discover`, then read selected sources with `orx paper`. A fabricated
citation is worse than no citation.
