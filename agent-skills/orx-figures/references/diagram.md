# Method and architecture diagrams (TikZ)

**Answers:** what the method *is* — the pipeline, the architecture, where the
contribution sits in it.

Draw schematics in TikZ. It is vector, its text is set by the same engine as
the paper, it diffs in git, and a reviewer's "move the loss term" is a one-line
edit rather than a redraw. Keep the division clean: **TikZ for schematics,
matplotlib for data.** A bar chart in pgfplots is a second plotting stack to
maintain for no gain.

Whatever draws it, the diagram must be genuinely vector (a PDF, not a PNG of a
canvas) and built at the width it will print at — the same two rules every
figure here obeys. Within that, TikZ is a recommendation rather than the
prevailing habit: most ML papers ship diagrams exported from a drawing tool.
Choose the drawing tool when the diagram is heavily illustrative; choose TikZ
when it is boxes, arrows, and symbols, which is most method figures.

## Build it from the shared scaffold

`assets/orx-tikz-preamble.tex` defines the block, data, arrow, and stage styles
and the same palette the plots use. The path is relative to this module's
`SKILL.md`; `orx skill figures/assets/orx-tikz-preamble.tex` prints it from
anywhere. Copy it as the head of the figure's `.tex`:

```sh
mkdir -p figs && orx skill figures/assets/orx-tikz-preamble.tex > figs/method.tex
```

A `.tex` in the working tree compiles where it sits, so `figs/method.tex`
produces `figs/method.pdf` next to it, ready for `\includegraphics`. See the
`orx-paper` module for how compilation and engine selection work.

## Position relatively, always

Use the `positioning` library — `right=of encoder`, `below=4mm of loss` — and
never absolute coordinates. A diagram built from absolute coordinates has to be
rebuilt every time a box gets a longer name; one built from relative positions
reflows. This is the difference between a diagram you will update after review
and one you will not.

## One accent, one direction

- **The contribution is the only accented element.** Everything else is the
  neutral `block` or `muted` style. A diagram where five things are colored
  tells the reader nothing about which one is new.
- **One flow direction**, left-to-right or top-to-bottom. Arrows that double
  back should look different on purpose (`backflow` — gradients, stop-gradient,
  teacher signals), not because the layout ran out of room.
- **Shape carries type**: rectangles are computations, trapezoids (`data`) are
  tensors and datasets, dashed boxes (`stage`) group phases.

## Sizing, and the `scale` trap

`\begin{tikzpicture}[scale=0.8]` scales the *coordinates* and not the node
text, so the boxes close in on labels that stay the same size until they
collide. `transform shape` scales text too — and then the text is smaller than
the caption, which is the thing you were told not to do.

Build the diagram at the width it will be printed. If it comes out too wide,
shorten labels, stack the layout, or drop a stage. Do not scale it down, and do
not lean on `width=\linewidth` to shrink a diagram drawn wider — that is the
rescue-by-scaling this whole rule exists to prevent. Check the compiled PDF's
actual width before including it.

## Label the edges

Tensor shapes, losses, and dimensionalities on the arrows (`edgelbl`) are the
cheapest readability win available: a reader who can follow the shapes can
reconstruct the method without the prose. `$B\times T\times d$` on an edge
replaces a sentence in Section 3.

## Traps

| Trap | Fix |
| --- | --- |
| `scale=0.7` to make it fit | Rebuild at the target width; shorten labels |
| Absolute `\node at (3.4, -1.2)` | `positioning`: `right=of prev` |
| A title node inside the picture | The caption is the title; delete it |
| Every box a different color | One accent for the contribution, neutral for the rest |
| `\tikzexternalize`, `pgfplots` externalization, `minted` | Unavailable — the document is compiled with `-no-shell-escape` |
| Arrows that cross with no reason | Reorder nodes; a crossing should mean something |
| Diagram fonts smaller than the caption | `\footnotesize` for nodes, `\scriptsize` for edge labels, nothing below |

## Template

`figs/method.tex` — the shared scaffold followed by the picture:

```latex
% ... contents of orx-tikz-preamble.tex above this line ...

\begin{document}
\begin{tikzpicture}[orx]

  \node[data]  (x)        {Batch\\$B\times T$};
  \node[block, right=of x]        (enc)  {Encoder\\$L$ layers};
  \node[ours,  right=of enc]      (mod)  {Adaptive\\router};
  \node[block, right=of mod]      (dec)  {Decoder};
  \node[data,  right=of dec]      (yhat) {Logits\\$B\times T\times V$};

  \draw[flow] (x)   -- node[edgelbl, above] {$B\times T$} (enc);
  \draw[flow] (enc) -- node[edgelbl, above] {$B\times T\times d$} (mod);
  \draw[flow] (mod) -- (dec);
  \draw[flow] (dec) -- (yhat);

  \node[muted, below=6mm of dec] (loss) {Cross-entropy};
  \draw[flow]     (yhat.south) |- (loss.east);
  \draw[backflow] (loss.west)  -| node[edgelbl, pos=0.25, below] {$\nabla$} (mod.south);

  \begin{scope}[on background layer]
    \node[stage, fit=(enc)(mod)(dec)] (trunk) {};
  \end{scope}
  \node[stagelbl, above=1mm of trunk] {trained end-to-end};

\end{tikzpicture}
\end{document}
```

Build it at the target width and include it with `width=\linewidth`, exactly
as every other figure here:

```latex
\begin{figure}[t]
  \centering
  \includegraphics[width=\linewidth]{figs/method.pdf}
  \caption{The router replaces the fixed gate in the decoder trunk; gradients
  reach it through the standard cross-entropy path.}
  \label{fig:method}
\end{figure}
```

To put the picture inline instead, move `\usepackage{tikz}`, the
`\usetikzlibrary{...}` line, and the `\definecolor` and `\tikzset` blocks into
the paper's preamble, then `\input` the `tikzpicture` body. Miss the libraries
and the paper stops compiling: `trapezium`, `Stealth`, `fit=`, and
`on background layer` each come from one of them. Do **not** carry
`\familydefault` across — the sans face rides in the `orx`, `edgelbl`, and
`stagelbl` styles, so it travels with the picture and touches nothing else.

## Checklist

- [ ] Every node placed with `positioning`, no absolute coordinates.
- [ ] Exactly one accented element, and it is the contribution.
- [ ] One flow direction; non-forward edges styled as such.
- [ ] Edges labelled with shapes or quantities where it helps.
- [ ] No `scale=`; the compiled PDF is already the target width.
- [ ] No title node; the caption carries the explanation.
- [ ] It compiles, and you have looked at the PDF.
