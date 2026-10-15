// Runs ZCode's own runtime (`ORX_ZCODE_RUNTIME`, the desktop app's
// `resources/glm/zcode.cjs`) as `zcode app-server --stdio`, signed in the way
// `zcode -p` is.
//
// The app's app-server leaves the account to its host: the ZCode app sends the
// Coding Plan providers and each request's credentials. orx is not that host,
// so the runtime would find no Coding Plan model. The command-line paths start
// the same provider registry with `{standalone: …}`, which reads the sign-in
// from ZCode's own credential store; this loader passes that option to the
// app-server's call as well, in memory, without touching the app's files.
//
// The bundle is minified, so the functions are found by the names it
// registers for them. A build where they cannot be found runs unchanged (API-key
// providers still work) and says so on stderr, which orx reads back.
"use strict";

const fs = require("node:fs");
const Module = require("node:module");
const path = require("node:path");

const UNAVAILABLE = "orx: ZCode standalone sign-in unavailable";
const READY_LOG = "Worker Provider Registry \\u5DF2\\u5C31\\u7EEA";

function registeredName(source, label) {
  const match = source.match(new RegExp(`r\\(([\\w$]+),"${label}"\\)`));
  return match ? match[1] : undefined;
}

function escape(name) {
  return name.replace(/\$/g, "\\$");
}

/** `source` with the app-server signed in by itself: its provider registry
 * started standalone, and that registry's request credentials (rather than
 * the host's) given to each session — the two edits of the npm repack's
 * `app-server-standalone-auth`. Both apply, or neither. */
function standalone(source) {
  // Builds that already choose the sign-in themselves (the npm repack's
  // `ZCODE_APP_SERVER_AUTH_MODE`, standalone by default).
  if (source.includes("appServerRegistryOptions")) return { source, status: "native" };
  const unavailable = { source, status: "unavailable" };
  const start = registeredName(source, "startProcessProviderRegistryRuntime");
  const reporter = registeredName(source, "createCliProviderRefreshReporter");
  const ready = source.indexOf(READY_LOG);
  if (!start || !reporter || ready < 0) return unavailable;

  // 1. `<registry>=await <lease>({…,create:r(()=><start>(<env>),"create")` just
  //    before the registry reports ready.
  const call = source.lastIndexOf(`create:r(()=>${start}(`, ready);
  if (call < 0 || ready - call > 4000) return unavailable;
  const created = source
    .slice(call)
    .match(new RegExp(`^create:r\\(\\(\\)=>${escape(start)}\\(([\\w$]+)(?:,\\{\\})?\\),"create"\\)`));
  if (!created) return unavailable;
  const owner = source
    .slice(Math.max(0, call - 400), call)
    .match(/([\w$]+)=await [\w$]+\(\{[^{}]*$/);
  if (!owner) return unavailable;
  const registry = owner[1];

  // 2. The session factory's `...<f>(<g>(…),<alias>.runtime.registryService,
  //    <alias>.configuredDefaultModelSelection)`, where `<alias>` is that registry.
  const factories = [
    ...source.matchAll(
      /\.\.\.[\w$]+\([\w$]+\([\w$]+,[\w$]+\),([\w$]+)\.runtime\.registryService,\1\.configuredDefaultModelSelection\)/g,
    ),
  ];
  if (factories.length !== 1) return unavailable;
  const [factory] = factories;
  const alias = factory[1];
  if (alias !== registry && !new RegExp(`[\\s,;(]${escape(alias)}=${escape(registry)}[,;]`).test(source)) {
    return unavailable;
  }
  const spreadEnd = factory.index + factory[0].length;
  const factoryEnd = source.indexOf(`"createZCodeApp")`, spreadEnd);
  // A later property could override the credentials given here.
  if (factoryEnd < 0 || source.slice(spreadEnd, factoryEnd).includes("providerRuntimeHeadersPort")) {
    return unavailable;
  }
  if (factory.index < call) return unavailable;

  const options = `${created[1]},{standalone:${reporter}(process.stderr)}`;
  const headers = `,...${alias}.providerRuntimeHeadersPort?{providerRuntimeHeadersPort:${alias}.providerRuntimeHeadersPort}:{}`;
  const callEnd = call + created[0].length;
  return {
    source:
      source.slice(0, call) +
      `create:r(()=>${start}(${options}),"create")` +
      source.slice(callEnd, spreadEnd) +
      headers +
      source.slice(spreadEnd),
    status: "patched",
  };
}

module.exports = { standalone, UNAVAILABLE };

if (require.main === module) {
  const runtime = process.env.ORX_ZCODE_RUNTIME;
  if (!runtime) {
    process.stderr.write("orx: ORX_ZCODE_RUNTIME is not set\n");
    process.exit(2);
  }
  // Run the runtime as the main module, compiled from the patched source
  // here rather than through a loader hook, so nothing depends on how this
  // Node or Electron spells the file's path.
  const target = path.resolve(runtime);
  const result = standalone(fs.readFileSync(target, "utf8"));
  process.stderr.write(`orx: ZCode app-server sign-in: ${result.status}\n`);
  if (result.status === "unavailable") process.stderr.write(`${UNAVAILABLE}\n`);
  const main = new Module(target, null);
  main.id = ".";
  main.filename = target;
  main.paths = Module._nodeModulePaths(path.dirname(target));
  Module._cache[target] = main;
  process.mainModule = main;
  process.argv[1] = target;
  main._compile(result.source, target);
  main.loaded = true;
}
