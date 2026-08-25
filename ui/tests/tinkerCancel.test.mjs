import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = (path) => readFile(new URL(path, import.meta.url), "utf8");

test("both Stop surfaces use the shared Tinker cancellation dialog", async () => {
  const [table, detail] = await Promise.all([
    source("../src/components/ExperimentsTable.tsx"),
    source("../src/components/DetailDrawer.tsx"),
  ]);
  for (const component of [table, detail]) {
    assert.match(component, /isTinkerRun/);
    assert.match(component, /<TinkerCancelDialog/);
  }
  assert.match(detail, /selectedRun\?\.status === "cancelled"/);
  assert.match(detail, />Open Tinker</);
});

test("Tinker dialog keeps cancellation errors visible and implements accessible focus handling", async () => {
  const dialog = await source("../src/components/TinkerCancelDialog.tsx");
  assert.match(dialog, /role="dialog"/);
  assert.match(dialog, /aria-modal="true"/);
  assert.match(dialog, /event\.key !== "Tab"/);
  assert.match(dialog, /previousFocus\?\.focus\(\)/);
  assert.match(dialog, /role="alert"/);
  assert.match(dialog, />Keep running</);
  assert.match(dialog, /Stop controller only/);
  assert.match(dialog, /Stop controller & open Tinker/);
  assert.ok(dialog.indexOf("onCancel(run.id)") < dialog.indexOf("window.open("));
});

test("Tinker is always visible in compute and environment settings", async () => {
  const [settings, logos] = await Promise.all([
    source("../src/components/SettingsPage.tsx"),
    source("../src/components/BackendLogos.tsx"),
  ]);
  assert.match(settings, /tinker: "Tinker"/);
  assert.match(settings, /target\.id === "tinker"/);
  assert.match(settings, /"TINKER_API_KEY", "HF_TOKEN", "WANDB_API_KEY"/);
  assert.match(logos, /case "tinker_job":\s+return <TinkerLogo/);
});
