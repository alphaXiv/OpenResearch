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

const rgb = (hex) => hex.slice(1).match(/.{2}/g).map((channel) => Number.parseInt(channel, 16));

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

test("text selection uses a global explicit foreground and background", () => {
  assert.deepEqual(globalSelection(), { color: "var(--selection-text)", background: "var(--selection-background)" });
});

test("text selection meets contrast thresholds in both themes", () => {
  for (const { name, css } of themeTokens()) {
    const ratio = contrast(hexToken(css, "selection-text"), hexToken(css, "selection-background"));
    const boundary = contrast(hexToken(css, "base"), hexToken(css, "selection-background"));

    assert.ok(ratio >= 4.5, `${name} selected text contrast ${ratio.toFixed(2)} is below 4.5`);
    assert.ok(boundary >= 1.2, `${name} selection boundary contrast ${boundary.toFixed(2)} is below 1.2`);
  }
});

test("chat annotations stay separate from active selection", () => {
  assert.ok(!base.includes(".chat-thread-inner ::selection"));
  for (const { css } of themeTokens()) {
    assert.notEqual(hexToken(css, "selection-background"), hexToken(css, "chat-annotation-highlight"));
  }
});

test("editor selection tints the highlighted overlay instead of redrawing its text", () => {
  assert.deepEqual(declarations(base, ".file-view-editarea::selection"), {
    color: "transparent",
    background: "var(--editor-selection)",
  });
  for (const { name, css } of themeTokens()) {
    const tint = css.match(/--editor-selection: rgb\((\d+) (\d+) (\d+) \/ (\d+)%\)/);
    assert.ok(tint, `${name} editor selection must be a translucent rgb() (no color-mix on macOS 11 WebKit)`);
    const veil = (hex) =>
      `#${rgb(hex)
        .map((channel, i) => Math.round(channel + (Number(tint[i + 1]) - channel) * (Number(tint[4]) / 100)))
        .map((channel) => channel.toString(16).padStart(2, "0"))
        .join("")}`;
    const background = hexToken(css, "base");
    const selected = veil(background);
    const boundary = contrast(background, selected);
    assert.ok(boundary >= 1.2, `${name} editor selection boundary contrast ${boundary.toFixed(2)} is below 1.2`);
    const text = contrast(veil(hexToken(css, "syntax-text")), selected);
    assert.ok(text >= 4.5, `${name} selected code contrast ${text.toFixed(2)} is below 4.5`);
    const comment = hexToken(css, "syntax-comment");
    const ratio = contrast(veil(comment), selected) / contrast(comment, background);
    assert.ok(ratio >= 0.8, `${name} selection costs comments ${((1 - ratio) * 100).toFixed(0)}% of their contrast`);
  }
});

test("shimmering labels select with the same foreground as the surrounding text", () => {
  const foreground = (color) => ({ color, "-webkit-text-fill-color": color });

  assert.deepEqual(declarations(app, ".tool-running-shimmer::selection"), foreground(globalSelection().color));
});

test("diff selections use the readable shared foreground and background", () => {
  assert.deepEqual(declarations(app, ".openresearch-diff ::selection"), {
    color: "var(--selection-text)",
    background: "var(--code-selection)",
  });
  const diff = read("components/GitDiff.tsx");
  assert.ok(diff.includes("--diff-selection-text-color:var(--selection-text)"));
  assert.ok(!diff.includes("--diff-selection-text-color:var(--primary)"));
  assert.ok(theme.includes("--code-selection: var(--selection-background)"));
});

test("selected syntax uses the shared contrast pair even for low-contrast tokens", () => {
  assert.ok(!base.includes("currentColor"));
  assert.ok(!base.includes("code ::selection"));
  for (const { name, css } of themeTokens()) {
    const background = hexToken(css, "selection-background");
    const selected = hexToken(css, "selection-text");
    assert.ok(contrast(hexToken(css, "syntax-comment"), background) < 4.5);
    assert.ok(contrast(selected, background) >= 4.5, `${name} selected syntax must remain readable`);
  }
});
