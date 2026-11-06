import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";

import { ArtifactMarkdown } from "../src/components/ArtifactsTab.tsx";
import { render } from "./render.mjs";

async function artifactHref(markdown, folder = "notes", entries = []) {
  const { container } = await render(createElement(ArtifactMarkdown, { projectId: "p", folder, markdown, entries }));
  return container.querySelector(".md a, .md span")?.getAttribute("href") ?? null;
}

const file = (path) => `/api/projects/p/files/file?path=${encodeURIComponent(path)}`;

test("artifact links to non-ASCII and literal-percent filenames are encoded once", async () => {
  assert.equal(await artifactHref("[data](数据.md#top)"), `${file("notes/数据.md")}#top`);
  assert.equal(await artifactHref("[sale](50%off.md?raw=1)"), `${file("notes/50%off.md")}&raw=1`);
  assert.equal(await artifactHref("[mixed](50%off-数据.md)"), file("notes/50%off-数据.md"));
  assert.equal(await artifactHref("[space](a%20b.md)"), file("notes/a b.md"));
  assert.equal(await artifactHref("[bare](50%.md)"), file("notes/50%.md"));
});

test("artifact links keep the version suffix and reject escapes from the root", async () => {
  const entries = [{ path: "notes/数据.md", name: "数据.md", isDir: false, size: 3, modifiedAt: 7 }];
  assert.equal(await artifactHref("[data](数据.md?path=x#top)", "notes", entries), `${file("notes/数据.md")}&v=7:3#top`);
  assert.equal(await artifactHref("[out](../../50%off.md)"), null);
});
