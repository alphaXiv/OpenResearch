import { test } from "node:test";
import assert from "node:assert/strict";
import { detectType, load, parse, render, renderAsync } from "pele/lazy";
import { normalizeMermaidNativeTargets } from "../src/mermaidNativeTargets.ts";

const options = { images: true, imageSchemes: ["file"], links: true, linkSchemes: ["http", "https", "mailto", "tel", "file"], idPrefix: "orx-native-test-" };

test("normalizes parsed native links and images without treating drive letters as schemes", async () => {
  const source = 'flowchart LR\n A@{ img: "C:/fixtures/a.png", label: "C:/label" } --> B\n click B href "D:/fixtures/a.md"';
  await load(detectType(source));
  const model = parse(source, options);
  normalizeMermaidNativeTargets(model);
  const { svg } = render(model, options);
  assert.match(svg, /file:\/\/\/C:\/fixtures\/a\.png/);
  assert.match(svg, /file:\/\/\/D:\/fixtures\/a\.md/);
  assert.match(svg, /C:\/label/);
  assert.doesNotMatch(svg, /file:\/\/\/C:\/label/);
});

test("source and normalized model produce identical ordinary diagrams", async () => {
  for (const source of [
    'flowchart LR\n A-->B', 'sequenceDiagram\n Alice->>Bob: Hello', 'classDiagram\n Animal <|-- Dog',
    'stateDiagram-v2\n [*] --> Ready', 'erDiagram\n CUSTOMER ||--o{ ORDER : places',
    'pie\n "A" : 4\n "B" : 6', 'gantt\n dateFormat YYYY-MM-DD\n section A\n Task: 2026-01-01, 2d',
    'gitGraph\n commit', 'mindmap\n root((Research))\n  Method', 'timeline\n 2026 : Launch',
    'journey\n title Research\n section Study\n Read: 5: Alice',
    'quadrantChart\n x-axis Low --> High\n y-axis Low --> High\n Item: [0.5, 0.5]',
    'xychart-beta\n x-axis [A, B]\n y-axis 0 --> 10\n bar [2, 5]',
    'sankey-beta\n\nA,B,5\nB,C,3', 'block-beta\n columns 1\n A["Start"]\n B["End"]\n A --> B',
  ]) {
    const expected = await renderAsync(source, options);
    const model = parse(source, options);
    normalizeMermaidNativeTargets(model);
    assert.equal(render(model, options).svg, expected.svg, source);
  }
});

test("normalization cannot make network images or script links render", async () => {
  for (const source of [
    'flowchart LR\n A@{ img: "https://example.com/x.png", label: "x" }',
    'flowchart LR\n A-->B\n click A href "javascript:alert(1)"',
    'flowchart LR\n A-->B\n click A href "x:unexpected"',
    '%%{init: {"flowchart": {"htmlLabels": true}}}%%\nflowchart LR\n A["<img src=\'https://example.com/x.png\'>"] --> B',
  ]) {
    await load(detectType(source));
    const model = parse(source, options);
    normalizeMermaidNativeTargets(model);
    const { svg } = render(model, options);
    assert.doesNotMatch(svg, /https:\/\/example\.com|javascript:alert|x:unexpected|<foreignObject|<img\b/i);
  }
});

test("normalizes URL-bearing maps and preserves unrelated text even with shared references", () => {
  const node = { url: "C:\\work\\plot.svg#view", text: "C:\\work\\label" };
  const model = { links: new Map([["task", "D:/work/report.md"]]), node, same: node };
  model.self = model;
  normalizeMermaidNativeTargets(model);
  assert.equal(model.links.get("task"), "file:///D:/work/report.md");
  assert.equal(node.url, "file:///C:/work/plot.svg#view");
  assert.equal(node.text, "C:\\work\\label");
});
