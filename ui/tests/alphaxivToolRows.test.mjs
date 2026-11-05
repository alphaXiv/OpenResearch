import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { Message, spawnRowTitle } from "../src/components/ChatPanel.tsx";
import { m } from "../src/paraglide/messages.js";
import { render, serve, settle } from "./render.mjs";

/** Text as the reader sees it, without the bidi isolates around interpolated values. */
const plain = (text) => text.replace(/[⁦-⁩]/g, "");

const call = (tool, input, extra = {}) => ({
  id: `tool-${tool}`, type: "tool", tool, state: { status: "completed", input, ...extra },
});
const label = (part) => plain(spawnRowTitle(part));

/** Renders an assistant message against an alphaXiv account holding `folders` and papers titled by `titles`. */
async function renderMessage(parts, { activePermissionId = null, folders, titles = {} } = {}) {
  serve((path) => {
    if (path === "/api/account/openresearch") return { loggedIn: folders !== undefined, apiUrl: null, user: { id: "u" } };
    if (path === "/api/alphaxiv/folders") return folders;
    const paper = /^\/api\/papers\/resolve\?id=(.+)$/.exec(path)?.[1];
    if (paper) return titles[decodeURIComponent(paper)] ? { paper: { paperId: paper, title: titles[decodeURIComponent(paper)] } } : Response.json({ error: "unknown" }, { status: 404 });
  });
  const { container } = await render(createElement(Message, {
    message: { id: "msg-1", role: "assistant", parts, createdAt: 0, completedAt: 1000 },
    activePermissionId,
    forkDisabled: false,
    branchDisabled: false,
    onFork: () => {},
    onSelectFork: () => {},
    onRecover: () => {},
  }));
  await settle();
  return container;
}

const approval = (tool, toolInput, options) => {
  const prompt = { id: "perm-1", type: "prompt", prompt: { kind: "permission", tool, toolInput } };
  return renderMessage([prompt], { activePermissionId: prompt.id, ...options });
};
const logos = (container) => container.querySelectorAll('[role="img"][aria-label="alphaXiv"]').length;
const link = (container, id) => container.querySelector(`a[href="https://www.alphaxiv.org/abs/${id}"]`);

test("every harness's alphaXiv tool name reaches the same row", () => {
  const input = { paper_id_or_url: "2307.12307" };
  const expected = plain(m.activity_alphaxiv_read_notes_on({ paper: "2307.12307" }));
  assert.equal(label(call("mcp__alphaxiv__get_paper_notes", input)), expected);
  assert.equal(label(call("orx_alphaxiv_get_paper_notes", input)), expected);
  assert.equal(label(call("alphaxiv:get_paper_notes", { arguments: input })), expected);
  assert.equal(label(call("orx_alphaxiv:get_paper_notes", { arguments: input })), expected);
  assert.equal(label(call("other", input, { title: "Tool: alphaxiv/get_paper_notes" })), expected);
  assert.equal(label(call("read", input, { title: "mcp__alphaxiv__get_paper_notes" })), expected);
});

test("alphaXiv rows name the paper, folder, query or researcher from the call", () => {
  assert.equal(label(call("mcp__alphaxiv__list_library", {})), plain(m.activity_alphaxiv_read_library()));
  assert.equal(label(call("mcp__alphaxiv__save_papers_to_folder", { paper_ids_or_urls: ["2307.12307"] })), plain(m.activity_alphaxiv_saved_paper({ paper: "2307.12307" })));
  assert.equal(label(call("mcp__alphaxiv__save_papers_to_folder", { paper_ids_or_urls: ["a", "b", "c"] })), plain(m.activity_alphaxiv_saved_papers({ count: "3" })));
  assert.equal(label(call("mcp__alphaxiv__create_folder", { name: "Diffusion" })), plain(m.activity_alphaxiv_created_folder({ name: "Diffusion" })));
  assert.equal(
    label(call("mcp__alphaxiv__answer_pdf_queries", { paper: "2307.12307", queries: "What datasets?" })),
    plain(m.activity_for_query({ activity: m.activity_read_target({ target: "2307.12307" }), query: "What datasets?" })),
  );
  assert.equal(
    label(call("mcp__alphaxiv__read_files_from_github_repository", { githubUrl: "https://github.com/karpathy/nanoGPT.git", path: "/model.py" })),
    plain(m.activity_read_target({ target: "karpathy/nanoGPT/model.py" })),
  );
  assert.equal(
    label(call("mcp__alphaxiv__discover_papers", { keywords: ["GRPO"], question: "GRPO variants" })),
    plain(m.activity_for_query({ activity: m.activity_searched_alphaxiv(), query: "GRPO variants" })),
  );
  assert.equal(label(call("mcp__alphaxiv__resolve_researchers", { people: [{ name: "Yann LeCun" }] })), plain(m.activity_alphaxiv_looked_up_researcher({ name: "Yann LeCun" })));
  assert.equal(label(call("mcp__alphaxiv__get_researcher", { researchers: ["A", "B"] })), plain(m.activity_alphaxiv_looked_up_researchers({ count: "2" })));
  assert.equal(label(call("mcp__alphaxiv__get_researcher_papers", { researchers: ["Yann LeCun"] })), plain(m.activity_alphaxiv_listed_papers_by({ name: "Yann LeCun" })));
  assert.equal(label(call("mcp__alphaxiv__rename_folder", { name: "x" })), plain(m.activity_used_alphaxiv()));
});

test("commands and URLs that mention alphaXiv keep their own rows", () => {
  assert.equal(label(call("mcp__other__list_library", {})), "mcp__other__list_library");
  const curl = call("bash", { command: "curl https://alphaxiv.org/abs/1" }, { title: "curl https://alphaxiv.org/abs/1" });
  assert.notEqual(label(curl), plain(m.activity_used_alphaxiv()));
  assert.doesNotMatch(label(curl), /alphaXiv/);
});

test("alphaXiv rows show the alphaXiv logo and link only a paper id", async () => {
  const notes = await renderMessage([call("mcp__alphaxiv__get_paper_notes", { paper_id_or_url: "2307.12307" })]);
  assert.equal(logos(notes), 1);
  assert.ok(link(notes, "2307.12307"));
  const titled = await renderMessage([call("mcp__alphaxiv__answer_pdf_queries", { paper: "Attention Is All You Need", queries: ["q"] })]);
  assert.equal(logos(titled), 1);
  assert.equal(titled.querySelector("a[href]")?.outerHTML, undefined);
});

test("an alphaXiv write's approval card names what it writes", async () => {
  const expected = plain(m.activity_alphaxiv_create_folder({ name: "Diffusion" }));
  assert.ok(plain((await approval("mcp__alphaxiv__create_folder", { name: "Diffusion" })).textContent).includes(expected));
  assert.ok(plain((await approval("orx_alphaxiv_create_folder", { name: "Diffusion" })).textContent).includes(expected));
});

test("the save approval lists every paper and names its folder once folders load", async () => {
  const input = { paper_ids_or_urls: ["2307.12307", "2401.04088", "2209.10652"], folder_id: "f2" };
  const loading = await approval("mcp__alphaxiv__save_papers_to_folder", input);
  assert.ok(plain(loading.textContent).includes(plain(m.activity_alphaxiv_save_papers({ count: "3" }))));
  for (const id of input.paper_ids_or_urls) assert.ok(link(loading, id));
  assert.equal(logos(loading), 3);

  const folders = [{ id: "f1", name: "Want to read" }, { id: "f2", name: "Interpretability" }];
  const named = await approval("mcp__alphaxiv__save_papers_to_folder", input, { folders });
  assert.ok(plain(named.textContent).includes(plain(m.activity_alphaxiv_save_papers_to({ count: "3", folder: "Interpretability" }))));
  const gone = await approval("mcp__alphaxiv__save_papers_to_folder", { ...input, folder_id: "gone" }, { folders });
  assert.ok(plain(gone.textContent).includes(plain(m.activity_alphaxiv_save_papers({ count: "3" }))));

  const single = await approval("mcp__alphaxiv__save_papers_to_folder", { paper_ids_or_urls: ["2307.12307"] });
  assert.ok(plain(single.textContent).includes(plain(m.activity_alphaxiv_save_a_paper())));
  assert.equal(plain(link(single, "2307.12307").textContent), "2307.12307");
  assert.ok(link(single, "2307.12307").closest("li"));
});

test("the save approval links each paper by title once it resolves, keeping its id beside it", async () => {
  const titles = { "2307.12307": "Robust Weighted Sum-Rate Maximization for Transmissive RIS Transmitter Enabled RSMA Networks" };
  const html = await approval("mcp__alphaxiv__save_papers_to_folder", { paper_ids_or_urls: ["2307.12307", "2401.04088"] }, { titles });
  const [titled, pending] = html.querySelectorAll("li");
  assert.equal(link(titled, "2307.12307").textContent, titles["2307.12307"]);
  assert.ok(plain(titled.textContent).endsWith("2307.12307"));
  assert.equal(plain(link(pending, "2401.04088").textContent), "2401.04088");
  assert.equal(plain(pending.textContent).trim(), "2401.04088");

  const folders = [{ id: "f2", name: "Interpretability" }];
  const single = { paper_ids_or_urls: ["2307.12307"], folder_id: "f2" };
  const named = await approval("mcp__alphaxiv__save_papers_to_folder", single, { folders, titles });
  assert.ok(plain(named.textContent).includes(plain(m.activity_alphaxiv_save_a_paper_to({ folder: "Interpretability" }))));
  assert.equal(link(named, "2307.12307").textContent, titles["2307.12307"]);
  const unresolved = await approval("mcp__alphaxiv__save_papers_to_folder", single, { folders });
  assert.ok(plain(unresolved.textContent).includes(plain(m.activity_alphaxiv_save_a_paper_to({ folder: "Interpretability" }))));
  assert.equal(plain(link(unresolved, "2307.12307").textContent), "2307.12307");
});

test("the create-folder approval names its parent folder", async () => {
  const html = await approval("mcp__alphaxiv__create_folder", { name: "SAEs", parent_folder_id: "f2" }, { folders: [{ id: "f2", name: "Interpretability" }] });
  assert.ok(plain(html.textContent).includes(plain(m.activity_alphaxiv_create_folder_in({ name: "SAEs", parent: "Interpretability" }))));
});
