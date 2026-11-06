import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { ArtifactMarkdown } from "../src/components/ArtifactsTab.tsx";
import { ChatImageScope, Md } from "../src/components/Md.tsx";
import { render } from "./render.mjs";

const markdown = [
  "![Remote](https://example.com/a.png)",
  "![Protocol relative](//example.com/b.png)",
  "![](HTTPS://example.com/no-alt.png)",
  "[![Badge](https://example.com/badge.svg)](https://example.com/ci)",
  "![Local](plot.png)",
].join("\n\n");

const surfaces = {
  chat: createElement(ChatImageScope, { projectId: "p" }, createElement(Md, { text: markdown })),
  artifact: createElement(ArtifactMarkdown, { projectId: "p", folder: "", markdown, entries: [] }),
};

for (const [name, element] of Object.entries(surfaces)) {
  test(`${name} markdown links remote images instead of loading them`, async () => {
    const { container } = await render(element);

    const images = [...container.querySelectorAll("img")];
    assert.deepEqual(images.map((image) => image.getAttribute("alt")), ["Local"]);
    assert.ok(!images[0].getAttribute("src").includes("example.com"));

    const links = [...container.querySelectorAll("a[href*='example.com']")];
    assert.deepEqual(links.map((link) => [link.getAttribute("href"), link.textContent]).slice(0, 3), [
      ["https://example.com/a.png", "Remote"],
      ["//example.com/b.png", "Protocol relative"],
      ["HTTPS://example.com/no-alt.png", "HTTPS://example.com/no-alt.png"],
    ]);
    assert.equal(container.querySelector("a a"), null);
  });
}
