---
name: orx-diagrams
description: "Draw flowcharts, sequence, state, ER, class, gantt, gitgraph and mindmap diagrams as mermaid, which the app renders from .mmd/.mermaid files and fenced blocks. Covers choosing the type, the quoting and reserved-word rules that break parsing, and when a result belongs in matplotlib instead. Use whenever asked to draw or diagram a process, pipeline, architecture, protocol, state machine or dataflow."
---

A diagram earns its place when the prose has stopped being readable: a pipeline
with six stages, a protocol with four actors, a state machine with a retry
branch. Written out in English those become a wall of "then A does X, unless…".
Drawn, they are one glance.

Mermaid renders here natively. A `.mmd` or `.mermaid` file opens as a rendered
diagram with a Render/Source toggle, and a fenced `mermaid` block inside a
markdown report draws where it sits. There is no export step, no headless
browser, and no image to keep in sync — the diagram is text, so it diffs in
git and a reviewer's "move that box" is a one-line edit.

## Mermaid or a figure

| The thing being drawn | Use |
|---|---|
| Pipeline, architecture, protocol, state machine, ER model, call sequence | **mermaid**, here |
| A method schematic that goes **into a paper** | TikZ — see `orx-figures` |
| Learning curves, bars, heatmaps, anything with **numbers on it** | matplotlib — see `orx-figures` |

The dividing line is data, not taste. Mermaid has no axes, so a diagram of
quantities is a diagram of nothing. And a schematic bound for a PDF is built at
print width in the paper's typeface — which is what `orx-figures` covers, and
what a screen diagram is not.

## Pick the type before writing a line

| Question the reader has | Type | First line |
|---|---|---|
| What happens, in what order? | flowchart | `flowchart TD` or `flowchart LR` |
| Who talks to whom, in what order? | sequence | `sequenceDiagram` |
| What can this be, and what moves it? | state | `stateDiagram-v2` |
| What are these things, and how do they relate? | ER | `erDiagram` |
| What does this class look like? | class | `classDiagram` |
| How is this shaped? | mindmap | `mindmap` |
| Over what time? | gantt | `gantt` |
| What branches and merges? | git | `gitGraph` |
| What is the split of a whole? | pie | `pie` |

`TD` (top-down) reads as a hierarchy and is the safer default. Use `LR` only when
the stages are genuinely sequential left to right — a wide `LR` chain scrolls off
the screen.

## Rules that break parsing

These are the failures, not style preferences. Each one costs a round trip: the
diagram falls back to its source, with mermaid's own parse error printed
underneath it. **Read that error before rewriting anything** — it names the line
and usually the token it choked on, which is faster than re-deriving the rule.

1. **The first line must name the type.** A block of bare edges is not a
   diagram — `A --> B` on its own will not parse. It opens with `flowchart TD`.
2. **Quote any label containing `( ) [ ] { }` or `|`.** These eight characters
   end the label early and turn the rest into a syntax error.
   `A[Train (3 epochs)]` fails; `A["Train (3 epochs)"]` works. Quote edge labels
   for the same reason: `A-->|yes (maybe)| B` fails, `A-->|"yes (maybe)"| B`
   works.
3. **Never use a `"` inside a quoted label.** Quoting handles every other
   character; a literal double quote does not survive it. Use `'` or reword.
4. **`graph`, `end`, `subgraph`, `class`, and `click` are reserved.** As a node
   id they are a parse error, not a warning. Suffix them: `end_`, `class_`,
   `click_`.
5. **Labels are plain text.** HTML is escaped and shown literally, and markdown
   emphasis is not interpreted — `A["**bold**"]` shows the asterisks. Write
   emphasis out as words.

## Shape it

**Dagre lays out by default, which is right for almost everything.** A graph
dense enough that dagre's layered layout tangles — a cyclic dependency map, a
large mesh — can ask for ELK instead, as the first line:

```
%%{init: {"layout": "elk"}}%%
flowchart TD
```

**Keep it under about 25 nodes.** Past that a diagram stops being a diagram:
edges cross, labels overlap, and the reader zooms instead of reads. A diagram
that needs splitting is two diagrams.

**Name the nodes for the reader.** `N1`, `N2` force the reader to hold a lookup
table. `collect`, `train`, `eval` are the diagram.

## Check before you ship

There is no server-side validator, so read it back once:

- First line names a diagram type
- Every label with `( ) [ ] { } |` is quoted, and no label contains `"`
- No node id is `graph`, `end`, `subgraph`, `class`, or `click`
- Every edge references a node that exists somewhere in the block
- A sequence diagram declares its participants when their names carry meaning:
  `participant U as User`, not a bare `User`
- Under ~25 nodes, and the layout is `TD` unless `LR` earns it
- Labels read as prose — no markdown, no HTML, no sentinel ids

## Where to put it

A standalone diagram goes in the working tree with a `.mmd` extension, where it
opens as a rendered file:

```sh
mkdir -p figs && cat > figs/pipeline.mmd <<'EOF'
flowchart LR
  collect[Collect runs] --> train[Train] --> eval[Evaluate]
  eval -->|pass| publish[Publish]
  eval -->|fail| revisit[Revisit]
EOF
```

Inside a report, fence it in place. Artifacts are the durable destination —
anything meant to outlive the session belongs there rather than in the checkout.