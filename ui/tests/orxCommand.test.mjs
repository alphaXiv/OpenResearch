import assert from "node:assert/strict";
import test from "node:test";
import {
  containsShellGlob,
  orxArgsMatch,
  orxArgv,
  orxArgvFromTokens,
  parseOrxLit,
  shellWords,
  shellWrapperBody,
  unwrapShellBody,
} from "../src/orxCommand.ts";

test("shell glob patterns are not literal file targets", () => {
  assert.equal(containsShellGlob(".openresearch/*.*"), true);
  assert.equal(containsShellGlob("src/file?.ts"), true);
  assert.equal(containsShellGlob("src/[ab].ts"), true);
  assert.equal(containsShellGlob("src/file.ts"), false);
});

test("quoted Codex argv is tokenized as a normal command", () => {
  assert.deepEqual(shellWords('"orx" "projects"'), ["orx", "projects"]);
  assert.deepEqual(
    shellWords('"orx" "discover" "embedding" "biology research agents" "--prioritize" "recency"'),
    ["orx", "discover", "embedding", "biology research agents", "--prioritize", "recency"],
  );
  assert.equal(orxArgsMatch('"orx" "projects"', "projects?"), true);
  assert.equal(orxArgsMatch('"orx" "runs" "d81084a9-589e-4c8f-9384-2c0003517216"', "runs?"), true);
  assert.equal(orxArgsMatch('which "orx" "projects"', "projects"), false);
  assert.equal(orxArgsMatch('ORX_DATA_DIR=/tmp "orx" "projects"', "projects"), true);
  assert.equal(orxArgsMatch('orx discover keyword "exp status"', "exp\\s+status"), false);
  assert.deepEqual(orxArgv('"orx" "logs" "d81084a9-589e-4c8f-9384-2c0003517216"'), [
    "logs",
    "d81084a9-589e-4c8f-9384-2c0003517216",
  ]);
  assert.deepEqual(orxArgv('"orx" "exp" "desc" "experiment-id" "--set" "note"'), [
    "exp",
    "desc",
    "experiment-id",
    "--set",
    "note",
  ]);
});

test("outer shell quotes do not consume quoted argv", () => {
  assert.equal(unwrapShellBody("'orx projects'"), "orx projects");
  assert.equal(unwrapShellBody('"orx" "projects"'), '"orx" "projects"');
  const serialized = String.raw`"orx discover keyword \"Scaling Laws for Neural Language Models\" --limit 5"`;
  const decoded = unwrapShellBody(serialized);
  assert.equal(decoded, 'orx discover keyword "Scaling Laws for Neural Language Models" --limit 5');
  assert.deepEqual(parseOrxLit(decoded), {
    kind: "discover",
    source: "alphaxiv",
    strategy: "keyword",
    query: "Scaling Laws for Neural Language Models",
  });
  const command = unwrapShellBody('"orx" "discover" "keyword" "biology agent benchmark"');
  assert.deepEqual(parseOrxLit(command), {
    kind: "discover",
    source: "alphaxiv",
    strategy: "keyword",
    query: "biology agent benchmark",
  });
});

test("tokenization stops at shell operators", () => {
  assert.deepEqual(shellWords('orx projects && echo ignored'), ["orx", "projects"]);
  assert.deepEqual(shellWords(String.raw`foo\
bar`), ["foobar"]);
  assert.deepEqual(shellWords(String.raw`\
`), []);
});

test("double-quoted backslashes survive before ordinary characters", () => {
  const command = String.raw`orx discover keyword "C:\models\papers"`;
  assert.deepEqual(shellWords(command), ["orx", "discover", "keyword", String.raw`C:\models\papers`]);
  assert.deepEqual(parseOrxLit(command), {
    kind: "discover",
    source: "alphaxiv",
    strategy: "keyword",
    query: String.raw`C:\models\papers`,
  });
  assert.deepEqual(
    shellWords(String.raw`orx discover keyword "a\\b \$x"`),
    ["orx", "discover", "keyword", String.raw`a\b $x`],
  );
});

test("structured argv preserves query boundaries and shell bodies", () => {
  const direct = ["env", "ORX_DATA_DIR=/tmp", "command", "/usr/local/bin/orx", "discover", "keyword", "multi word query"];
  assert.deepEqual(orxArgvFromTokens(direct), ["discover", "keyword", "multi word query"]);
  assert.deepEqual(parseOrxLit(direct), {
    kind: "discover",
    source: "alphaxiv",
    strategy: "keyword",
    query: "multi word query",
  });

  const body = 'orx discover openalex "protein folding" --limit 20';
  const wrappedBody = shellWrapperBody(["/bin/zsh", "-lc", body]);
  assert.equal(wrappedBody, body);
  assert.equal(shellWrapperBody(["/bin/zsh", "-c", body]), null);
  if (wrappedBody === null) assert.fail("expected a shell body");
  assert.deepEqual(parseOrxLit(wrappedBody), {
    kind: "discover",
    source: "openalex",
    strategy: "openalex",
    query: "protein folding",
  });
});

test("orx text outside command position is ignored", () => {
  for (const command of [
    'echo "orx discover keyword hidden"',
    'node -e \'console.log("orx discover keyword hidden")\'',
    'printf \'{"command":"orx discover keyword hidden"}\'',
    '# orx discover keyword hidden',
  ]) {
    assert.equal(parseOrxLit(command), null);
  }
});

test("paper discovery commands expose their strategy and query", () => {
  assert.deepEqual(
    parseOrxLit('"orx" "discover" "embedding" "biology research agents" "--published-after" "2024-01-01" "--prioritize" "recency"'),
    {
      kind: "discover",
      source: "alphaxiv",
      strategy: "embedding",
      query: "biology research agents",
    },
  );
  assert.deepEqual(
    parseOrxLit('orx discover keyword "biology agent benchmark" --prioritize=recency'),
    {
      kind: "discover",
      source: "alphaxiv",
      strategy: "keyword",
      query: "biology agent benchmark",
    },
  );
  assert.deepEqual(parseOrxLit('orx discover openalex "protein folding" --limit 20'), {
    kind: "discover",
    source: "openalex",
    strategy: "openalex",
    query: "protein folding",
  });
  assert.deepEqual(parseOrxLit('orx discover biorxiv "single-cell atlas"'), {
    kind: "discover",
    source: "biorxiv",
    strategy: "biorxiv",
    query: "single-cell atlas",
  });
});

test("paper parsing remains intact", () => {
  assert.deepEqual(parseOrxLit('"orx" "paper" "10.1101/2024.01.01.123456v2"'), {
    kind: "paper",
    source: "biorxiv",
    id: "10.1101/2024.01.01.123456v2",
  });
});
