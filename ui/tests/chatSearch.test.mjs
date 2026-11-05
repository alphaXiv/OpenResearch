import assert from "node:assert/strict";
import test, { afterEach, beforeEach } from "node:test";
import { InfiniteQueryObserver } from "@tanstack/react-query";
import { chatSearchSnippet } from "../src/chatSearch.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { serve } from "./render.mjs";

let client, chatSearchQuery, requests;
beforeEach(() => {
  const workspace = createWorkspace("search");
  ({ queryClient: client, queries: { chatSearchQuery } } = workspace);
  requests = [];
  serve((path, init) => {
    const url = new URL(path, location.href);
    assert.equal(url.pathname, "/api/search");
    return new Promise((resolve) => requests.push({ params: Object.fromEntries(url.searchParams), signal: init.signal, resolve }));
  });
});
afterEach(() => client.clear());
const settle = () => new Promise((resolve) => setTimeout(resolve));
const page = (id, cursor = null) => ({ hits: [{ messageId: id }], cursor, indexing: false, requiresLongerQuery: false });
const first = (observer) => observer.getCurrentResult().data?.pages[0].hits[0].messageId;

test("search aborts stale reads and isolates placeholders by chat and project", async (t) => {
  const observer = new InfiniteQueryObserver(client, chatSearchQuery({ q: "warmup", scope: "chat", sessionId: "one" }));
  t.after(observer.subscribe(() => {}));
  await settle();
  assert.deepEqual(requests[0].params, { q: "warmup", scope: "chat", sessionId: "one" });
  requests[0].resolve(page("one"));
  await settle();

  observer.setOptions(chatSearchQuery({ q: "optimizer", scope: "chat", sessionId: "one" }));
  assert.equal(first(observer), "one", "the chat's previous results stay up while the next query loads");
  assert.equal(observer.getCurrentResult().isPlaceholderData, true);
  observer.setOptions(chatSearchQuery({ q: "", scope: "chat", sessionId: "one" }));
  assert.equal(first(observer), undefined);
  await settle();
  assert.equal(requests.length, 2, "an empty query asks the server nothing");

  observer.setOptions(chatSearchQuery({ q: "optimizer", scope: "chat", sessionId: "two" }));
  assert.equal(requests[1].signal.aborted, true);
  assert.equal(first(observer), undefined, "another chat never shows this chat's results");
  observer.setOptions(chatSearchQuery({ q: "optimizer", scope: "project", projectId: "p" }));
  await settle();
  assert.equal(requests[2].signal.aborted, true);
  assert.deepEqual(requests.at(-1).params, { q: "optimizer", scope: "project", projectId: "p" });
  requests.at(-1).resolve(page("project"));
  await settle();
  assert.equal(first(observer), "project");
  requests[1].resolve(page("stale"));
  await settle();
  assert.equal(first(observer), "project");
});

test("search forwards opaque cursors and closing cancels pagination", async () => {
  const observer = new InfiniteQueryObserver(client, chatSearchQuery({ q: "warmup", scope: "all" }));
  const stop = observer.subscribe(() => {});
  await settle();
  requests[0].resolve(page("one", "opaque-cursor"));
  await settle();
  const next = observer.fetchNextPage();
  await settle();
  assert.equal(requests[1].params.cursor, "opaque-cursor");
  stop();
  assert.equal(requests[1].signal.aborted, true);
  requests[1].resolve(page("two"));
  await next;
  assert.equal(observer.getCurrentResult().data.pages.length, 1);
});

test("result snippets keep visible Markdown text, code, and image descriptions", () => {
  assert.equal(chatSearchSnippet("The **warmup** [schedule](https://example.test) uses `bf16`.\n\n![loss curve](plot.png)"), "The warmup schedule uses bf16. loss curve");
  assert.equal(chatSearchSnippet("```python\noptimizer.step()\n```\n\n<script>hidden</script>"), "optimizer.step()");
});
