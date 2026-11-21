import assert from "node:assert/strict";
import { test } from "node:test";
import { act, createElement } from "react";
import { ComposerProjectPicker } from "../src/components/ComposerProjectPicker.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, press, render, serve, settle, type } from "./render.mjs";

const project = (id, name, extra = {}) => ({ id, name, updatedAt: 1, ...extra });
const sweeps = project("p1", "Tokenizer sweeps", { scratch: true });
const nanochat = project("p2", "nanochat (demo)", { updatedAt: 2, icon: { glyph: "flask-conical" } });
const blank = project("p3", "No project", { scratch: true, unnamed: true });

async function pick(overrides = {}) {
  serve(() => ({}));
  const calls = { selected: [], settings: [], created: 0, imported: 0 };
  const props = {
    projects: [sweeps, nanochat, blank],
    activity: [],
    projectId: nanochat.id,
    projectName: nanochat.name,
    onSelect: (id) => calls.selected.push(id),
    onNewProject: () => { calls.created++; },
    onImport: () => { calls.imported++; },
    onOpenSettings: (opened) => calls.settings.push(opened.id),
    ...overrides,
  };
  await render(createElement(ComposerProjectPicker, props));
  const trigger = () => document.querySelector('button[aria-haspopup="dialog"]');
  const open = async () => {
    await act(async () => trigger().click());
    await settle();
  };
  const popup = () => document.querySelector(`[role="dialog"][aria-label="${m.projects_home_projects()}"]`);
  const rows = () => [...popup().querySelectorAll("[data-nav]")].map((row) => row.textContent);
  return { calls, trigger, open, popup, rows, search: () => popup().querySelector("input") };
}

test("the picker lists projects with their own icon and searches them by name", async () => {
  const picker = await pick();
  await picker.open();
  const [nanochatRow, sweepsRow] = picker.popup().querySelectorAll("[data-nav]");
  assert.ok(nanochatRow.querySelector(".lucide-flask-conical"));
  assert.ok(sweepsRow.querySelector(".lucide-folder"));
  assert.deepEqual(picker.rows(), ["nanochat (demo)", "Tokenizer sweeps", m.projects_home_new_project()]);
  assert.equal(document.activeElement, picker.search());

  await type(picker.search(), "TOKEN");
  assert.deepEqual(picker.rows(), ["Tokenizer sweeps", m.projects_home_new_project()]);

  await type(picker.search(), "zzz");
  assert.deepEqual(picker.rows(), [m.projects_home_new_project()]);
  assert.ok(find("[role=status]", m.project_picker_no_matches()));
});

test("Enter in the search opens the first match", async () => {
  const picker = await pick();
  await picker.open();
  await type(picker.search(), "token");
  await press("Enter", picker.search());
  assert.deepEqual(picker.calls.selected, ["p1"]);
  assert.equal(picker.popup(), null);
});

test("each project has its own settings button", async () => {
  const picker = await pick();
  await picker.open();
  const button = document.querySelector(`button[aria-label="${m.sidebar_project_settings()} · Tokenizer sweeps"]`);
  await act(async () => button.click());
  assert.deepEqual(picker.calls.settings, ["p1"]);
  assert.deepEqual(picker.calls.selected, []);
  assert.equal(picker.popup(), null);
});

test("an unnamed project is the New project row; the others can start another", async () => {
  const picker = await pick();
  await picker.open();
  await act(async () => find("[data-nav]", m.projects_home_new_project()).click());
  assert.equal(picker.calls.created, 1);
});

test("a new project is named in a field of its own beside Import, and any other project is not", async () => {
  const fresh = await pick({ projectId: blank.id, projectName: "No project", draftProject: blank });
  assert.equal(fresh.trigger().textContent, m.projects_home_new_project());
  assert.ok(document.querySelector(`input[aria-label="${m.new_project_name_placeholder()}"]`));
  assert.ok(find("button", m.new_project_import()));
});

test("an empty named project is an ordinary project in the picker", async () => {
  await pick({ projectId: sweeps.id, projectName: sweeps.name });
  assert.equal(document.querySelector('button[aria-haspopup="dialog"]').textContent, sweeps.name);
  assert.equal(document.querySelector(`input[aria-label="${m.new_project_name_placeholder()}"]`), null);
  assert.equal(find("button", m.new_project_import()), null);
});
