import { chromium } from "@playwright/test";
const out = "/Users/clo/or/19/.shots/pr";
const browser = await chromium.launch();
const FOUND = ["claude-code", "codex", "opencode", "antigravity"];
const fresh = (h) => FOUND.includes(h.id) && !h.profile ? { ...h, installed: h.id === "codex", authenticated: false, agentReady: false, authState: "needsLogin", authMethod: undefined, account: undefined, accountName: undefined, plan: undefined, defaultModel: undefined, models: [] } : h;
const sample = (h) => h.id === "codex" && !h.profile ? { ...h, account: "ada@example.org", accountName: "Ada", plan: "Plus" } : h;
const open = async (scheme, mode) => {
  const page = await browser.newPage({ viewport: { width: 1280, height: 1000 }, colorScheme: scheme, deviceScaleFactor: 2 });
  if (mode === "fresh") {
    await page.addInitScript(() => {
      const Real = window.WebSocket;
      const Stub = function (url, ...rest) { return String(url).includes("/api/harnesses/setup") ? { readyState: 0, send() {}, close() {} } : new Real(url, ...rest); };
      Object.assign(Stub, { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 });
      window.WebSocket = Stub;
    });
  }
  await page.route(/\/api\/harnesses(\?|$)/, async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    body.harnesses = body.harnesses.filter((h) => !h.profile).map(mode === "fresh" ? fresh : sample);
    await route.fulfill({ response, json: body });
  });
  await page.route(/\/api\/settings\/ui-state(\?|$)/, async (route) => {
    if (route.request().method() !== "GET") return route.continue();
    const response = await route.fetch();
    await route.fulfill({ response, json: { ...(await response.json()), onboardingCompleted: false, hiddenAgents: [] } });
  });
  await page.route(/\/api\/projects(\?|$)/, async (route) => {
    if (route.request().method() !== "GET") return route.continue();
    const response = await route.fetch();
    const body = await response.json();
    await route.fulfill({ response, json: Array.isArray(body) ? [] : { ...body, projects: [] } });
  });
  await page.goto("http://localhost:43272/projects");
  await page.locator(".onb-intro button:not([disabled])").click({ timeout: 20000 });
  await page.getByRole("button", { name: "Dismiss" }).click({ timeout: 2000 }).catch(() => {});
  await page.getByRole("heading", { name: "Set up your agents" }).waitFor({ timeout: 20000 });
  await page.waitForTimeout(2500);
  await page.mouse.move(1200, 20);
  return page;
};
const shot = (page, name, full = false) => page.screenshot({ path: `${out}/${name}.png`, fullPage: full });
try {
  let page = await open("dark", "fresh");
  await shot(page, "1-fresh-dark", true);
  await page.getByRole("switch", { name: "Free Models" }).hover();
  await page.waitForTimeout(700);
  await shot(page, "2-free-models-locked", true);
  await page.close();

  page = await open("light", "fresh");
  await shot(page, "1-fresh-light", true);
  await page.close();

  page = await open("dark", "agents");
  await shot(page, "3-agents-dark", true);
  await page.getByRole("button", { name: /^Custom endpoint/ }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByText("Running on this computer").waitFor({ timeout: 15000 });
  await page.waitForTimeout(400);
  await shot(page, "5-endpoint-address");
  await dialog.getByRole("button", { name: "Ollama" }).click();
  await page.waitForTimeout(500);
  await dialog.getByLabel("API key").fill("sk-local");
  await shot(page, "6-endpoint-key");
  await dialog.getByRole("button", { name: "Find models" }).click();
  await page.waitForTimeout(1500);
  await shot(page, "7-endpoint-models");
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await page.waitForTimeout(500);
  await page.getByRole("button", { name: /^Other providers/ }).click();
  await page.getByRole("dialog", { name: "Add agent" }).waitFor();
  await page.waitForTimeout(500);
  await shot(page, "4-other-providers");
  await page.getByRole("dialog").getByRole("button", { name: "Cancel" }).click();
  await page.close();

  page = await open("light", "agents");
  await shot(page, "3-agents-light", true);
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  await page.getByRole("heading", { name: "Bring your alphaXiv library" }).waitFor();
  await page.waitForTimeout(800);
  await shot(page, "9-step-2-light", true);
  await page.close();
  console.log("ok");
} catch (e) { console.log(e.message.split("\n").slice(0, 3).join(" ")); } finally {
  await browser.close();
}
