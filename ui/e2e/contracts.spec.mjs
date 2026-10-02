import { test, expect } from "@playwright/test";
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { project, session, paneUrl, sql, post } from "./fixtures.mjs";

test.afterEach(async ({ page }) => { await page.unrouteAll({ behavior: "wait" }); });

for (const action of ["compile", "sync", "link", "open"]) {
  test(`restored paper stays passive until ${action}`, async ({ page, request }, info) => {
    sql("UPDATE ui_state SET onboarding_completed=1,tour_completed=1;");
    const p = await project(request), s = await session(request, p);
    writeFileSync(join(p.repoPath, "paper.tex"), "\\documentclass{article}\\begin{document}Paper\\end{document}\n");
    writeFileSync(join(p.repoPath, "other.tex"), "\\documentclass{article}\\begin{document}Other\\end{document}\n");
    execFileSync("git", ["-C", p.repoPath, "add", "paper.tex", "other.tex"]);
    execFileSync("git", ["-C", p.repoPath, "-c", "user.name=Test fixture", "-c", "user.email=fixture@example.invalid", "commit", "-m", "Seed paper"]);
    if (action === "open") {
      await post(request, `/api/chat/sessions/${s.id}/message`, { text: "Prepare the live checkout" });
      await expect.poll(async () => (await (await request.get(`/api/chat/sessions/${s.id}/worktree`)).json()).exists).toBe(true);
    }
    const calls = { compile: 0, sync: 0, status: 0 };
    let hasToken = action !== "link", linked = action !== "link";
    const state = () => ({ hasToken, hasSession: false, link: linked ? { projectId: "paper", url: "https://www.overleaf.com/project/paper" } : null });
    await page.route("**/api/latex/engine", (route) => route.fulfill({ json: { engine: "tectonic", hint: null, installCommand: null } }));
    await page.route("**/api/overleaf/token", (route) => { hasToken = true; return route.fulfill({ json: { hasToken } }); });
    await page.route("**/file/latex", (route) => { calls.compile++; return route.fulfill({ json: { ok: false, pdfPath: null, hadErrors: false, log: "Fixture compiler: no PDF", note: null } }); });
    await page.route(/\/file\/overleaf(?:\?|\/|$)/, (route) => {
      const url = route.request().url();
      if (url.includes("/sync")) { calls.sync++; return route.fulfill({ json: { pulled: [], pushed: [], conflicts: [] } }); }
      if (url.includes("/status")) { calls.status++; return route.fulfill({ json: { remoteChanged: true } }); }
      if (route.request().method() === "POST") linked = true;
      return route.fulfill({ json: state() });
    });
    await page.clock.install();
    await page.goto(paneUrl(p, { kind: "file", path: "paper.tex" }, s.id));
    await expect(page.getByRole("button", { name: "Compile PDF", exact: true })).toBeEnabled();
    await expect(page.getByRole("button", { name: /^Overleaf —/ })).toBeEnabled();
    await page.reload();
    await expect(page.getByRole("button", { name: "Compile PDF", exact: true })).toBeEnabled();
    await page.clock.fastForward(31_000);
    await page.locator("textarea.file-view-editarea").focus();
    expect(calls).toEqual({ compile: 0, sync: 0, status: 0 });
    if (action === "compile") await page.getByRole("button", { name: "Compile PDF", exact: true }).click();
    if (action === "sync" || action === "link") {
      await page.getByRole("button", { name: /^Overleaf —/ }).click();
      if (action === "link") {
        await page.getByRole("textbox", { name: "Overleaf Git token", exact: true }).fill("e2e-dummy-token");
        await page.getByRole("button", { name: "Save token", exact: true }).click();
        await expect(page.getByRole("button", { name: "Link and sync", exact: true })).toBeVisible();
        expect(calls).toEqual({ compile: 0, sync: 0, status: 0 });
        await page.getByPlaceholder("https://www.overleaf.com/project/…").fill("paper");
      }
      await page.getByRole("button", { name: action === "sync" ? "Sync files" : "Link and sync", exact: true }).click();
    }
    if (action === "open") {
      await page.getByRole("button", { name: "Files", exact: true }).click();
      await page.getByRole("button", { name: "other.tex", exact: true }).filter({ has: page.locator(".file-tree-name") }).press("Enter");
    }
    await expect.poll(() => calls).toEqual({ compile: 1, sync: 1, status: 0 });
    await expect(page.getByRole("button", { name: "Compile PDF", exact: true })).toBeEnabled();
    await page.clock.runFor(100);
    await page.clock.fastForward(31_000);
    await expect.poll(() => calls).toEqual({ compile: 1, sync: 2, status: 1 });
    await info.attach("external-boundary-requests", { body: JSON.stringify({ action, calls }), contentType: "application/json" });
  });
}

for (const outcome of ["success", "failure", "popup-blocked"]) {
  test(`remote connection handles ${outcome} without losing the home tab`, async ({ page, request }, info) => {
    sql("UPDATE ui_state SET onboarding_completed=1,tour_completed=1;");
    await project(request);
    await page.route("**/api/settings/ssh", (route) => route.fulfill({ json: { hosts: [{ host: "research" }], defaultHost: null } }));
    const requests = [];
    await page.route("**/api/remote/sessions", (route) => {
      if (route.request().method() === "GET") return route.fulfill({ json: { sessions: [] } });
      requests.push(route.request().postDataJSON());
      return route.fulfill(outcome === "failure" ? { status: 500, json: { error: "Fixture connection failed" } } : { json: { gatewayUrl: new URL("/remote-launch?connected", page.url()).href } });
    });
    if (outcome === "popup-blocked") await page.addInitScript(() => { window.open = () => null; });
    await page.goto("/projects");
    await page.getByRole("button", { name: "Local", exact: true }).click();
    const popup = outcome === "popup-blocked" ? null : page.waitForEvent("popup");
    await page.getByRole("button", { name: "research", exact: true }).click();
    if (outcome === "popup-blocked") {
      await expect(page.getByText(/Your browser blocked the remote workspace tab/)).toBeVisible();
      expect(requests).toHaveLength(0);
    } else {
      const tab = await popup;
      if (outcome === "failure") {
        await expect.poll(() => tab.isClosed()).toBe(true);
        await expect(page.getByText("Fixture connection failed", { exact: true })).toBeVisible();
      } else {
        await expect(tab).toHaveURL(/\/remote-launch\?connected$/);
        await expect(page.getByRole("dialog")).toHaveCount(0);
        await tab.close();
      }
      expect(requests).toEqual([{ host: "research", uiPreferences: { theme: "system", locale: "en" } }]);
    }
    await expect(page).toHaveURL(/\/projects$/);
    await info.attach("connection-requests", { body: JSON.stringify({ outcome, requests }), contentType: "application/json" });
  });
}

for (const kind of ["local", "ssh"]) for (const scenario of ["loading", "error", "onboarding", "projects"]) {
  test(`${kind} home connection control during ${scenario}`, async ({ page, request }, info) => {
    const p = kind === "ssh" && scenario === "projects" ? await project(request) : null;
    await page.route("**/api/events", (route) => route.abort());
    if (kind === "ssh") await page.route("**/_orx/runtime", (route) => route.fulfill({ json: {
      kind, version: "e2e", dashboardProtocol: 1, session: { id: "e2e", host: "research", user: "tester", status: "connected", version: "e2e", dashboardProtocol: 1, error: null, installPaths: null, uiPreferences: { theme: null, locale: null }, canStartNewHost: true },
    } }));
    let release;
    const hold = new Promise((resolve) => { release = resolve; });
    await page.route("**/api/projects", async (route) => {
      if (scenario === "loading") await hold;
      await route.fulfill(scenario === "error" ? { status: 500, json: { error: "Fixture offline" } } : { json: { projects: p ? [p] : [] } });
    });
    await page.route("**/api/settings/ui-state", async (route) => {
      const response = await route.fetch(), body = await response.json();
      if (scenario === "onboarding") body.onboardingCompleted = false;
      else body.onboardingCompleted = true;
      await route.fulfill({ response, json: body });
    });
    try {
      await page.goto("/projects");
      if (scenario === "error") await expect(page.getByText("Fixture offline", { exact: true })).toBeVisible();
      if (scenario === "onboarding") await expect(page.getByRole("heading", { name: "A workspace for your research agents", exact: true })).toBeVisible();
      const control = page.getByRole("button", { name: kind === "ssh" ? /SSH.*research/ : "Local", exact: kind === "local" });
      if (kind === "ssh" || scenario === "projects") await expect(control).toBeVisible();
      else await expect(control).toHaveCount(0);
      if (kind === "ssh") {
        await control.click();
        await expect(page.getByRole("button", { name: "Disconnect", exact: true })).toBeVisible();
      }
      if (p) {
        await page.keyboard.press("Escape");
        await page.goto(`/projects/${p.id}/tasks/new`);
        await expect(control).toBeVisible();
        await control.click();
        await expect(page.getByRole("button", { name: "Disconnect", exact: true })).toBeVisible();
      }
      await info.attach("home-state", { body: JSON.stringify({ kind, scenario }), contentType: "application/json" });
    } finally { release(); }
  });
}

for (const [backend, label, fields, edit] of [
  ["k8s", "Kubernetes", ["context", "namespace"], "Namespace"],
  ["slurm", "Slurm", ["host", "partition", "account", "timeLimit"], "Account"],
  ["ray", "Ray", ["address"], "Jobs / Dashboard URL"],
]) {
  test(`${label} refresh updates clean fields and preserves dirty fields`, async ({ page, request }, info) => {
    sql("UPDATE ui_state SET onboarding_completed=1,tour_completed=1;");
    const p = await project(request);
    let value = "original", reads = 0;
    await page.route(`**/api/settings/${backend}`, async (route) => {
      const response = await route.fetch(), body = await response.json();
      Object.assign(body, Object.fromEntries(fields.map((field) => [field, value])));
      if (backend === "k8s") body.contexts = ["original", "refreshed", "external"];
      if (backend === "slurm") body.hosts = [{ host: "original" }, { host: "refreshed" }, { host: "external" }];
      reads++;
      await route.fulfill({ response, json: body });
    });
    await page.clock.install();
    await page.goto(`/projects/${p.id}/settings/compute`);
    await page.getByRole("button", { name: new RegExp(label) }).click();
    const input = page.getByRole("textbox", { name: edit, exact: true });
    const selector = backend === "ray" ? null : page.getByRole("button", { name: backend === "k8s" ? "Context" : "Login node", exact: true });
    await expect(input).toHaveValue("original");
    if (selector) await expect(selector).toContainText("original");
    const refresh = async () => {
      const before = reads;
      await page.clock.fastForward(301_000);
      await page.evaluate(() => window.dispatchEvent(new Event("visibilitychange")));
      await expect.poll(() => reads).toBeGreaterThan(before);
    };
    value = "refreshed";
    await refresh();
    await expect(input).toHaveValue("refreshed");
    if (selector) await expect(selector).toContainText("refreshed");
    await input.fill("my edit");
    const before = await page.locator("input").evaluateAll((inputs) => inputs.map((element) => element.value));
    value = "external";
    await refresh();
    await expect(input).toHaveValue("my edit");
    if (selector) await expect(selector).toContainText("refreshed");
    expect(await page.locator("input").evaluateAll((inputs) => inputs.map((element) => element.value))).toEqual(before);
    await info.attach("settings-refresh", { body: JSON.stringify({ backend, reads, before }), contentType: "application/json" });
  });
}
