import assert from "node:assert/strict";
import { test } from "node:test";
import { clampRailWidth, panelMaxWidth, railWidthBesidePanel, sidebarFitsWithPanel } from "../src/panelLayout.ts";

test("sidebar fits only when all three columns have their minimum width", () => {
  for (const width of [320, 720, 800, 900, 1039]) assert.equal(sidebarFitsWithPanel(width, 300), false);
  for (const width of [1040, 1280, 1920]) assert.equal(sidebarFitsWithPanel(width, 300), true);
  assert.equal(sidebarFitsWithPanel(1040, 400), false);
  assert.equal(sidebarFitsWithPanel(1140, 400), true);
});

test("panel leaves room for chat with or without the sidebar", () => {
  assert.equal(panelMaxWidth(720, 0), 360);
  assert.equal(panelMaxWidth(800, 0), 420);
  assert.equal(panelMaxWidth(900, 0), 520);
  assert.equal(panelMaxWidth(1040, 300), 360);
  assert.equal(panelMaxWidth(1280, 300), 600);
  assert.equal(panelMaxWidth(1280, 400), 500);
  assert.equal(panelMaxWidth(320, 0), 160);
});

test("sidebar width stays within its bounds", () => {
  assert.equal(clampRailWidth(100), 220);
  assert.equal(clampRailWidth(333.4), 333);
  assert.equal(clampRailWidth(900), 480);
});

test("a sidebar at its width beside the panel still fits", () => {
  for (const width of [1040, 1280, 1920]) assert.equal(sidebarFitsWithPanel(width, railWidthBesidePanel(width)), true);
  assert.equal(sidebarFitsWithPanel(1040, railWidthBesidePanel(1040) + 1), false);
});
