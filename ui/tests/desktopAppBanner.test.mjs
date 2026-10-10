import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import ts from "typescript";

const require = createRequire(import.meta.url);
const RELEASES = "https://github.com/alphaXiv/OpenResearch/releases/latest";
const KEY = "desktop-app-banner-dismissed-on";

function storage(initial = {}) {
  const values = { ...initial };
  return { getItem: (key) => values[key] ?? null, setItem: (key, value) => { values[key] = value; }, values };
}

function mount({ platform = "MacIntel", stored = {}, desktop = false } = {}) {
  const source = readFileSync(new URL("../src/components/DesktopAppBanner.tsx", import.meta.url), "utf8");
  const code = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  const state = [];
  let cursor = 0;
  const react = {
    ...React,
    useEffect: () => {},
    useState: (initial) => {
      const index = cursor++;
      if (!(index in state)) state[index] = typeof initial === "function" ? initial() : initial;
      return [state[index], (value) => { state[index] = value; }];
    },
  };
  const mocks = {
    react,
    "lucide-react": { Monitor: "Monitor", X: "X" },
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
    "./ui": { ButtonLink: "a", IconButton: "IconButton" },
  };
  const localStorage = storage(stored);
  const window = desktop ? { __ORX_DESKTOP__: true, addEventListener() {}, removeEventListener() {} } : {};
  const exports = {};
  new Function("require", "exports", "window", "navigator", "localStorage", code)(
    (name) => mocks[name] ?? require(name), exports, window, { platform }, localStorage,
  );
  const render = () => { cursor = 0; return exports.DesktopAppBanner(); };
  return { render, localStorage };
}

function find(node, type) {
  if (!node || typeof node !== "object") return null;
  if (node.type === type) return node;
  for (const child of [node.props?.children].flat()) {
    const found = find(child, type);
    if (found) return found;
  }
  return null;
}

test("links the download for the viewer's platform", () => {
  const cases = [
    ["MacIntel", `${RELEASES}/download/OpenResearch.dmg`],
    ["Win32", `${RELEASES}/download/OpenResearch-Setup.exe`],
    ["Linux x86_64", `${RELEASES}/download/OpenResearch-x86_64.AppImage`],
    ["Linux aarch64", `${RELEASES}/download/OpenResearch-aarch64.AppImage`],
  ];
  for (const [platform, href] of cases) {
    const link = find(mount({ platform }).render(), "a");
    assert.equal(link.props.href, href, platform);
    assert.equal(link.props.target, undefined, platform);
  }
});

test("falls back to the release page in a new tab", () => {
  const link = find(mount({ platform: "FreeBSD amd64" }).render(), "a");
  assert.equal(link.props.href, RELEASES);
  assert.equal(link.props.target, "_blank");
});

test("dismissing hides the banner until the next calendar day", () => {
  const banner = mount();
  find(banner.render(), "IconButton").props.onClick();
  assert.equal(banner.localStorage.values[KEY], new Date().toDateString());
  assert.equal(banner.render(), null);

  assert.equal(mount({ stored: { [KEY]: new Date().toDateString() } }).render(), null);
  const yesterday = new Date(Date.now() - 24 * 60 * 60 * 1000).toDateString();
  assert.notEqual(mount({ stored: { [KEY]: yesterday } }).render(), null);
});

test("never renders inside the desktop app", () => {
  assert.equal(mount({ desktop: true }).render(), null);
});
