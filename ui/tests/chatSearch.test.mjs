import assert from "node:assert/strict";
import test from "node:test";
import { InfiniteQueryObserver } from "@tanstack/react-query";
import { queryModules } from "./queryModules.mjs";
import { chatSearchSnippet } from "../src/chatSearch.ts";

const tick = () => new Promise(resolve => setTimeout(resolve, 0));
const page = (id, cursor = null) => ({ hits: [{ messageId: id }], cursor, indexing: false });

test("search aborts stale reads and isolates placeholders by chat and project", async (t) => {
  const requests = [];
  const { client, load } = queryModules({
    searchChatMessages: (params, cursor, signal) => new Promise(resolve => requests.push({ params, cursor, signal, resolve })),
  }, { nativeClient: true });
  client.setDefaultOptions({ queries: { ...client.getDefaultOptions().queries, gcTime: Infinity } });
  const options = load("chat").chatSearchQuery;
  const observer = new InfiniteQueryObserver(client, options({ q: "warmup", scope: "chat", sessionId: "one" }));
  const stop = observer.subscribe(() => {});
  t.after(() => { stop(); client.clear(); });
  requests[0].resolve(page("one"));
  await tick();
  observer.setOptions(options({ q: "optimizer", scope: "chat", sessionId: "one" }));
  assert.equal(observer.getCurrentResult().data.pages[0].hits[0].messageId, "one");
  assert.equal(observer.getCurrentResult().isPlaceholderData, true);
  observer.setOptions(options({ q: "", scope: "chat", sessionId: "one" }));
  assert.equal(observer.getCurrentResult().data, undefined);
  assert.equal(requests.length, 2);
  observer.setOptions(options({ q: "optimizer", scope: "chat", sessionId: "two" }));
  assert.equal(requests[1].signal.aborted, true);
  assert.equal(observer.getCurrentResult().data, undefined);
  observer.setOptions(options({ q: "optimizer", scope: "project", projectId: "p" }));
  assert.equal(requests[2].signal.aborted, true);
  requests.at(-1).resolve(page("project"));
  await tick();
  assert.equal(observer.getCurrentResult().data.pages[0].hits[0].messageId, "project");
  requests[1].resolve(page("stale"));
  await tick();
  assert.equal(observer.getCurrentResult().data.pages[0].hits[0].messageId, "project");
});

test("search forwards opaque cursors and closing cancels pagination", async (t) => {
  const requests = [];
  const { client, load } = queryModules({
    searchChatMessages: (params, cursor, signal) => new Promise(resolve => requests.push({ params, cursor, signal, resolve })),
  });
  t.after(() => client.clear());
  const observer = new InfiniteQueryObserver(client, load("chat").chatSearchQuery({ q: "warmup", scope: "all" }));
  const stop = observer.subscribe(() => {});
  requests[0].resolve(page("one", "opaque-cursor"));
  await tick();
  const next = observer.fetchNextPage();
  assert.equal(requests[1].cursor, "opaque-cursor");
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
