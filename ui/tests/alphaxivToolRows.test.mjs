import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";
import * as bashCommand from "../src/bashCommand.ts";
import * as chatRecovery from "../src/chatRecovery.ts";
import * as chatRendering from "../src/chatRendering.ts";
import * as orxCommand from "../src/orxCommand.ts";

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/ChatPanel.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;
const stub = new Proxy({}, { get: (_, name) => (name === "__esModule" ? false : () => null) });
const messages = new Proxy({}, {
  get: (_, name) => (params) => params ? `${String(name)} ${JSON.stringify(params)}` : String(name),
});
let folders;
let titles = {};
const mocks = {
  "../queries/projects": new Proxy({}, { get: (_, name) => (name === "resolvePaperQuery" ? (id) => ({ id }) : stub[name]) }),
  "@tanstack/react-query": {
    ...require("@tanstack/react-query"),
    useQueries: ({ queries }) => queries.map(({ id }) => ({ data: titles[id] ? { paperId: id, title: titles[id] } : undefined })),
  },
  "../queries/settings": new Proxy({}, { get: (_, name) => (name === "useAlphaXivFolders" ? () => ({ data: folders }) : stub[name]) }),
  "../paraglide/messages.js": { m: messages },
  "../paraglide/runtime.js": { getLocale: () => "en" },
  "../i18n": { ltr: String, autoDir: String },
  "../bashCommand": bashCommand,
  "../chatRecovery": chatRecovery,
  "../chatRendering": chatRendering,
  "../queries/chatState": { LOCAL_PREFIX: "local-", SHELL_TOOL: "shell" },
  "../orxCommand": orxCommand,
  "../api": new Proxy({}, { get: (_, name) => (name === "fmtNumber" ? String : stub[name]) }),
  "./LitSourceLogo": { ...orxCommand, LitSourceLogo: ({ source }) => React.createElement("i", { "data-source": source }), paperUrl: (_, id) => `https://www.alphaxiv.org/abs/${id}` },
  "@tanstack/react-router": { useNavigate: () => () => {} },
  "./ui": new Proxy({}, {
    get: (_, name) => (name === "Button" ? ({ children }) => React.createElement("button", null, children) : stub[name]),
  }),
};
const exports = {};
new Function("require", "exports", compiled)(
  (name) => mocks[name] ?? (name.startsWith(".") ? stub : require(name)),
  exports,
);

const call = (tool, input, extra = {}) => ({
  id: `tool-${tool}`, type: "tool", tool, state: { status: "completed", input, ...extra },
});
const label = (part) => exports.spawnRowTitle(part);
const renderMessage = (parts, activePermissionId = null) => renderToStaticMarkup(React.createElement(exports.Message, {
  message: { id: "msg-1", role: "assistant", parts, createdAt: 0, completedAt: 1000 },
  activePermissionId,
  forkDisabled: false,
  branchDisabled: false,
  onFork: () => {},
  onSelectFork: () => {},
  onRecover: () => {},
}));

test("every harness's alphaXiv tool name reaches the same row", () => {
  const input = { paper_id_or_url: "2307.12307" };
  const expected = 'activity_alphaxiv_read_notes_on {"paper":"2307.12307"}';
  assert.equal(label(call("mcp__alphaxiv__get_paper_notes", input)), expected);
  assert.equal(label(call("orx_alphaxiv_get_paper_notes", input)), expected);
  assert.equal(label(call("alphaxiv:get_paper_notes", { arguments: input })), expected);
  assert.equal(label(call("orx_alphaxiv:get_paper_notes", { arguments: input })), expected);
  assert.equal(label(call("other", input, { title: "Tool: alphaxiv/get_paper_notes" })), expected);
  assert.equal(label(call("read", input, { title: "mcp__alphaxiv__get_paper_notes" })), expected);
});

test("alphaXiv rows name the paper, folder, query or researcher from the call", () => {
  assert.equal(label(call("mcp__alphaxiv__list_library", {})), "activity_alphaxiv_read_library");
  assert.equal(label(call("mcp__alphaxiv__save_papers_to_folder", { paper_ids_or_urls: ["2307.12307"] })), 'activity_alphaxiv_saved_paper {"paper":"2307.12307"}');
  assert.equal(label(call("mcp__alphaxiv__save_papers_to_folder", { paper_ids_or_urls: ["a", "b", "c"] })), 'activity_alphaxiv_saved_papers {"count":"3"}');
  assert.equal(label(call("mcp__alphaxiv__create_folder", { name: "Diffusion" })), 'activity_alphaxiv_created_folder {"name":"Diffusion"}');
  assert.equal(
    label(call("mcp__alphaxiv__answer_pdf_queries", { paper: "2307.12307", queries: "What datasets?" })),
    'activity_for_query {"activity":"activity_read_target {\\"target\\":\\"2307.12307\\"}","query":"What datasets?"}',
  );
  assert.equal(
    label(call("mcp__alphaxiv__read_files_from_github_repository", { githubUrl: "https://github.com/karpathy/nanoGPT.git", path: "/model.py" })),
    'activity_read_target {"target":"karpathy/nanoGPT/model.py"}',
  );
  assert.equal(
    label(call("mcp__alphaxiv__discover_papers", { keywords: ["GRPO"], question: "GRPO variants" })),
    'activity_for_query {"activity":"activity_searched_alphaxiv","query":"GRPO variants"}',
  );
  assert.equal(label(call("mcp__alphaxiv__resolve_researchers", { people: [{ name: "Yann LeCun" }] })), 'activity_alphaxiv_looked_up_researcher {"name":"Yann LeCun"}');
  assert.equal(label(call("mcp__alphaxiv__get_researcher", { researchers: ["A", "B"] })), 'activity_alphaxiv_looked_up_researchers {"count":"2"}');
  assert.equal(label(call("mcp__alphaxiv__get_researcher_papers", { researchers: ["Yann LeCun"] })), 'activity_alphaxiv_listed_papers_by {"name":"Yann LeCun"}');
  assert.equal(label(call("mcp__alphaxiv__rename_folder", { name: "x" })), "activity_used_alphaxiv");
});

test("commands and URLs that mention alphaXiv keep their own rows", () => {
  assert.equal(label(call("mcp__other__list_library", {})), "mcp__other__list_library");
  const curl = call("bash", { command: "curl https://alphaxiv.org/abs/1" }, { title: "curl https://alphaxiv.org/abs/1" });
  assert.doesNotMatch(label(curl), /alphaxiv_|used_alphaxiv/);
});

test("alphaXiv rows show the alphaXiv logo and link only a paper id", () => {
  const notes = renderMessage([call("mcp__alphaxiv__get_paper_notes", { paper_id_or_url: "2307.12307" })]);
  assert.match(notes, /data-source="alphaxiv"/);
  assert.match(notes, /href="https:\/\/www.alphaxiv.org\/abs\/2307.12307"/);
  const titled = renderMessage([call("mcp__alphaxiv__answer_pdf_queries", { paper: "Attention Is All You Need", queries: ["q"] })]);
  assert.match(titled, /data-source="alphaxiv"/);
  assert.doesNotMatch(titled, /href=/);
});

const approval = (tool, toolInput) => {
  const prompt = { id: "perm-1", type: "prompt", prompt: { kind: "permission", tool, toolInput } };
  return renderMessage([prompt], prompt.id).replaceAll("&quot;", '"');
};

test("an alphaXiv write's approval card names what it writes", () => {
  folders = undefined;
  assert.match(approval("mcp__alphaxiv__create_folder", { name: "Diffusion" }), /activity_alphaxiv_create_folder \{"name":"Diffusion"\}/);
  assert.match(approval("orx_alphaxiv_create_folder", { name: "Diffusion" }), /activity_alphaxiv_create_folder \{"name":"Diffusion"\}/);
});

test("the save approval lists every paper and names its folder once folders load", () => {
  const input = { paper_ids_or_urls: ["2307.12307", "2401.04088", "2209.10652"], folder_id: "f2" };
  folders = undefined;
  const loading = approval("mcp__alphaxiv__save_papers_to_folder", input);
  assert.match(loading, /activity_alphaxiv_save_papers \{"count":"3"\}/);
  for (const id of input.paper_ids_or_urls) assert.match(loading, new RegExp(`href="https://www.alphaxiv.org/abs/${id}"`));
  assert.equal(loading.match(/data-source="alphaxiv"/g)?.length, 3);

  folders = [{ id: "f1", name: "Want to read" }, { id: "f2", name: "Interpretability" }];
  assert.match(approval("mcp__alphaxiv__save_papers_to_folder", input), /activity_alphaxiv_save_papers_to \{"count":"3","folder":"Interpretability"\}/);
  assert.match(
    approval("mcp__alphaxiv__save_papers_to_folder", { ...input, folder_id: "gone" }),
    /activity_alphaxiv_save_papers \{"count":"3"\}/,
  );

  const single = approval("mcp__alphaxiv__save_papers_to_folder", { paper_ids_or_urls: ["2307.12307"] });
  assert.match(single, /activity_alphaxiv_save_a_paper</);
  assert.match(single, /<ul[^]*href="https:\/\/www.alphaxiv.org\/abs\/2307.12307"[^>]*>2307.12307<\/a>/);
});

test("the save approval links each paper by title once it resolves, keeping its id beside it", () => {
  folders = undefined;
  const input = { paper_ids_or_urls: ["2307.12307", "2401.04088"] };
  titles = { "2307.12307": "Robust Weighted Sum-Rate Maximization for Transmissive RIS Transmitter Enabled RSMA Networks" };
  const html = approval("mcp__alphaxiv__save_papers_to_folder", input);
  assert.match(html, /href="https:\/\/www.alphaxiv.org\/abs\/2307.12307"[^>]*>Robust Weighted Sum-Rate/);
  assert.match(html, /<span class="shrink-0 text-xs text-subtext">2307.12307<\/span>/);
  assert.match(html, /href="https:\/\/www.alphaxiv.org\/abs\/2401.04088"[^>]*>2401.04088<\/a>/);
  assert.doesNotMatch(html, /text-subtext">2401.04088/);

  folders = [{ id: "f2", name: "Interpretability" }];
  const single = { paper_ids_or_urls: ["2307.12307"], folder_id: "f2" };
  const titled = approval("mcp__alphaxiv__save_papers_to_folder", single);
  assert.match(titled, /activity_alphaxiv_save_a_paper_to \{"folder":"Interpretability"\}/);
  assert.match(titled, /<li[^]*>Robust Weighted Sum-Rate[^]*text-subtext">2307.12307</);
  titles = {};
  const pending = approval("mcp__alphaxiv__save_papers_to_folder", single);
  assert.match(pending, /activity_alphaxiv_save_a_paper_to \{"folder":"Interpretability"\}/);
  assert.match(pending, /<li[^]*>2307.12307<\/a>/);
});

test("the create-folder approval names its parent folder", () => {
  folders = [{ id: "f2", name: "Interpretability" }];
  assert.match(
    approval("mcp__alphaxiv__create_folder", { name: "SAEs", parent_folder_id: "f2" }),
    /activity_alphaxiv_create_folder_in \{"name":"SAEs","parent":"Interpretability"\}/,
  );
});
