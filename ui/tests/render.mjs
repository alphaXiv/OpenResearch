import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach } from "node:test";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { MachineSelectionContext } from "../src/machineSelection.tsx";
import { createWorkspace, WorkspaceContext } from "../src/workspace.tsx";

const mounted = new Set();
const unserved = globalThis.fetch;

/** Every workspace's query client, including those the app creates for itself; each test starts with none. */
export const clients = new Set();
const { mount } = QueryClient.prototype;
QueryClient.prototype.mount = function () {
  clients.add(this);
  return mount.call(this);
};

/** The page's event streams, which never connect; a test dispatches the server's events on them. */
export const streams = [];
globalThis.EventSource = class extends EventTarget {
  static CLOSED = 2;
  readyState = 0;
  constructor(url) {
    super();
    this.url = url;
    streams.push(this);
  }
  close() { this.readyState = 2; }
};

afterEach(async () => {
  for (const unmount of mounted) await unmount();
  for (const client of clients) client.clear();
  clients.clear();
  globalThis.fetch = unserved;
  streams.length = 0;
  history.replaceState(null, "", "/");
});

async function mountRoot(element) {
  const container = document.body.appendChild(document.createElement("div"));
  const root = createRoot(container);
  const show = (next) => act(async () => root.render(next));
  const unmount = async () => {
    mounted.delete(unmount);
    await act(async () => root.unmount());
    container.remove();
  };
  mounted.add(unmount);
  await show(element);
  return { container, show, unmount };
}

/** Mounts `element` on a local page showing `workspace` and its dialogs; every mount ends with its test. */
export async function render(element, workspace = createWorkspace("test")) {
  const machines = { owner: workspace, activeId: "local", sessions: [], canSwitch: false, selectLocal() {}, selectRemote() {} };
  const wrap = (next) => createElement(MachineSelectionContext, { value: machines }, createElement(WorkspaceContext, { value: workspace },
    createElement(QueryClientProvider, { client: workspace.queryClient }, next, createElement(workspace.dialogs.DialogManager))));
  const { container, show, unmount } = await mountRoot(wrap(element));
  return { container, workspace, rerender: (next) => show(wrap(next)), unmount };
}

// Captured before any test mocks timers, so settling still advances under t.mock.timers.
const { setTimeout: realSetTimeout } = globalThis;
const tick = () => act(() => new Promise((resolve) => realSetTimeout(resolve)));

const emptyBackend = [
  [/^GET \/_orx\/runtime$/, () => ({ kind: "local", version: "test", buildChannel: "stable" })],
  [/^GET \/api\/projects$/, () => ({ projects: [] })],
  [/^GET \/api\/settings\/ui-state$/, () => ({ onboardingCompleted: true })],
  [/^GET \/api\/update$/, () => null],
  [/^GET \/api\/remote\/sessions$/, () => ({ sessions: [] })],
  [/^POST \/api\/telemetry\/(locale|event)$/, (init) => JSON.parse(init.body)],
  [/^GET \/api\/chat\/sessions$/, () => ({ sessions: [] })],
  [/^GET \/api\/projects\/[^/]+\/ui-state$/, () => null],
  [/^POST \/api\/(settings|projects\/[^/]+)\/ui-state$/, (init) => JSON.parse(init.body)],
  [/^GET \/api\/projects\/[^/]+\/experiments$/, () => ({ experiments: [] })],
  [/^GET \/api\/projects\/[^/]+\/runs$/, () => ({ runs: [] })],
];

/** Serves a local backend with no work in it; `routes` ("GET /api/path": reply) answer first. */
export function serveApp(routes = {}) {
  serve((path, init) => {
    const request = `${init.method ?? "GET"} ${path.split("?")[0]}`;
    if (routes[request]) return routes[request](init, path);
    return emptyBackend.find(([pattern]) => pattern.test(request))?.[1](init);
  });
}

/** Renders the whole app at `path`, as the browser loads it. */
export async function renderApp(path) {
  const { AppRoot } = await import("../src/AppRoot.tsx");
  history.replaceState(null, "", path);
  const view = await mountRoot(createElement(AppRoot));
  await settle();
  return view;
}

/** Moves the browser to `path`, as its back and forward buttons do. */
export async function navigate(path) {
  await act(async () => {
    history.pushState(null, "", path);
    window.dispatchEvent(new PopStateEvent("popstate"));
  });
  await settle();
}

/** Lets the app's requests, and the renders they cause, finish; gives up on a request the test holds open. */
export async function settle() {
  const busy = () => [...clients].some((client) => client.isFetching() || client.isMutating());
  for (let ticks = 0; ticks < 10 && busy(); ticks++) await tick();
  await tick();
  await tick();
}

/** Answers the app's API requests with `handler(path, init)`: a Response, a JSON body, or undefined to fail the request. */
export function serve(handler) {
  globalThis.fetch = async (input, init = {}) => {
    const url = new URL(String(input), location.href);
    const body = await handler(url.pathname + url.search, init);
    if (body === undefined) throw new Error(`unexpected fetch: ${input}`);
    return body instanceof Response ? body : Response.json(body);
  };
}

/** The first `selector` match in the page whose text includes `text`; dialogs and menus portal out of a render's container. */
export function find(selector, text = "") {
  return [...document.body.querySelectorAll(selector)].find((element) => element.textContent.includes(text)) ?? null;
}

/** Types `value` into a field the way keystrokes reach React: through the native setter, then an input event. */
export async function type(field, value) {
  const { set } = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(field), "value");
  await act(async () => {
    set.call(field, value);
    field.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

/** Presses `key`, with `modifiers`, on the focused element or on `target`. */
export async function press(key, target = document.activeElement ?? document.body, modifiers = {}) {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...modifiers });
  await act(async () => target.dispatchEvent(event));
  return event;
}
