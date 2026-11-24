# Use an uploaded LaTeX template

Copy the template's class and style files next to your `.tex` so the compiler
finds them, and start from its entry `.tex` rather than the default preamble:

```sh
cp .orx/latex-templates/<name>/*.cls .orx/latex-templates/<name>/*.sty \
   .orx/latex-templates/<name>/*.bst .
cp .orx/latex-templates/<name>/<entry>.tex paper.tex
```

Then fill in the template's own structure: keep its `\documentclass` line, its
package list, and its section skeleton, and replace only the placeholder
content. A conference class encodes margins, fonts, and an anonymization mode
that the submission is checked against; overriding it defeats the point of
uploading it. If the template needs a package the machine lacks, say so rather
than quietly switching to the default preamble.
