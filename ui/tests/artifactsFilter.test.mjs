import assert from "node:assert/strict";
import { test } from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { find, renderApp, serveApp, type } from "./render.mjs";

const entry = (path, children) => ({
  name: path.split("/").pop(),
  path,
  isDir: children !== undefined,
  size: children ? 0 : 10,
  modifiedAt: 1,
  ...(children ? { children } : {}),
});

const tree = [
  entry("outputs", [entry("outputs/figures", [entry("outputs/figures/loss.png"), entry("outputs/figures/notes.txt")])]),
  entry("report.md"),
];

async function artifactsTab() {
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
    "GET /api/projects/p/files": () => ({ dir: "/tmp/artifacts", entries: tree, truncated: false }),
  });
  const pane = encodeURIComponent(JSON.stringify({ kind: "home", view: "artifacts" }));
  await renderApp(`/projects/p/tasks/new?pane=${pane}`);
}

const names = () => [...document.querySelectorAll(".files-tab .file-tree-name")].map((name) => name.textContent);
const click = (element) => act(async () => element.click());
const chevron = (name) => find(".files-tab .file-tree-row", name).querySelector(".file-tree-chevron");

async function filterBy(query) {
  if (!document.querySelector(".artifacts-filter-input")) await click(find(`button[aria-label="${m.artifacts_filter()}"]`));
  await type(document.querySelector(".artifacts-filter-input"), query);
}

test("a filter shows only matching files, however deep, with their folders", async () => {
  await artifactsTab();
  await filterBy("png");
  assert.deepEqual(names(), ["outputs", "figures", "loss.png"]);
});

test("folders closed while filtering stay open once the filter clears", async () => {
  await artifactsTab();
  await filterBy("png");
  await click(chevron("figures"));
  assert.deepEqual(names(), ["outputs", "figures"]);

  await filterBy("");
  assert.deepEqual(names(), ["outputs", "figures", "loss.png", "notes.txt", "report.md"]);
});
