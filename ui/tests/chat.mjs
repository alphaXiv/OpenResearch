import { EditorView } from "@codemirror/view";
import { afterEach } from "node:test";
import { act } from "react";
import { toast } from "sonner";
import { fmtNumber } from "../src/api.ts";
import { m } from "../src/paraglide/messages.js";
import { clients, find, navigate, press, renderApp, serveApp, settle, streams } from "./render.mjs";
import { model } from "./models.mjs";

// The transcript virtualizes against its viewport, which happy-dom never lays out.
for (const size of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"]) {
  Object.defineProperty(HTMLElement.prototype, size, { configurable: true, get: () => 800 });
}

// Toasts belong to the page, which outlives each test's app.
afterEach(() => toast.dismiss());

export const codex = {
  id: "codex", name: "Codex", installed: true, installBroken: false, authenticated: true, authState: "ready", agentReady: true,
  supportsSteering: false, supportsCompaction: true, models: [model("gpt-6.1-sol"), model("gpt-6-astra")], defaultModel: "gpt-6.1-sol",
  options: { permissionModes: [{ id: "auto", label: "Auto" }], defaultPermissionMode: "auto", planActivation: "command", reasoningLevels: [] },
};

export const session = (id, fields = {}) => ({
  id, projectId: "p", harness: "codex", title: `Chat ${id}`, model: "gpt-6.1-sol", serviceTier: null, permissionMode: "auto", planMode: false,
  autonomy: "agentic", reasoningLevel: null, archived: false, createdAt: 1, updatedAt: 1, busy: false, activeLeafId: null, ...fields,
});

export const userMessage = (id, text, parentId = null) =>
  ({ id, role: "user", parts: [{ id: `${id}-text`, type: "text", text }], createdAt: 1, parentId });
export const assistantMessage = (id, parts, parentId = null) =>
  ({ id, role: "assistant", parts: typeof parts === "string" ? [{ id: `${id}-text`, type: "text", text: parts }] : parts, createdAt: 2, completedAt: 3, parentId });

/** A request the test answers later; answering lets the page finish handling it. */
export function deferred() {
  let yes, no;
  const promise = new Promise((resolve, reject) => { yes = resolve; no = reject; });
  const answer = async (finish) => {
    await act(async () => finish());
    await settle();
  };
  return { promise, resolve: (value) => answer(() => yes(value)), reject: (error) => answer(() => no(error)) };
}

/** A reply the server refused, which the app reports without asking whether it arrived. */
export const refused = (error = "refused") => Response.json({ error }, { status: 400 });

/** A connected remote host's runtime for its session `id`. */
const remoteRuntime = (id) => ({
  kind: "ssh", version: "test", buildChannel: "stable", dashboardProtocol: 1,
  session: { id, host: "gpu", user: "me", status: "connected", version: "test", dashboardProtocol: 1, gatewayUrl: "http://localhost:4910/", error: null, repairCommand: null, installPaths: null, uiPreferences: { theme: null, locale: null }, canStartNewHost: true },
});

/**
 * Renders the app at `path` over a backend holding `sessions` and their `transcripts` (message lists);
 * `routes` ("POST /api/path": reply, or "GET /api/prefix/*") answer first. Every request lands in `requests`.
 * A `remote` page's `reconnect` moves it to a fresh session on its host, which retires the page's workspace.
 */
export async function renderChat(path, { sessions = [session("s")], transcripts = {}, harnesses = [codex], routes = {}, remote = false } = {}) {
  const requests = [];
  let remoteSession = "one";
  const projects = [...new Set(sessions.map((row) => row.projectId))].map((id) => ({ id, name: `Project ${id}`, updatedAt: 1 }));
  serveApp({
    ...(remote ? { "GET /_orx/runtime": () => remoteRuntime(remoteSession) } : {}),
    "GET /api/projects": () => ({ projects }),
    "GET /api/projects/activity": () => ({ activity: [] }),
    "GET /api/chat/sessions": (_, url) => ({ sessions: sessions.filter((row) => url.includes(`projectId=${row.projectId}`)) }),
    "GET /api/chat/active-runs": () => [],
    "GET /api/harnesses": () => ({ harnesses }),
    "GET /api/harnesses/setup/commands": () => ({}),
    "GET /api/skills": () => ({ skills: [] }),
    ...Object.fromEntries(sessions.flatMap((row) => [
      [`GET /api/chat/sessions/${row.id}`, () => ({ session: row })],
      [`GET /api/chat/sessions/${row.id}/messages`, () => {
        const messages = transcripts[row.id] ?? [];
        return { messages, queued: [], activeLeafId: messages.at(-1)?.id ?? null };
      }],
    ])),
    ...routes,
  });
  const served = globalThis.fetch;
  globalThis.fetch = async (input, init = {}) => {
    const url = new URL(String(input), location.href);
    const request = `${init.method ?? "GET"} ${url.pathname}`;
    requests.push({ request, body: init.body ? JSON.parse(init.body) : undefined });
    const prefix = Object.keys(routes).find((key) => key.endsWith("/*") && request.startsWith(key.slice(0, -1)));
    if (!prefix) return served(input, init);
    const body = await routes[prefix](init, url.pathname);
    return body instanceof Response ? body : Response.json(body);
  };
  const view = await renderApp(path);
  const reconnect = async () => {
    remoteSession = "two";
    await act(async () => { for (const client of clients) void client.invalidateQueries({ predicate: (query) => query.queryKey[2] === "getRuntime" }); });
    await settle();
  };
  return { ...view, navigate, reconnect, requests, sent: () => requests.filter(({ request }) => /^POST \/api\/chat\/sessions\/[^/]+\/message$/.test(request)).map(({ body }) => body) };
}

/** Delivers a server event on the page's stream. */
export async function emit(type, data) {
  await act(async () => streams[0].dispatchEvent(new MessageEvent(type, { data: JSON.stringify(data) })));
  await settle();
}

/** Lets FileReader loads, which finish outside React, land. */
export async function idle() {
  await act(async () => new Promise((resolve) => setTimeout(resolve, 10)));
  await settle();
}

/** The `index`th composer on the page: the main chat's first, then a side chat's. */
export function composer(index = 0) {
  const box = () => document.querySelectorAll('[data-onboarding="composer"]')[index];
  const field = () => box().querySelector(".cm-content");
  const editor = () => EditorView.findFromDOM(box().querySelector(".cm-editor"));
  return {
    field,
    get draft() { return editor().state.doc.toString(); },
    get caret() { return editor().state.selection.main.head; },
    get attachments() { return box().querySelectorAll('[data-chip^="#"]').length; },
    get annotations() {
      const text = box().textContent;
      if (text.includes(m.chat_one_annotation())) return 1;
      return [2, 3, 4].find((count) => text.includes(m.chat_annotation_count({ count: fmtNumber(count) }))) ?? 0;
    },
    /** The warning that a send may have reached the agent, or null. */
    get warning() { return box().querySelector('[role="alert"]')?.textContent ?? null; },
    /** Types `text` at the caret. */
    async type(text) {
      const view = editor();
      await act(async () => view.dispatch(view.state.replaceSelection(text), { userEvent: "input.type" }));
      await settle();
    },
    async send(modifiers = {}) {
      const event = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true, ...modifiers });
      await act(async () => {
        field().focus();
        field().dispatchEvent(event);
      });
      await settle();
    },
    async attach(name = "figure.png") {
      const input = box().querySelector('input[type="file"]');
      Object.defineProperty(input, "files", { configurable: true, value: [new File([name], name, { type: "image/png" })] });
      await act(async () => input.dispatchEvent(new Event("change", { bubbles: true })));
      await idle();
    },
    /** Chooses the model the picker names `model` on `agent` in this composer's model picker. */
    async pick(model, agent = "Codex") {
      await act(async () => box().querySelector('[data-onboarding="model-picker"] button').click());
      await act(async () => document.querySelector(`button[aria-label="${m.model_picker_model_on({ model, agent })}"]`).click());
      await settle();
    },
    async dismissWarning() {
      await act(async () => box().querySelector('[role="alert"] button').click());
    },
  };
}

/** Selects `text` in the transcript's assistant replies and adds it to the composer. */
export async function annotate(text) {
  const reply = [...document.querySelectorAll(".msg-assistant .md")].find((element) => element.textContent.includes(text));
  const walker = document.createTreeWalker(reply, NodeFilter.SHOW_TEXT);
  let node = walker.nextNode();
  while (!node.data.includes(text)) node = walker.nextNode();
  const range = document.createRange();
  range.setStart(node, node.data.indexOf(text));
  range.setEnd(node, node.data.indexOf(text) + text.length);
  await act(async () => {
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(range);
    // The transcript reads a selection change on the next animation frame.
    await new Promise((resolve) => setTimeout(resolve));
  });
  await act(async () => find("button", m.chat_panel_ask_about_this()).click());
  await settle();
}

export { press, settle };
