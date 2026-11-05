import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const theme = readFileSync(new URL("../src/theme.css", import.meta.url), "utf8");

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
  const [r, g, b] = rgb(hex)
    .map((channel) => channel / 255)
    .map((channel) => (channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(first, second) {
  const [lighter, darker] = [luminance(first), luminance(second)].sort((a, b) => b - a);
  return (lighter + 0.05) / (darker + 0.05);
}

test("text selection meets contrast thresholds in both themes", () => {
  for (const { name, css } of themeTokens()) {
    const ratio = contrast(hexToken(css, "selection-text"), hexToken(css, "selection-background"));
    const boundary = contrast(hexToken(css, "base"), hexToken(css, "selection-background"));

    assert.ok(ratio >= 4.5, `${name} selected text contrast ${ratio.toFixed(2)} is below 4.5`);
    assert.ok(boundary >= 1.2, `${name} selection boundary contrast ${boundary.toFixed(2)} is below 1.2`);
  }
});

test("chat annotations stay distinguishable from active selection", () => {
  for (const { css } of themeTokens()) {
    assert.notEqual(hexToken(css, "selection-background"), hexToken(css, "chat-annotation-highlight"));
  }
});

test("editor selection tints the highlighted overlay and keeps code readable", () => {
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

test("selected syntax uses the shared contrast pair even for low-contrast tokens", () => {
  for (const { name, css } of themeTokens()) {
    const background = hexToken(css, "selection-background");
    assert.ok(contrast(hexToken(css, "syntax-comment"), background) < 4.5);
    assert.ok(contrast(hexToken(css, "selection-text"), background) >= 4.5, `${name} selected syntax must remain readable`);
  }
});
