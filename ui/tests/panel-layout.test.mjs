import assert from "node:assert/strict";
import { test } from "node:test";
import { panelMaxWidth, sidebarFitsWithPanel } from "../src/panelLayout.ts";

test("sidebar fits only when all three columns have their minimum width", () => {
  for (const width of [320, 720, 800, 900, 1039]) assert.equal(sidebarFitsWithPanel(width), false);
  for (const width of [1040, 1280, 1920]) assert.equal(sidebarFitsWithPanel(width), true);
});

test("panel leaves room for chat with or without the sidebar", () => {
  assert.equal(panelMaxWidth(720, false), 360);
  assert.equal(panelMaxWidth(800, false), 420);
  assert.equal(panelMaxWidth(900, false), 520);
  assert.equal(panelMaxWidth(1040, true), 360);
  assert.equal(panelMaxWidth(1280, true), 600);
  assert.equal(panelMaxWidth(320, false), 160);
});
