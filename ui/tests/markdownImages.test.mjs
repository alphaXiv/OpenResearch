import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { ArtifactMarkdown } from "../src/components/ArtifactsTab.tsx";
import { ExperimentOverview } from "../src/components/ExperimentOverview.tsx";
import { ChatImageScope, Md } from "../src/components/Md.tsx";
import { render, serveApp, settle } from "./render.mjs";

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

const experiment = {
  id: "e", projectId: "p", slug: "exp", branchName: "exp", description: "![plot](artifacts/example.svg)",
  runCommand: "python train.py", agentStatus: "idle", createdAt: 0, updatedAt: 0, chatSessionId: "s", archived: false,
};
const run = {
  id: "r", experimentId: "e", projectId: "p", status: "done", resultMarkdown: "![loss](figures/loss.png)",
  createdAt: 0, updatedAt: 0, endedAt: 0, contact: null, cancelling: false, lostReason: null, host: null,
};

async function experimentImages(sessions) {
  serveApp({ "GET /api/chat/sessions": () => sessions && { sessions } });
  const noop = () => {};
  const { container } = await render(createElement(ExperimentOverview, {
    experiment, lineageNode: null, parentExperiment: null, project: { id: "p", githubEnabled: false }, runs: [run],
    onOpenExperiment: noop, onOpenLogs: noop, onOpenTrackio: noop, onOpenCode: noop, onOpenDiff: noop,
  }));
  await settle();
  return Object.fromEntries([...container.querySelectorAll("img")].map((image) => [image.alt, new URL(image.src, location.href)]));
}

test("experiment descriptions and run results load project images from the experiment's chat", async () => {
  const sources = await experimentImages([{ id: "s", projectId: "p", harness: "claude" }]);
  assert.equal(sources.plot.pathname, "/api/projects/p/files/file");
  assert.equal(sources.plot.searchParams.get("path"), "example.svg");
  assert.equal(sources.loss.pathname, "/api/projects/p/file/raw");
  assert.equal(sources.loss.searchParams.get("path"), "figures/loss.png");
  assert.equal(sources.loss.searchParams.get("sessionId"), "s");
});

test("experiment images load from the project once the experiment's chat is deleted", async () => {
  const sources = await experimentImages([]);
  assert.equal(sources.plot.pathname, "/api/projects/p/files/file");
  assert.equal(sources.loss.pathname, "/api/projects/p/file/raw");
  assert.equal(sources.loss.searchParams.get("sessionId"), null);
});

test("experiment images keep the experiment's chat when its chats can't be listed", async () => {
  const sources = await experimentImages(undefined);
  assert.equal(sources.loss.searchParams.get("sessionId"), "s");
});
