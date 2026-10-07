import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (file) => readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8");
const base = read("base.css");
const app = read("app.css");
const theme = read("theme.css");

/** Declarations of the one top-level rule whose selector starts a line. */
function declarations(styles, selector) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const matches = [...styles.matchAll(new RegExp(`^${escaped} \\{\\n([\\s\\S]*?)\\n\\}`, "gm"))];
  assert.equal(matches.length, 1, `expected exactly one rule for ${selector}`);
  return Object.fromEntries(
    matches[0][1]
      .split(";")
      .map((declaration) => declaration.trim())
      .filter(Boolean)
      .map((declaration) => declaration.split(/:\s*/, 2)),
  );
}

function themeTokens() {
  const light = theme.match(/:root \{([\s\S]*?)\n\}/)?.[1];
  const dark = theme.match(/:root\[data-theme="dark"\] \{([\s\S]*?)\n\}/)?.[1];
  assert.ok(light !== undefined, "missing light theme tokens");
  assert.ok(dark !== undefined, "missing dark theme tokens");
  return [
    { name: "light", css: light },
    { name: "dark", css: dark },
  ];
}

function hexToken(css, name) {
  const match = css.match(new RegExp(`--${name}:\\s*(#[0-9a-fA-F]{6})`));
  assert.ok(match, `missing --${name}`);
  return match[1];
}

/** The hex value a `var(--token)` declaration resolves to in one theme. */
function resolve(css, value) {
  const token = value.match(/^var\(--([a-z-]+)\)$/)?.[1];
  assert.ok(token, `expected a theme token, got ${value}`);
  return hexToken(css, token);
}

function luminance(hex) {
  const channels = hex
    .slice(1)
    .match(/.{2}/g)
    .map((channel) => Number.parseInt(channel, 16) / 255)
    .map((channel) => (channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4));
  assert.equal(channels.length, 3);
  return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
}

function contrast(first, second) {
  const [lighter, darker] = [luminance(first), luminance(second)].sort((a, b) => b - a);
  return (lighter + 0.05) / (darker + 0.05);
}

const globalSelection = () => declarations(base, "::selection");
const chatSelection = () => declarations(base, ".chat-thread-inner ::selection");

test("text selection uses a global explicit foreground and background", () => {
  assert.deepEqual(globalSelection(), { color: "var(--base)", background: "var(--primary)" });
});

test("text selection meets contrast thresholds in both themes", () => {
  for (const { name, css } of themeTokens()) {
    const ratio = contrast(hexToken(css, "base"), hexToken(css, "primary"));

    assert.ok(ratio >= 4.5, `${name} selected text contrast ${ratio.toFixed(2)} is below 4.5`);
    assert.ok(ratio >= 3, `${name} selection boundary contrast ${ratio.toFixed(2)} is below 3`);
  }
});

test("chat selection keeps readable text on the annotation highlight in both themes", () => {
  // A chat rule without its own colour inherits the global selection foreground.
  const { color = globalSelection().color, background } = chatSelection();

  for (const { name, css } of themeTokens()) {
    const ratio = contrast(resolve(css, color), resolve(css, background));

    assert.ok(ratio >= 4.5, `${name} chat selection contrast ${ratio.toFixed(2)} is below 4.5`);
  }
});

test("editor selection keeps the overlay textarea's text transparent", () => {
  assert.deepEqual(declarations(base, ".file-view-editarea::selection"), {
    color: "transparent",
    background: "var(--editor-selection)",
  });
});

test("shimmering labels select with the same foreground as the surrounding text", () => {
  const foreground = (color) => ({ color, "-webkit-text-fill-color": color });

  assert.deepEqual(declarations(app, ".tool-running-shimmer::selection"), foreground(globalSelection().color));
  assert.deepEqual(
    declarations(app, ".chat-thread-inner .tool-running-shimmer::selection"),
    foreground(chatSelection().color),
  );
});
