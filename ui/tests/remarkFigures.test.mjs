import assert from "node:assert/strict";
import test from "node:test";
import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { remarkFigures } from "../src/remarkFigures.ts";

const processor = unified().use(remarkParse).use(remarkFigures).use(remarkRehype);
const render = (text) => processor.runSync(processor.parse(text));

test("a standalone captioned image becomes a figure with a separate linked caption", () => {
  const tree = render('![Performance plot](<figures/plot one.png> "Figure 1. Fewer samples. [Source, p. 6](https://www.alphaxiv.org/abs/1234)")');
  const figure = tree.children[0];
  assert.equal(figure.tagName, "figure");
  const [image, caption] = figure.children;
  assert.equal(image.tagName, "img");
  assert.equal(image.properties.src, "figures/plot%20one.png");
  assert.equal(image.properties.alt, "Performance plot");
  assert.equal(image.properties.title, undefined);
  assert.equal(caption.tagName, "figcaption");
  assert.equal(caption.children[0].value, "Figure 1. Fewer samples. ");
  assert.equal(caption.children[1].tagName, "a");
  assert.equal(caption.children[1].properties.href, "https://www.alphaxiv.org/abs/1234");
});

test("ordinary images and unrelated paragraphs remain ordinary Markdown", () => {
  for (const text of ['![Plot](plot.png)\n\nFigure 1: body text', 'Text ![Plot](plot.png "tooltip")']) {
    const tree = render(text);
    assert.equal(tree.children[0].tagName, "p");
    assert.equal(tree.children.some((node) => node.tagName === "figure"), false);
  }
  assert.equal(render('```md\n![Plot](plot.png "Caption")\n```').children[0].tagName, "pre");
});

test("numbered references open the matching inline image while paper citations stay external", () => {
  const tree = render('[**Table 1**](https://www.alphaxiv.org/abs/1234) and [paper](https://www.alphaxiv.org/abs/1234)\n\n![Results](table1.png "Table 1. Results. [Source](https://www.alphaxiv.org/abs/1234)")');
  const [reference, , paper] = tree.children[0].children;
  assert.equal(reference.properties["data-figure-src"], "table1.png");
  assert.equal(paper.properties["data-figure-src"], undefined);
  const local = render('[Fig. 2](fig2.png)').children[0].children[0];
  assert.equal(local.properties["data-figure-src"], undefined);
  assert.equal(local.properties.href, "fig2.png");
  const ambiguous = render('[Table 1](https://example.com/paper)\n\n![Table 1](a.png)\n\n![Table 1](b.png)');
  assert.equal(ambiguous.children[0].children[0].properties["data-figure-src"], undefined);
});
