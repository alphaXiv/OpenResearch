import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { NewProjectForm } from "../src/components/NewProjectForm.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, render, serve } from "./render.mjs";

const { setImmediate: realSetImmediate } = globalThis;

test("project creation and debounced prewarm use the current selection, including custom and null OpenCode models", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  /** Runs `ms` of timers, then the requests and renders they start; query notifications also wait on a zero timeout. */
  const advance = async (ms = 0) => {
    await act(async () => t.mock.timers.tick(ms));
    for (let round = 0; round < 10; round++) {
      await act(async () => {
        await new Promise((resolve) => realSetImmediate(resolve));
        t.mock.timers.tick(0);
      });
    }
  };

  const created = [];
  const warmed = [];
  serve((path, init) => {
    if (path === "/api/harnesses") return { harnesses: [{ id: "codex", agentReady: true, models: [{ id: "catalog-default" }] }] };
    if (path.startsWith("/api/project-path/status?")) {
      return { gitVersion: "2.50.0", resolvedPath: "/project", exists: true, directory: true, empty: false, initialized: true, gitState: "ready" };
    }
    if (path === "/api/project-path/pick") return { path: "/project" };
    if (path === "/api/projects/starter-prompts/prewarm") {
      warmed.push(JSON.parse(init.body).starterAgent);
      return { ok: true };
    }
    if (path === "/api/projects" && init.method === "POST") {
      created.push(JSON.parse(init.body).starterAgent);
      return { project: { id: "p", name: "Project" }, githubPublicationError: null };
    }
  });
  const form = (preferredAgent) => createElement(NewProjectForm, { preferredAgent, onCreated: () => {} });

  const view = await render(form({ harness: "codex", model: "saved-model" }));
  await advance();
  await act(async () => find("button", m.new_project_form_existing_folder()).click());
  await act(async () => document.querySelector(`button[aria-label="${m.new_project_choose_existing_folder()}"]`).click());
  await advance(200);

  const custom = { harness: "opencode", model: "my-provider/custom-model", permissionMode: null, reasoningLevel: null };
  await view.rerender(form(custom));
  await advance(1200);
  assert.deepEqual(warmed, [{ harness: "opencode", model: "my-provider/custom-model" }], "changing selection replaces the pending prewarm");

  const other = { harness: "codex", model: "other" };
  await view.rerender(form(other));
  await advance(600);
  await view.rerender(form({ ...other }));
  await advance(600);
  assert.deepEqual(warmed.at(-1), other, "an equivalent selection does not restart the debounce");

  for (const selection of [custom, { harness: "opencode", model: null }, { harness: "claude-code", model: null }, null]) {
    await view.rerender(form(selection));
    await advance(1200);
    await act(async () => find("button", m.new_project_use_folder()).click());
    await advance();
    const agent = selection ? { harness: selection.harness, model: selection.model } : { harness: "codex", model: "catalog-default" };
    assert.deepEqual(created.at(-1), agent);
    assert.deepEqual(warmed.at(-1), agent);
  }
  assert.equal(created.length, 4);
});
