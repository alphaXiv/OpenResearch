import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import ts from "typescript";

const require = createRequire(import.meta.url);
const code = ts.transpileModule(readFileSync(new URL("../src/components/LibraryView.tsx", import.meta.url), "utf8"), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;

class NotConnected extends Error {}
function render(account, folders, papers) {
  const events = [];
  const mocks = {
    react: { ...React, useEffect() {}, useRef: () => ({ current: null }), useState: (value) => [value, () => {}] },
    "@tanstack/react-query": { useInfiniteQuery: () => papers },
    "../api": { AlphaXivNotConnectedError: NotConnected },
    "../queries/client": { queryClient: { invalidateQueries: async () => {} }, workspaceKey: (...key) => key },
    "../queries/settings": { useOpenResearchAccount: () => account, useAlphaXivFolders: () => folders },
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
    "./OpenResearchAccount": { accountErrorMessage: () => "account outage" },
    "./ui": { Button: "button", Spinner: "spinner" },
  };
  const exports = {};
  new Function("require", "exports", "window", "Event", code)((name) => mocks[name] ?? require(name), exports, { dispatchEvent: (event) => events.push(event.type) }, Event);
  const nodes = (tree) => React.isValidElement(tree) ? [tree, ...React.Children.toArray(tree.props.children).flatMap(nodes)] : [];
  return { nodes: nodes(exports.LibraryView()), events };
}

const signedIn = { data: { loggedIn: true } };
const folder = { id: "f", name: "Reading", paperCount: 1 };
const page = { papers: [{ paperGroupId: "p", title: "Paper", authors: ["Author"], authorCount: 1, publishedAt: "2025-01-01", url: "https://www.alphaxiv.org/abs/p" }] };

test("library exposes folders, paper links, and pagination", () => {
  let loaded = 0;
  const { nodes } = render(signedIn, { data: [folder] }, { data: { pages: [page] }, hasNextPage: true, fetchNextPage: () => { loaded++; } });
  assert.ok(nodes.some((node) => node.type === "nav" && node.props["aria-label"] === "library_folders"));
  assert.equal(nodes.find((node) => node.type === "a").props.href, page.papers[0].url);
  nodes.find((node) => node.type === "button" && node.props.children === "library_more").props.onClick();
  assert.equal(loaded, 1);
});

test("signed-out and unconnected accounts offer the connection flow", () => {
  for (const [account, folders, label, event] of [
    [{ data: { loggedIn: false } }, {}, "account_sign_in_alphaxiv", "openresearch-sign-in"],
    [signedIn, { error: new NotConnected() }, "library_connect", "alphaxiv-connect"],
  ]) {
    const { nodes, events } = render(account, folders, {});
    const button = nodes.find((node) => node.type === "button");
    assert.equal(button.props.children, label);
    button.props.onClick();
    assert.deepEqual(events, [event]);
  }
});

test("long titles wrap and extra authors are shown", () => {
  const paper = { ...page.papers[0], title: "x".repeat(200), authors: ["A", "B", "C"], authorCount: 8 };
  const { nodes } = render(signedIn, { data: [folder] }, { data: { pages: [{ papers: [paper] }] } });
  assert.match(nodes.find((node) => node.type === "a").props.className, /break-words/);
  assert.ok(JSON.stringify(nodes.find((node) => node.type === "p").props.children).includes("library_more_authors"));
});

test("account outages show their reason", () => {
  const { nodes } = render({ isError: true, error: new Error("offline") }, {}, {});
  assert.ok(nodes.some((node) => node.type === "p" && node.props.children === "account outage"));
});
