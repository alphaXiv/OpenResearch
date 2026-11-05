import assert from "node:assert/strict";
import { test } from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { find, renderApp, serveApp, settle } from "./render.mjs";

/** Too narrow for the sidebar beside an open panel, and wide enough for both. */
const NARROW = 800;
const WIDE = 1600;

const resize = (width) => act(async () => happyDOM.setViewport({ width, height: 900 }));
const rail = () => document.querySelector(".session-rail");
const showSidebar = () => find(`.chat-pane button[aria-label="${m.chat_panel_show_sidebar()}"]`);

/** A project chat at `width`, with its files panel open unless `panel` is false. */
async function chat(width, panel = true) {
  await resize(width);
  serveApp({ "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }) });
  const pane = panel ? `?pane=${encodeURIComponent(JSON.stringify({ kind: "home", view: "files" }))}` : "";
  await renderApp(`/projects/p/tasks/new${pane}`);
}

const focus = (element) => act(async () => element.focus());

test("widening the window moves focus from the vanishing show-sidebar button into the sidebar", async () => {
  await chat(NARROW);
  assert.equal(rail(), null);
  const button = showSidebar();
  await focus(button);
  await resize(WIDE);
  await settle();
  assert.equal(button.isConnected, false);
  assert.ok(rail().contains(document.activeElement));
});

test("collapsing the sidebar focuses the show-sidebar button", async () => {
  await chat(WIDE, false);
  const hide = find(`.session-rail button[aria-label="${m.header_hide_sidebar()}"]`);
  await focus(hide);
  await act(async () => hide.click());
  await settle();
  assert.equal(rail(), null);
  assert.equal(document.activeElement.getAttribute("aria-label"), m.chat_panel_show_sidebar());
});

test("widening leaves focus that is elsewhere", async () => {
  await chat(NARROW);
  const elsewhere = document.querySelector(".right-pane [data-workspace-content]");
  await focus(elsewhere);
  await resize(WIDE);
  await settle();
  assert.ok(rail());
  assert.equal(document.activeElement, elsewhere);
});

test("widening leaves focus on the body", async () => {
  await chat(NARROW);
  await act(async () => document.activeElement.blur());
  await resize(WIDE);
  await settle();
  assert.ok(rail());
  assert.equal(document.activeElement, document.body);
});

test("widening keeps a collapsed sidebar collapsed and its show button focused", async () => {
  await chat(WIDE);
  await act(async () => find(`.session-rail button[aria-label="${m.header_hide_sidebar()}"]`).click());
  await settle();
  await resize(NARROW);
  const collapsed = showSidebar();
  await focus(collapsed);
  await resize(WIDE);
  await settle();
  assert.equal(rail(), null);
  assert.equal(document.activeElement, collapsed);
});

test("focus moved elsewhere during the resize stays there", async () => {
  await chat(NARROW);
  await focus(showSidebar());
  const other = document.querySelector(".right-pane [data-workspace-content]");
  await act(async () => {
    happyDOM.setViewport({ width: WIDE, height: 900 });
    other.focus();
  });
  await settle();
  assert.ok(rail());
  assert.equal(document.activeElement, other);
});
