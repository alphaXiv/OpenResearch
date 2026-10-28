import test from "node:test";
import assert from "node:assert/strict";
import { QueryObserver } from "@tanstack/react-query";
import { queryModules } from "./queryModules.mjs";

const settle = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };

test("locale reporting waits for readiness, retries, and repeats on reconnect and language changes", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let available = false;
  const calls = [];
  const { load, client } = queryModules({ reportLocale: async (locale) => {
    calls.push(locale);
    if (!available) throw new Error("503");
    return { locale };
  } }, { nativeClient: true });
  t.after(() => client.clear());
  const query = load("settings").reportLocaleQuery;
  const observer = new QueryObserver(client, { ...query("en"), enabled: false });
  const off = observer.subscribe(() => {});
  t.after(off);
  await settle();
  assert.deepEqual(calls, []);
  observer.setOptions({ ...query("en"), enabled: true });
  await settle();
  assert.equal(calls.length, 1);
  available = true;
  t.mock.timers.tick(1_000);
  await settle();
  assert.equal(observer.getCurrentResult().status, "success");
  assert.deepEqual(calls, ["en", "en"]);
  observer.setOptions({ ...query("en"), enabled: false });
  observer.setOptions({ ...query("en"), enabled: true });
  await settle();
  assert.deepEqual(calls, ["en", "en", "en"]);
  observer.setOptions({ ...query("ja"), enabled: true });
  await settle();
  assert.equal(calls.at(-1), "ja");
  load("client").activateWorkspace("another-backend");
  observer.setOptions({ ...query("ja"), enabled: true });
  await settle();
  assert.deepEqual(calls.slice(-2), ["ja", "ja"]);
});
