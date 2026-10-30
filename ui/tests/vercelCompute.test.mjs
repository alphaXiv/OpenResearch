import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = (path) => readFile(new URL(path, import.meta.url), "utf8");

test("Vercel Sandbox settings page shows credentials, region, and the SDK environment", async () => {
  const [settings, api, messages] = await Promise.all([
    source("../src/components/SettingsPage.tsx"),
    source("../src/api.ts"),
    source("../messages/en.json"),
  ]);
  assert.match(settings, /target\.id === "vercel" && <VercelSection/);
  assert.match(settings, /settings_vercel_from_process_env/);
  assert.match(settings, /settings_environment_tab/);
  assert.match(settings, /settings_vercel_region/);
  assert.match(settings, /settings_vercel_sdk/);
  assert.match(api, /getVercelSettings/);
  assert.match(messages, /settings_vercel_credentials_help/);
  const flavored = settings.match(/const FLAVORED_TARGETS[\s\S]*?\];/)[0];
  assert.ok(flavored.indexOf('"ray"') < flavored.indexOf('"vercel"'));
  assert.ok(flavored.indexOf('"vercel"') < flavored.indexOf('"openresearch"'));
  const quickSetup = settings.match(/const QUICK_SETUP_TARGETS[^\n]*\[[^\]]*\];/)[0];
  assert.doesNotMatch(quickSetup, /vercel/);
});
