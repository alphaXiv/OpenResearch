import assert from "node:assert/strict";
import test from "node:test";
import { QueryObserver } from "@tanstack/react-query";
import { createWorkspace } from "../src/workspace.tsx";
import { serve } from "./render.mjs";

const settle = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };

test("locale reporting waits for readiness, retries, and repeats on reconnect, language changes and another backend", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let available = false;
  const calls = [];
  serve(async (path, init) => {
    if (path !== "/api/telemetry/locale") return;
    const { locale } = JSON.parse(init.body);
    calls.push(locale);
    return available ? { locale } : new Response("", { status: 503 });
  });
  const [workspace, other] = [createWorkspace("local"), createWorkspace("another-backend")];
  t.after(() => [workspace, other].forEach(({ queryClient }) => queryClient.clear()));
  const { reportLocaleQuery } = workspace.queries;
  const observer = new QueryObserver(workspace.queryClient, { ...reportLocaleQuery("en"), enabled: false });
  t.after(observer.subscribe(() => {}));
  await settle();
  assert.deepEqual(calls, []);

  observer.setOptions({ ...reportLocaleQuery("en"), enabled: true });
  await settle();
  assert.equal(calls.length, 1);
  available = true;
  t.mock.timers.tick(1_000);
  await settle();
  assert.equal(observer.getCurrentResult().status, "success");
  assert.deepEqual(calls, ["en", "en"]);

  observer.setOptions({ ...reportLocaleQuery("en"), enabled: false });
  observer.setOptions({ ...reportLocaleQuery("en"), enabled: true });
  await settle();
  assert.deepEqual(calls, ["en", "en", "en"]);
  observer.setOptions({ ...reportLocaleQuery("ja"), enabled: true });
  await settle();
  assert.equal(calls.at(-1), "ja");

  await other.queryClient.fetchQuery(other.queries.reportLocaleQuery("ja"));
  assert.deepEqual(calls.slice(-2), ["ja", "ja"]);
});
