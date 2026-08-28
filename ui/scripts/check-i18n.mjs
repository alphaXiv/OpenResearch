import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const uiDir = dirname(dirname(fileURLToPath(import.meta.url)));
const settings = await readJson(join(uiDir, "project.inlang/settings.json"));
const catalogs = new Map();
const errors = [];

for (const locale of settings.locales) {
  const file = join(uiDir, "messages", `${locale}.json`);
  try {
    catalogs.set(locale, await readJson(file));
  } catch (error) {
    errors.push(`${locale}: ${error instanceof Error ? error.message : String(error)}`);
  }
}

const base = catalogs.get(settings.baseLocale);
if (!base) {
  errors.push(`Base locale catalog is unavailable: ${settings.baseLocale}`);
} else {
  const baseKeys = messageKeys(base);
  for (const [locale, catalog] of catalogs) {
    const keys = messageKeys(catalog);
    const missing = baseKeys.filter((key) => !keys.includes(key));
    const unexpected = keys.filter((key) => !baseKeys.includes(key));
    const invalid = keys.filter(
      (key) => typeof catalog[key] !== "string" || catalog[key].trim() === "",
    );
    const placeholderMismatch = keys.filter(
      (key) =>
        typeof catalog[key] === "string" &&
        placeholders(catalog[key]).join(",") !== placeholders(base[key]).join(","),
    );
    const entities = keys.filter(
      (key) => typeof catalog[key] === "string" && /&(?:apos|quot|amp|lt|gt);/.test(catalog[key]),
    );
    if (missing.length) errors.push(`${locale}: missing keys: ${missing.join(", ")}`);
    if (unexpected.length) errors.push(`${locale}: unexpected keys: ${unexpected.join(", ")}`);
    if (invalid.length) errors.push(`${locale}: empty or non-string values: ${invalid.join(", ")}`);
    if (placeholderMismatch.length) errors.push(`${locale}: placeholder mismatch: ${placeholderMismatch.join(", ")}`);
    if (entities.length) errors.push(`${locale}: HTML entities must be decoded: ${entities.join(", ")}`);
  }
}

if (errors.length) {
  console.error(`Localization catalog lint failed:\n${errors.map((error) => `- ${error}`).join("\n")}`);
  process.exitCode = 1;
}

async function readJson(file) {
  try {
    return JSON.parse(await readFile(file, "utf8"));
  } catch (error) {
    throw new Error(`${file}: ${error instanceof Error ? error.message : String(error)}`);
  }
}

function messageKeys(catalog) {
  return Object.keys(catalog)
    .filter((key) => !key.startsWith("$"))
    .sort();
}

function placeholders(message) {
  if (typeof message !== "string") return [];
  return [...message.matchAll(/\{([A-Za-z][A-Za-z0-9_]*)\}/g)]
    .map((match) => match[1])
    .sort();
}
