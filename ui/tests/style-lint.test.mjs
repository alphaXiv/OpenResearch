import assert from "node:assert/strict";
import test from "node:test";

import { findStyleViolations } from "../scripts/check-styles.mjs";

test("style lint rejects arbitrary typography and colors", () => {
  const source = 'className="text-[15px] text-[#def] bg-[red] bg-[var(--surface)] border-[0.5px] from-[red] shadow-[0_0_1px_red] divide-[tomato]"; const color = "#abc";';
  assert.deepEqual(
    findStyleViolations(source).map(({ rule, value }) => [rule, value]),
    [
      ["arbitrary-text", "text-[15px]"],
      ["arbitrary-color", "text-[#def]"],
      ["arbitrary-color", "bg-[red]"],
      ["arbitrary-color", "bg-[var(--surface)]"],
      ["arbitrary-color", "from-[red]"],
      ["arbitrary-color", "shadow-[0_0_1px_red]"],
      ["arbitrary-color", "divide-[tomato]"],
      ["raw-color", "#abc"],
    ],
  );
});

test("style lint allows theme aliases and theme color definitions", () => {
  const source = 'className="text-sm bg-surface border-[0.5px]"; --surface: oklch(0.9 0 0);';
  assert.deepEqual(findStyleViolations(source, { allowRawColors: true }), []);
});

test("style lint allows literal colors only in SVG paint attributes", () => {
  const source = '<svg fill="#123"><path stroke="rgb(1 2 3)" /><stop stopColor="#abcdef" /></svg>; const color = "#456";';
  assert.deepEqual(
    findStyleViolations(source).map(({ rule, value }) => [rule, value]),
    [["raw-color", "#456"]],
  );
});
