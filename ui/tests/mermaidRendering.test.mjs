import { test } from "node:test";
import assert from "node:assert/strict";
import { renderAsync } from "pele/lazy";

const options = { images: true, imageSchemes: ["file", ..."abcdefghijklmnopqrstuvwxyz"], links: true, linkSchemes: ["http", "https", "mailto", "tel", "file", ..."abcdefghijklmnopqrstuvwxyz"], idPrefix: "orx-test-" };

test("diagram families render with links available for explicit navigation", async () => {
  for (const source of [
    'flowchart LR\n A[Start] --> B[End]',
    'sequenceDiagram\n Alice->>Bob: Hello',
    'classDiagram\n Animal <|-- Dog',
    'stateDiagram-v2\n [*] --> Ready',
    'erDiagram\n CUSTOMER ||--o{ ORDER : places',
    'pie title Animals\n "Cats" : 4\n "Dogs" : 6',
    'gantt\n title Work\n dateFormat YYYY-MM-DD\n section A\n Task: 2026-01-01, 2d',
    'gitGraph\n commit\n branch feature\n checkout feature\n commit',
    'mindmap\n root((Research))\n  Method\n  Results',
    'timeline\n title History\n 2026 : Launch',
    'journey\n title Research\n section Study\n Read: 5: Alice',
    'quadrantChart\n title Priorities\n x-axis Low --> High\n y-axis Low --> High\n Item: [0.5, 0.5]',
    'xychart-beta\n title "Results"\n x-axis [A, B]\n y-axis "Score" 0 --> 10\n bar [2, 5]',
    'sankey-beta\n\nA,B,5\nB,C,3',
    'block-beta\n columns 1\n A["Start"]\n B["End"]\n A --> B',
    'flowchart LR\n A[link] --> B\n click A href "https://example.com/"',
  ]) {
    const { svg } = await renderAsync(source, options);
    assert.match(svg, /<svg\b/);
    if (!source.includes('click A href')) assert.doesNotMatch(svg, /https?:\/\/example\.com/);
  }
});

test("invalid diagrams reject for source fallback", async () => {
  await assert.rejects(renderAsync("this is not a diagram", options));
});

test("diagram directives cannot restore HTML labels or external images", async () => {
  const source = `%%{init: {"flowchart": {"htmlLabels": true}}}%%
flowchart LR
 A["<img src='https://example.com/image.png'>"] --> B`;
  const { svg } = await renderAsync(source, options);
  assert.doesNotMatch(svg, /<foreignObject|<img\b|<image\b|https:\/\/example\.com/i);
  const imageNode = await renderAsync('flowchart LR\n A@{ img: "https://example.com/x.png", label: "x" }', options);
  assert.doesNotMatch(imageNode.svg, /<image\b|https:\/\/example\.com/i);
  const local = await renderAsync('flowchart LR\n A@{ img: "img/local.png", label: "x" }', options);
  assert.match(local.svg, /<image\b[^>]*href="img\/local\.png"/);
  await assert.rejects(renderAsync('flowchart LR\n A-->B\n style A fill:url(https://example.com/x)', options));
});
