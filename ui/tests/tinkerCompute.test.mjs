import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = (path) => readFile(new URL(path, import.meta.url), "utf8");

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
