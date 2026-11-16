import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { Message } from "../src/components/ChatPanel.tsx";
import { discoveredPapers, headlineMetric, linkedPapers, paperHeader } from "../src/researchActivity.ts";
import { render, serve, settle } from "./render.mjs";

/** Text as the reader sees it, without the bidi isolates around interpolated values. */
const plain = (text) => text.replace(/[⁦-⁩]/g, "").replace(/\s+/g, " ").trim();

let ids = 0;
const bash = (command, output, status = "completed") => ({
  id: `tool-${ids++}`, type: "tool", tool: "Bash", state: { status, input: { command }, output },
});
const text = (value, phase) => ({ id: `text-${ids++}`, type: "text", text: value, ...(phase ? { phase } : {}) });

const DISCOVERED = `[
  {
    "source": "alphaxiv",
    "id": "2305.07759",
    "title": "TinyStories: How Small Can Language Models Be\\n  and Still Speak Coherent English?",
    "abstract": "We introduce TinyStories…",
    "publicationDate": "2023-05-12T00:00:00.000Z",
    "votes": 40
  },
  {
    "source": "alphaxiv",
    "id": "2203.15556",
    "title": "Training Compute-Optimal Large Lang`;

async function renderTurn(parts, titles = {}) {
  serve((path) => {
    const paper = /^\/api\/papers\/resolve\?id=(.+)$/.exec(path)?.[1];
    if (paper) {
      const resolved = titles[decodeURIComponent(paper)];
      return resolved ? { paper: { paperId: decodeURIComponent(paper), ...resolved } } : Response.json({ error: "unknown" }, { status: 404 });
    }
  });
  const { container } = await render(createElement(Message, {
    message: { id: "msg-1", role: "assistant", parts, createdAt: 0, completedAt: 192_000 },
    activePermissionId: null,
    forkDisabled: false,
    branchDisabled: false,
    onFork: () => {},
    onSelectFork: () => {},
    onRecover: () => {},
  }));
  await settle();
  return container;
}

/** A card's lines as the reader sees them: its title, then what it says about it. */
const cardLines = (card) => [...card.querySelector(":scope > .min-w-0").children].map((line) => plain(line.textContent));

const openWork = async (container) => {
  await act(async () => container.querySelector("[data-work-toggle]").click());
  await settle();
};

test("discovered papers survive output a harness cut short", () => {
  const papers = discoveredPapers([DISCOVERED]);
  assert.deepEqual(papers.get("2305.07759"), { title: "TinyStories: How Small Can Language Models Be and Still Speak Coherent English?", year: 2023 });
  assert.equal(papers.has("2203.15556"), false);
});

test("an OpenAlex, bioRxiv or PubMed header names the paper and its authors", () => {
  assert.deepEqual(
    paperHeader("# Scaling Laws for Neural Language Models\nJared Kaplan, Sam McCandlish, Tom Henighan\n2020-01-23 · 4000 citations\nDOI: x"),
    { title: "Scaling Laws for Neural Language Models", authors: ["Jared Kaplan", "Sam McCandlish", "Tom Henighan"], year: 2020 },
  );
  assert.deepEqual(paperHeader("# A preprint\nSmith, J.; Doe, A.\n2024-02-01 · neuroscience").authors, ["Smith, J.", "Doe, A."]);
  assert.equal(paperHeader("alphaXiv: https://www.alphaxiv.org/abs/2305.07759\n\n## Report"), null);
});

test("a run card plots validation loss before training metrics", () => {
  const metric = (name) => ({ name, first: { step: 0, value: 3 }, last: { step: 9, value: 1 }, series: [{ step: 0, value: 3 }, { step: 9, value: 1 }] });
  assert.equal(headlineMetric([metric("train/lr"), metric("train/loss"), metric("val/bpb")]).name, "val/bpb");
  assert.equal(headlineMetric([metric("tokens_per_sec"), metric("train/loss")]).name, "train/loss");
  assert.equal(headlineMetric([]), null);
});

test("a literature turn reads as papers on a shelf under a closed survey of its searches", async () => {
  const container = await renderTurn([
    text("Checking the literature first.", "commentary"),
    bash("orx discover keyword \"tiny stories small models\"", DISCOVERED),
    bash("orx discover embedding \"compute optimal training\" --limit 5; orx discover keyword \"Chinchilla\"", "[]"),
    bash("orx paper 2305.07759", "alphaXiv: https://www.alphaxiv.org/abs/2305.07759\n\n## Report"),
    bash("orx paper 2203.15556", "alphaXiv: https://www.alphaxiv.org/abs/2203.15556\n\n## Report"),
    text("Insufficient pretraining is the leading bottleneck.", "final_answer"),
  ], { "2203.15556": { title: "Training Compute-Optimal\n  Large Language Models", authors: ["Jordan Hoffmann", "Sebastian Borgeaud", "Arthur Mensch"], publishedAt: Date.UTC(2022, 2, 29) } });

  assert.match(plain(container.querySelector("[data-work-toggle]").textContent), /^2 papers · 3m 12s$/);
  await openWork(container);

  assert.deepEqual([...container.querySelectorAll(".paper-card")].map(cardLines), [
    ["TinyStories: How Small Can Language Models Be and Still Speak Coherent English?", "2023"],
    ["Training Compute-Optimal Large Language Models", "Hoffmann et al. · 2022"],
  ]);
  assert.equal(container.querySelector(".paper-card").getAttribute("href"), "https://www.alphaxiv.org/abs/2305.07759");
  const readCall = [...container.querySelectorAll(".paper-card [data-part-id]")].map((anchor) => anchor.getAttribute("data-part-id"));
  assert.equal(readCall.length, 2, "chat search can scroll to each paper read");
  const survey = [...container.querySelectorAll(".tool-group-summary")].map((summary) => plain(summary.textContent));
  assert.ok(survey.includes("Surveyed the literature · 3 searches"), survey.join(" | "));
});

test("code work folds into one quiet line naming the file", async () => {
  const container = await renderTurn([
    text("Looking at the trainer.", "commentary"),
    { id: "read-1", type: "tool", tool: "Read", state: { status: "completed", input: { file_path: "/repo/base_train.py" } } },
    { id: "edit-1", type: "tool", tool: "Edit", state: { status: "completed", input: { file_path: "/repo/base_train.py" } } },
    text("Done.", "final_answer"),
  ]);
  assert.match(plain(container.querySelector("[data-work-toggle]").textContent), /^Worked for 3m 12s$/);
  await openWork(container);
  const summary = plain(container.querySelector(".tool-group-summary").textContent);
  assert.equal(summary, "Read and edited base_train.py");
});

test("a report written to the artifacts directory shows as a card with its title", async () => {
  const content = "# Nanochat bottleneck diagnosis\n\nPretraining is short.\n";
  const container = await renderTurn([
    text("Writing it up.", "commentary"),
    { id: "write-1", type: "tool", tool: "Write", state: { status: "completed", input: { file_path: "/repo/artifacts/diagnosis.md", content } } },
    text("See the report.", "final_answer"),
  ]);
  assert.match(plain(container.querySelector("[data-work-toggle]").textContent), /^1 report · 3m 12s$/);
  await openWork(container);
  assert.deepEqual(cardLines(container.querySelector(".report-card")), ["Nanochat bottleneck diagnosis", "diagnosis.md · 3 lines"]);
});

test("an answer's paper links name each paper once, with the pages it cites", () => {
  const answer = [
    "Tokens matter ([Chinchilla, p. 8](https://www.alphaxiv.org/pdf/2203.15556?page=8)), see also",
    "[p. 1](https://www.alphaxiv.org/pdf/2203.15556v2?page=1) and [LIMA](https://arxiv.org/abs/2305.11206).",
    "Biology: [Smith](https://doi.org/10.1101/2024.01.01.123456), [Doe](https://pubmed.ncbi.nlm.nih.gov/38012345/),",
    "[Roe](https://openalex.org/W4389). A broken link: [x](https://doi.org/10.1000/example%).",
  ].join(" ");
  assert.deepEqual(linkedPapers(answer), [
    { source: "alphaxiv", id: "2203.15556", pages: [8, 1] },
    { source: "alphaxiv", id: "2305.11206", pages: [] },
    { source: "biorxiv", id: "10.1101/2024.01.01.123456", pages: [] },
    { source: "pubmed", id: "38012345", pages: [] },
    { source: "openalex", id: "W4389", pages: [] },
  ]);
});

test("the papers an answer cites show under it, marking those the turn never read", async () => {
  const container = await renderTurn([
    text("Reading the papers.", "commentary"),
    bash("orx paper 2203.15556", "alphaXiv: https://www.alphaxiv.org/abs/2203.15556\n\n## Report"),
    text("Train longer ([Chinchilla, p. 8](https://www.alphaxiv.org/pdf/2203.15556?page=8); [p. 1](https://www.alphaxiv.org/pdf/2203.15556?page=1)). Small models can be coherent ([TinyStories](https://www.alphaxiv.org/abs/2305.07759)).", "final_answer"),
  ], {
    "2203.15556": { title: "Training Compute-Optimal Large Language Models", authors: ["Jordan Hoffmann", "A B", "C D"], publishedAt: Date.UTC(2022, 2, 29) },
    "2305.07759": { title: "TinyStories", authors: ["Ronen Eldan", "Yuanzhi Li"], publishedAt: Date.UTC(2023, 4, 12) },
  });
  const sources = container.querySelector('section[aria-label="Sources"]');
  assert.deepEqual([...sources.querySelectorAll(".paper-card")].map(cardLines), [
    ["Training Compute-Optimal Large Language Models", "Hoffmann et al. · 2022 · p. 1, 8"],
    ["TinyStories", "Eldan and Li · 2023 · not read"],
  ]);
});

test("papers read in a shell loop each land on the shelf and count as read", async () => {
  const container = await renderTurn([
    text("Reading both.", "commentary"),
    bash("for p in 2203.15556 2305.07759; do echo \"=== $p\"; orx paper $p 2>&1 | head -c 6000; done", "=== 2203.15556\n…\n=== 2305.07759\n…"),
    text("See [Chinchilla](https://www.alphaxiv.org/abs/2203.15556) and [TinyStories](https://www.alphaxiv.org/abs/2305.07759).", "final_answer"),
  ], {
    "2203.15556": { title: "Training Compute-Optimal Large Language Models" },
    "2305.07759": { title: "TinyStories" },
  });
  assert.match(plain(container.querySelector("[data-work-toggle]").textContent), /^2 papers · /);
  const sources = container.querySelector('section[aria-label="Sources"]');
  assert.deepEqual([...sources.querySelectorAll(".paper-card")].map(cardLines), [
    ["Training Compute-Optimal Large Language Models", "2203.15556"],
    ["TinyStories", "2305.07759"],
  ]);
});

test("loading one of orx's guides says what it prepares the agent to do", async () => {
  const container = await renderTurn([
    { id: "skill-1", type: "tool", tool: "Skill", state: { status: "completed", input: { skill: "orx-create" } } },
    text("Started.", "final_answer"),
  ]);
  await openWork(container);
  assert.equal(plain(container.querySelector(".tool-row").textContent), "Prepared to start an experiment");
});

test("a paper read by URL counts as read where the answer cites it by id", async () => {
  const container = await renderTurn([
    bash("orx paper https://arxiv.org/abs/2305.07759", "alphaXiv: https://www.alphaxiv.org/abs/2305.07759\n\n## Report"),
    text("Small models can be coherent ([TinyStories](https://www.alphaxiv.org/abs/2305.07759)).", "final_answer"),
  ], { "2305.07759": { title: "TinyStories" } });
  const sources = container.querySelector('section[aria-label="Sources"]');
  assert.deepEqual([...sources.querySelectorAll(".paper-card")].map(cardLines), [["TinyStories", "2305.07759"]]);
});

test("reading an existing report is not counted as writing one", async () => {
  const container = await renderTurn([
    { id: "read-report", type: "tool", tool: "Read", state: { status: "completed", input: { file_path: "/repo/artifacts/diagnosis.md" } } },
    text("Done.", "final_answer"),
  ]);
  assert.match(plain(container.querySelector("[data-work-toggle]").textContent), /^Worked for /);
});

test("saving a paper to an alphaXiv folder does not count as reading it", async () => {
  const container = await renderTurn([
    { id: "save-1", type: "tool", tool: "mcp__alphaxiv__save_papers_to_folder", state: { status: "completed", input: { paper_ids_or_urls: ["2305.07759"] } } },
    text("Saved [TinyStories](https://www.alphaxiv.org/abs/2305.07759).", "final_answer"),
  ], { "2305.07759": { title: "TinyStories" } });
  assert.equal(container.querySelector(".paper-shelf"), null);
  const sources = container.querySelector('section[aria-label="Sources"]');
  assert.deepEqual([...sources.querySelectorAll(".paper-card")].map(cardLines), [["TinyStories", "not read"]]);
});
