import assert from "node:assert/strict";
import test from "node:test";
import { hostForRun, upsertEngineHost, worstPerAlias } from "../src/engineHosts.ts";

const host = (alias, root, status) => ({ alias, root, status, class: "key", sinceMs: 1, nextRetryMs: null, cause: null });

test("updates replace only the connection with the same alias and root", () => {
  const hosts = [host("gpu-box", "/a", "live"), host("gpu-box", "/b", "needs_auth")];
  const updated = upsertEngineHost(hosts, host("gpu-box", "/a", "backoff"));
  assert.deepEqual(updated.map((item) => `${item.root}:${item.status}`).sort(), ["/a:backoff", "/b:needs_auth"]);
});

test("one row per alias shows its worst connection", () => {
  const worst = worstPerAlias([host("gpu-box", "/a", "live"), host("gpu-box", "/b", "needs_auth"), host("lab", "/c", "connecting"), host("lab", "/d", "idle")]);
  assert.equal(worst.get("gpu-box").status, "needs_auth");
  assert.equal(worst.get("lab").status, "connecting");
});

test("a run uses its own root's connection when its handle names one", () => {
  const hosts = [host("gpu-box", "/a", "live"), host("gpu-box", "/b", "needs_auth")];
  const run = (root) => ({ host: "gpu-box", backend: root ? { kind: "engine", handle: { type: "Ssh", host: "gpu-box", root } } : null });
  assert.equal(hostForRun(hosts, run("/a")).status, "live");
  assert.equal(hostForRun(hosts, run("/b")).status, "needs_auth");
  assert.equal(hostForRun(hosts, run(null)).status, "needs_auth");
  assert.equal(hostForRun(hosts, { host: null, backend: null }), undefined);
});
