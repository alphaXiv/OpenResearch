import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { act } from "react";
import { renderChat } from "./chat.mjs";
import { clients, find, navigate, settle } from "./render.mjs";

const flagged = JSON.parse(readFileSync(new URL("./fixtures/runMonitor/flagged.json", import.meta.url), "utf8"));
const { evidence } = flagged.flag;
const run = {
  id: "r1", experimentId: "e1", projectId: "p", status: "running", createdAt: 1, startedAt: 1, updatedAt: 10, contact: null, cancelling: false, host: null, backend: { kind: "local_job" },
  monitor: { tone: "flag", whole: 0.25, headline: null, sentence: flagged.sentence, bindings: { loss: "NaN" }, signals: flagged.view.signals, etaMs: null, flag: { id: "f1", state: "waiting", mode: "copilot" }, rev: flagged.rev, generation: flagged.generation, asOf: flagged.asOf },
};
const experiment = { id: "e1", projectId: "p", slug: "lr", createdAt: 1, updatedAt: 1 };
const pane = (value) => `/projects/p/tasks/s?pane=${encodeURIComponent(JSON.stringify(value))}`;
const terminal = pane({ kind: "experiment", experimentId: "e1", view: "terminal", runId: "r1" });

/** Chat s with run r1 flagged above its composer; every read of r1's log lands in `starts`. */
async function page(experiments) {
  const starts = [];
  await renderChat("/projects/p/tasks/s", {
    routes: {
      "GET /api/projects/p/runs": () => ({ runs: [run] }),
      "GET /api/projects/p/experiments": () => ({ experiments: experiments() }),
      "GET /api/chat/shown-runs": () => [{ sessionId: "s", run, experimentName: "lr", experimentSlug: "lr", runNo: 1, runCount: 1 }],
      "GET /api/runs/r1/monitor": () => flagged,
      "GET /api/compute/hosts": () => [],
      "GET /api/runs/r1/log": (init, path) => {
        const start = Number(new URL(path, location.href).searchParams.get("start"));
        starts.push(start);
        return { kind: "data", ident: evidence.ident, start, data: Buffer.from("line\n").toString("base64"), len: 10_000, eof: true, kept: [[0, 10_000]] };
      },
    },
  });
  const openEvidence = async () => {
    await act(async () => find("button", String(evidence.from)).click());
    await settle();
  };
  return { starts, openEvidence };
}

test("evidence opens its run's log at the cited bytes once; going elsewhere and back opens it as usual", async () => {
  const { starts, openEvidence } = await page(() => [experiment]);
  await openEvidence();
  assert.equal(starts[0], evidence.byteFrom);

  const seen = starts.length;
  await navigate(pane({ kind: "home", view: "files" }));
  await navigate(terminal);
  assert.ok(starts.length > seen, "the log was read again");
  assert.ok(!starts.slice(seen).includes(evidence.byteFrom), JSON.stringify(starts));
});

test("a log position its open never used is dropped by the next navigation, even back to its log", async () => {
  let experiments = [];
  const { starts, openEvidence } = await page(() => experiments);
  await openEvidence();
  assert.equal(starts.length, 0, "with no experiment there is no log to open");

  await navigate(pane({ kind: "home", view: "files" }));
  experiments = [experiment];
  await act(async () => {
    for (const client of clients) {
      for (const query of client.getQueryCache().findAll()) if (query.queryKey[2] === "listExperiments") client.setQueryData(query.queryKey, experiments);
    }
  });
  await navigate(terminal);
  assert.ok(starts.length > 0);
  assert.ok(!starts.includes(evidence.byteFrom), JSON.stringify(starts));
});

test("evidence on the log already open moves it to the cited bytes", async () => {
  const { starts, openEvidence } = await page(() => [experiment]);
  await navigate(terminal);
  const seen = starts.length;
  assert.ok(seen > 0, "the log is open");
  await openEvidence();
  assert.ok(starts.slice(seen).includes(evidence.byteFrom), JSON.stringify(starts));
});

test("what a look flagged while the flag was open is shown with the flag's evidence", async () => {
  flagged.flag.noted = [{ at: flagged.flag.at + 60_000, text: "Checkpoint write failed at step **450**.", lookId: "7" }];
  try {
    await page(() => [experiment]);
    await settle();
    const said = document.body.textContent.replace(/[⁦-⁩]/g, "");
    assert.match(said, /Also noted at .+: Checkpoint write failed at step 450\./, said.slice(0, 2000));
  } finally {
    delete flagged.flag.noted;
  }
});
