# Say so when the document needs a different engine

Papers compile with pdfLaTeX by default. A document that needs XeLaTeX or
LuaLaTeX (`fontspec`, `unicode-math`, a system OpenType font) must say so on
its first line, or it will be built with the wrong engine and fail:

```latex
% !TeX program = lualatex
```

Do not add the line otherwise: pdfLaTeX is the most widely supported engine, and
`microtype`'s letter tracking works only there.
