import { test, expect } from "@playwright/test";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { fixture, data, sqlValue, sql, paneUrl, post, project, session } from "./fixtures.mjs";

test.afterEach(async ({ page }) => { await page.unrouteAll({ behavior: "wait" }); });

async function pickModel(page, model) {
  await page.locator(".model-picker > button").click();
  await page.getByRole("button", { name: /^Model/ }).click();
  if (model) {
    await page.getByPlaceholder("Search or enter model ID…").fill(model);
    await page.getByRole("button", { name: /Use .* as the model ID/ }).first().click();
  } else {
    await page.getByRole("button", { name: "Default model", exact: true }).first().click();
  }
}
async function send(page, text) {
  const replies = page.getByText(/E2E reply using/);
  const before = await replies.count();
  await page.locator(".composer-input textarea").fill(text);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(replies).toHaveCount(before + 1);
  await expect(page.getByRole("button", { name: "Stop", exact: true })).toHaveCount(0);
  await expect(replies.last()).toBeVisible();
}

test.beforeEach(async ({ request }) => {
  sql("UPDATE ui_state SET onboarding_completed=1,tour_completed=1;");
  await post(request, "/api/settings/ui-state", { preferredAgent: { harness: "codex", model: "fixture-model", reasoningLevel: "high" } });
});

test("manual and default models reach the real agent transport and survive reload", async ({ page, request }, info) => {
  const p = await project(request);
  await page.goto(`/projects/${p.id}/tasks/new`);
  await pickModel(page, "provider/manual-model");
  await send(page, "Check the selected model");
  await expect(page.getByText("E2E reply using provider/manual-model", { exact: true })).toBeVisible();
  await page.reload();
  await expect(page.locator(".model-picker > button")).toContainText("Manual Model");
  await page.route("**/api/harnesses", async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    body.harnesses.find((harness) => harness.id === "codex").models = [];
    await route.fulfill({ response, json: body });
  });
  await page.reload();
  await expect(page.locator(".model-picker > button")).toContainText("Manual Model");
  await send(page, "Send with an empty catalog");
  await pickModel(page, null);
  await send(page, "Use the CLI default now");
  await expect(page.locator(".model-picker > button")).toContainText("Default model");
  const native = readFileSync(join(fixture, "native-requests.jsonl"), "utf8").trim().split("\n").map(JSON.parse);
  const turns = native.filter((entry) => entry.method === "turn/start");
  expect(turns.at(-3).params.model).toBe("provider/manual-model");
  expect(turns.at(-2).params.model).toBe("provider/manual-model");
  expect(turns.at(-1).params.model).toBeUndefined();
  const messages = await (await request.get(`/api/chat/sessions/${page.url().split("/tasks/")[1].split("?")[0]}/messages`)).json();
  expect(messages.messages.filter((message) => message.role === "user")).toHaveLength(3);
  await info.attach("persisted-transcript", { body: JSON.stringify(messages, null, 2), contentType: "application/json" });
});

test("connection dialogs work from both home and the project sidebar", async ({ page, request }) => {
  const p = await project(request);
  for (const path of ["/projects", `/projects/${p.id}/tasks/new`]) {
    await page.goto(path);
    await page.getByRole("button", { name: path === "/projects" ? "Local" : "Connect to remote", exact: true }).click();
    const hosts = page.getByRole("dialog", { name: "Connect to remote" });
    await expect(hosts.getByRole("textbox", { name: "Search SSH hosts" })).toBeFocused();
    await hosts.getByRole("button", { name: "Configure SSH hosts…" }).click();
    await expect(page.getByRole("dialog", { name: "SSH config" })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(hosts).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
  }
});

test("Tinker settings are visible and its test credential persists masked", async ({ page, request }) => {
  const p = await project(request);
  await page.goto(`/projects/${p.id}/settings/compute`);
  await expect(page.getByRole("button", { name: /Tinker/ })).toBeVisible();
  await page.getByRole("button", { name: /Tinker/ }).click();
  await expect(page.getByRole("textbox", { name: "API key" })).toBeVisible();
  await expect(page.locator(".tinker-logo").last()).toBeVisible();
  await page.getByRole("button", { name: "Close panel" }).click();
  await post(request, "/api/settings/env", { key: "TINKER_API_KEY", value: "e2e-test-value" });
  await page.goto(`/projects/${p.id}/settings/environment`);
  await expect(page.getByText("TINKER_API_KEY", { exact: true })).toBeVisible();
  await expect(page.getByText("e2e-test-value", { exact: true })).toHaveCount(0);
});

test("run links select the matching log and never substitute missing or foreign runs", async ({ page, request }, info) => {
  const p = await project(request);
  const experiment = `exp-${randomUUID()}`;
  const foreign = `foreign-${randomUUID()}`;
  sql(`INSERT INTO local_experiments (id,project_id,slug,branch_name,run_command,created_at,updated_at) VALUES (${sqlValue(experiment)},${sqlValue(p.id)},'test','main','true',1,1);`);
  const runs = ["old", "new", "foreign"].map((name) => ({ id: `${name}-${randomUUID()}`, name }));
  mkdirSync(join(data, "run-logs"), { recursive: true });
  for (const [index, run] of runs.entries()) {
    sql(`INSERT INTO runs (id,experiment_id,project_id,status,backend_json,created_at,updated_at) VALUES (${sqlValue(run.id)},${sqlValue(run.name === "foreign" ? foreign : experiment)},${sqlValue(p.id)},'done','{"kind":"tinker_job"}',${index + 1},${index + 1});`);
    writeFileSync(join(data, "run-logs", `${run.id}.log`), `E2E ${run.name} log\n`);
  }
  const logs = [];
  page.on("request", (request) => { if (/\/api\/runs\/[^/]+\/log\?/.test(request.url())) logs.push(request.url()); });
  for (const [runId, expected] of [[undefined, runs[1]], [runs[0].id, runs[0]], ["missing", null], [runs[2].id, null]]) {
    logs.length = 0;
    const snapshots = Promise.all(["experiments", "runs"].map((kind) => page.waitForResponse((response) => response.url().endsWith(`/api/projects/${p.id}/${kind}`)).then((response) => response.body())));
    await page.goto(paneUrl(p, { kind: "experiment", experimentId: experiment, view: "terminal", ...(runId ? { runId } : {}) }));
    await snapshots;
    await expect(page.getByRole("complementary").getByRole("button", { name: "test", exact: true })).toBeVisible();
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    if (expected) {
      await expect.poll(() => logs.some((url) => url.includes(expected.id))).toBe(true);
      await expect(page.locator(".xterm-screen")).toBeVisible();
      await expect(page.locator(".xterm-rows")).toContainText(`E2E ${expected.name} log`);
      expect(logs.every((url) => url.includes(expected.id))).toBe(true);
    } else {
      await expect(page.getByRole("complementary").getByText("Unavailable", { exact: true })).toBeVisible();
      expect(logs).toHaveLength(0);
    }
  }
  await page.goto(paneUrl(p, { kind: "experiment", experimentId: experiment, view: "overview" }));
  await expect(page.getByRole("complementary").locator(".tinker-logo")).toBeVisible();
  await info.attach("run-fixture", { body: JSON.stringify({ project: p.id, experiment, runs }, null, 2), contentType: "application/json" });
});

test("file edits persist on disk and restore in the same task after reload", async ({ page, request }, info) => {
  const p = await project(request);
  const s = await session(request, p);
  await page.goto(paneUrl(p, { kind: "file", path: "notes.txt" }, s.id));
  const editor = page.locator("textarea.file-view-editarea");
  await expect(editor).toHaveValue("original notes\n");
  await editor.fill("saved through the dashboard\n");
  await editor.press("ControlOrMeta+s");
  await expect.poll(() => readFileSync(join(p.repoPath, "notes.txt"), "utf8")).toBe("saved through the dashboard\n");
  await page.reload();
  await expect(editor).toHaveValue("saved through the dashboard\n");
  await info.attach("saved-file", { path: join(p.repoPath, "notes.txt"), contentType: "text/plain" });
});

test("long conversations retain selections, expose full history, and reset scroll on chat switch", async ({ page, request }, info) => {
  const p = await project(request);
  const sessions = [await session(request, p), await session(request, p)];
  for (const s of sessions) {
    let parent = null;
    for (let index = 0; index < 80; index++) {
      const id = `${s.id}-${index}`;
      const parts = [{ id: `${id}-text`, type: "text", text: `Transcript ${s.id} line ${index}`, ...(index % 2 ? { phase: "final_answer" } : {}) }];
      sql(`INSERT INTO chat_messages (id,session_id,role,parts_json,created_at,parent_id) VALUES (${sqlValue(id)},${sqlValue(s.id)},${sqlValue(index % 2 ? "assistant" : "user")},${sqlValue(JSON.stringify(parts))},${index + 1},${parent ? sqlValue(parent) : "NULL"});`);
      parent = id;
    }
    sql(`UPDATE chat_sessions SET active_leaf_id=${sqlValue(parent)} WHERE id=${sqlValue(s.id)};`);
  }
  await page.goto(`/projects/${p.id}/tasks/${sessions[0].id}`);
  const thread = page.locator(".chat-thread");
  await expect(thread.getByText(`Transcript ${sessions[0].id} line 79`, { exact: true })).toBeVisible();
  await thread.evaluate((element) => { element.scrollTop = 0; });
  const first = thread.locator(`[data-message-id="${sessions[0].id}-0"]`);
  await expect(first).toBeVisible();
  await first.evaluate((element) => {
    const range = document.createRange();
    range.selectNodeContents(element);
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(range);
    document.dispatchEvent(new Event("selectionchange"));
  });
  await thread.evaluate((element) => { element.scrollTop = element.scrollHeight; });
  await expect(first).toHaveCount(1);
  await page.getByRole("button", { name: "Show full conversation" }).focus();
  await page.keyboard.press("Enter");
  await expect(thread.locator("[data-message-id]")).toHaveCount(80);
  await thread.evaluate((element) => { element.scrollTop = 0; });
  await page.getByRole("button", { name: "Scroll to bottom", exact: true }).click();
  await expect.poll(() => thread.evaluate((element) => element.scrollHeight - element.scrollTop - element.clientHeight)).toBeLessThan(80);
  await page.locator(".composer-input textarea").fill("A draft\nwith multiple\nlines\nresizes the footer");
  await expect.poll(() => thread.evaluate((element) => element.scrollHeight - element.scrollTop - element.clientHeight)).toBeLessThan(80);
  await page.setViewportSize({ width: 1200, height: 720 });
  await expect.poll(() => thread.evaluate((element) => element.scrollHeight - element.scrollTop - element.clientHeight)).toBeLessThan(80);
  await thread.evaluate((element) => { element.scrollTop = 0; });
  await expect(first).toBeVisible();
  await page.setViewportSize({ width: 1300, height: 850 });
  await expect(first).toBeVisible();
  await expect.poll(() => thread.evaluate((element) => element.scrollTop)).toBeLessThan(80);
  await page.goto(`/projects/${p.id}/tasks/${sessions[1].id}`);
  await expect(first).toHaveCount(0);
  await expect(thread.getByText(`Transcript ${sessions[1].id} line 79`, { exact: true })).toBeVisible();
  await expect.poll(() => thread.evaluate((element) => element.scrollHeight - element.scrollTop - element.clientHeight)).toBeLessThan(80);
  await page.setViewportSize({ width: 1200, height: 720 });
  await expect.poll(() => thread.evaluate((element) => element.scrollHeight - element.scrollTop - element.clientHeight)).toBeLessThan(80);
  await info.attach("conversation-fixture", { body: JSON.stringify({ project: p.id, sessions: sessions.map((s) => s.id), messagesPerSession: 80 }), contentType: "application/json" });
});

test("catalog reasoning and speed replace unsupported stored choices before sending", async ({ page, request }, info) => {
  const p = await project(request);
  await post(request, "/api/settings/ui-state", { preferredAgent: { harness: "codex", model: "fixture-model", reasoningLevel: "high", serviceTier: "priority" } });
  await page.route("**/api/harnesses", async (route) => {
    const response = await route.fetch(), body = await response.json();
    Object.assign(body.harnesses.find((harness) => harness.id === "codex").models[0], { reasoningLevels: [{ id: "medium", label: "Medium" }], defaultReasoningLevel: "medium", serviceTiers: [] });
    await route.fulfill({ response, json: body });
  });
  await page.goto(`/projects/${p.id}/tasks/new`);
  await expect(page.locator(".model-picker > button")).toContainText("Medium");
  await send(page, "Reconcile unsupported choices");
  const native = readFileSync(join(fixture, "native-requests.jsonl"), "utf8").trim().split("\n").map(JSON.parse);
  const turn = native.filter((entry) => entry.method === "turn/start").at(-1);
  expect(turn.params.effort).toBe("medium");
  expect(turn.params.serviceTier).toBe("default");
  await info.attach("native-turn", { body: JSON.stringify(turn), contentType: "application/json" });
});
