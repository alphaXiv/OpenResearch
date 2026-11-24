import assert from "node:assert/strict";
import test from "node:test";
import { bugReportUrl } from "../src/bugReport.ts";

const machine = { hostname: "h", os: "macos", arch: "aarch64", chip: "Apple M4 Pro", osName: "macOS 27.0.1", model: "MacBook Pro", cpuCount: 14, memBytes: null, gpus: [] };
const checkout = { commit: "a1ff70b2f920", base: "246bc28af1cf", pr: null };

const fields = (url) => Object.fromEntries(new URL(url).searchParams);

test("a team report is a bare Linear draft with the build footer", () => {
  const url = bugReportUrl({ linear: true, version: "0.2.30", screenshot: { copied: true }, machine, checkout });
  assert.ok(url.startsWith("linear://alphaxiv/team/ORX/new?"));
  const { description } = fields(url);
  assert.match(description, /^\(remember to paste your screenshot with (Cmd|Ctrl)\+V\)/);
  assert.match(description, /v0\.2\.30, macOS 27\.0\.1, MacBook Pro, Apple M4 Pro/);
  assert.match(description, /Dev Build based on `246bc28af1cf` from main\./);
  assert.doesNotMatch(description, /What happened/);
});

test("a GitHub report keeps the template and says what it couldn't read", () => {
  const url = bugReportUrl({
    linear: false,
    version: "canary-1234567",
    screenshot: { copied: false, error: null },
    machine: new Error("probe failed"),
    checkout: { ...checkout, base: null, pr: "https://github.com/alphaXiv/OpenResearch/pull/1" },
  });
  assert.ok(url.startsWith("https://github.com/alphaXiv/OpenResearch/issues/new?"));
  const { body, labels } = fields(url);
  assert.equal(labels, "bug");
  assert.match(body, /### What happened/);
  assert.match(body, /attach a screenshot/);
  assert.match(body, /canary-1234567, machine unknown \(probe failed\)/);
  assert.match(body, /`a1ff70b2f920`, fork point on main unknown \(https:\/\/github\.com\/alphaXiv\/OpenResearch\/pull\/1\)/);
});
