import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { OpenSharedProjectForm } from "../src/components/OpenSharedProject.tsx";
import { fmtBytes } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { find, render, serve, settle } from "./render.mjs";

const project = { id: "p1", name: "Tiny LM" };
const preview = { sharedProject: { slug: "abcdefghijkl", url: "https://openresearch.sh/p/abcdefghijkl", name: "Tiny LM", byteSize: 4 << 20, publisher: "Ada" } };
const pathStatus = { exists: false, resolvedPath: "/Users/ada/OpenResearch/tiny-lm" };

/** The open-link dialog over a backend whose import answers with `importReply`, then `polls` in turn. */
async function openLink({ importReply, polls = [], timers }) {
  const requests = [];
  serve((path, init) => {
    const request = `${init.method ?? "GET"} ${path.split("?")[0]}`;
    requests.push(request);
    if (request === "GET /api/shared-projects/abcdefghijkl") return preview;
    if (request === "GET /api/project-path/status") return pathStatus;
    if (request === "POST /api/projects/import") return importReply();
    if (request === "GET /api/operations/i1") return polls.shift();
  });
  const opened = [];
  await render(createElement(OpenSharedProjectForm, {
    source: { link: "abcdefghijkl" },
    onCancel() {},
    onOpened: (result) => opened.push(result),
  }));
  await settle();
  timers?.enable({ apis: ["setTimeout"] });
  await act(() => find("button", m.open_shared_submit()).click());
  await settle();
  return { opened, requests };
}

/** The progress bar shows `label` and `bytes`, and is `percent` full or, with null, animated. */
function assertProgress(label, bytes, percent) {
  const bar = find("[role=status] [role=progressbar]", label);
  assert.ok(bar, `no progress bar labelled ${label}`);
  assert.ok(bar.textContent.includes(bytes));
  assert.equal(bar.getAttribute("aria-valuenow"), percent);
}

const status = (fields) => ({ phase: "downloading", bytes: 0, totalBytes: null, result: null, error: null, ...fields });

test("a slow download shows its progress until the project opens", async (t) => {
  const { opened } = await openLink({
    timers: t.mock.timers,
    importReply: () => Response.json({ operationId: "i1" }, { status: 202 }),
    polls: [
      status({ bytes: 1 << 20, totalBytes: 4 << 20 }),
      status({ phase: "unpacking", bytes: 3 << 20, totalBytes: 12 << 20 }),
      status({ phase: "opening" }),
      status({ result: { project, missingBranches: [] } }),
    ],
  });
  assertProgress(m.open_shared_downloading(), m.operation_bytes({ done: fmtBytes(1 << 20), total: fmtBytes(4 << 20) }), "25");

  await act(() => t.mock.timers.tick(500));
  await settle();
  assertProgress(m.open_shared_unpacking(), m.operation_bytes({ done: fmtBytes(3 << 20), total: fmtBytes(12 << 20) }), "25");

  await act(() => t.mock.timers.tick(500));
  await settle();
  assertProgress(m.open_shared_opening(), "", null);
  assert.equal(opened.length, 0);

  await act(() => t.mock.timers.tick(500));
  await settle();
  assert.deepEqual(opened, [{ project, missingBranches: [] }]);
});

test("a download that fails says why and lets the user try again", async () => {
  await openLink({
    importReply: () => Response.json({ operationId: "i1" }, { status: 202 }),
    polls: [status({ error: "This project link doesn't work; its publisher may have unpublished it." })],
  });
  assert.ok(find("[role=alert]", "unpublished"));
  assert.equal(find("button", m.open_shared_submit()).disabled, false);
});

test("an older orx up that answers with the project opens it without polling", async () => {
  const { opened, requests } = await openLink({ importReply: () => ({ project, missingBranches: ["orx/gone"] }) });
  assert.deepEqual(opened, [{ project, missingBranches: ["orx/gone"] }]);
  assert.ok(!requests.includes("GET /api/operations/i1"));
});

test("a dropped connection says orx was lost, not the browser's bare error", async () => {
  await openLink({ importReply: () => Promise.reject(new TypeError("Load failed")) });
  assert.ok(find("[role=alert]", m.request_connection_lost()));
  assert.ok(!find("[role=alert]", "Load failed"));
});

test("publishing shows its upload until the link appears", async (t) => {
  const { ShareProjectDialog } = await import("../src/components/ShareProjectDialog.tsx");
  const link = { id: "l1", slug: "abcdefghijkl", url: "https://openresearch.sh/p/abcdefghijkl", name: "Tiny LM", byteSize: 8 << 20, publishedAt: "2026-11-11T00:00:00Z" };
  const polls = [
    { ...status({ phase: "preparing" }) },
    { ...status({ phase: "uploading", bytes: 2 << 20, totalBytes: 8 << 20 }) },
    { ...status({ phase: "uploading" }), result: { sharedProject: link, skipped: [] } },
  ];
  serve((path, init) => {
    const request = `${init.method ?? "GET"} ${path.split("?")[0]}`;
    if (request === "GET /api/chat/sessions") return { sessions: [] };
    if (request === "GET /api/projects/p1/publish") return { signedIn: true, sharedProject: null };
    if (request === "POST /api/projects/p1/publish") return Response.json({ operationId: "i1" }, { status: 202 });
    if (request === "GET /api/operations/i1") return polls.shift();
  });
  const { workspace } = await render(null);
  await act(async () => workspace.dialogs.showCustomDialog({
    component: ShareProjectDialog,
    props: { project: { ...project, slug: "tiny-lm" }, remote: false, returnFocus: { current: null } },
    onConfirm: () => {},
    onCancel: "close",
  }));
  await settle();
  t.mock.timers.enable({ apis: ["setTimeout"] });
  await act(() => find('[role="dialog"] button', m.share_publish()).click());
  await settle();
  assertProgress(m.share_publish_preparing(), "", null);

  await act(() => t.mock.timers.tick(500));
  await settle();
  assertProgress(m.share_publish_uploading(), m.operation_bytes({ done: fmtBytes(2 << 20), total: fmtBytes(8 << 20) }), "25");

  await act(() => t.mock.timers.tick(500));
  await settle();
  assert.equal(find('[role="dialog"] input')?.value, link.url);
  assert.equal(find("[role=status]"), null);
});
