# Size and include a paper figure

Pick the width from the document (`COLUMN`, `TEXT`, `WIDE` in the style
module) and build at it. A figure drawn 8 inches wide and shrunk to a
3.25-inch column has 3pt tick labels.

Build the figure at the column width, **and** still write `width=\linewidth`:

```latex
\begin{figure}[t]          % figure* for a WIDE figure in a two-column paper
  \centering
  \includegraphics[width=\linewidth]{figs/loss_curve.pdf}
  \caption{Validation loss over training, mean of 5 seeds with 95\% intervals.}
  \label{fig:loss}
\end{figure}
```

The two together are the point. Sized correctly, `width=\linewidth` scales by
1.0 and changes nothing, but it still fits if the venue's column turns out
narrower than you assumed. What it cannot do is rescue a figure built at the
wrong size: a 15-inch canvas dropped into a 5.5-inch column is scaled to 0.35,
and 11pt tick labels land at 4pt. That is the most common reason a real paper's
figures are unreadable, and `width=\linewidth` is what hides it until print.

A `WIDE` figure only gets a 6.75-inch `\linewidth` inside a `figure*`; in a
plain `figure` in a two-column paper it silently scales to about a half.
