import { readdir, readFile } from "node:fs/promises";
import { dirname, extname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const uiDir = resolve(scriptDir, "..");
const sourceDir = resolve(uiDir, "src");
const sourceExtensions = new Set([".css", ".ts", ".tsx"]);
const arbitraryText = /\btext-\[[^\]\s"'`]+\]/g;
const arbitraryColor = /\b(?:accent|bg|border(?:-[trblxyse])?|caret|decoration|divide(?:-[xy])?|fill|outline|ring(?:-offset)?|stroke)-\[([^\]\s"'`]+)\]/g;
const colorOnlyArbitrary = /\b(?:from|placeholder|shadow|to|via)-\[[^\]\s"'`]+\]/g;
const rawColor = /#[\da-fA-F]{3,8}(?![\da-fA-F])|(?:color-mix|color|hsla?|lab|lch|light-dark|oklab|oklch|rgba?)\(/g;
const colorValue = /^(?:#|[a-z]+$|var\(--|(?:color-mix|color|hsla?|lab|lch|light-dark|oklab|oklch|rgba?)\()/i;
const svgPaintAttribute = /\b(?:fill|floodColor|lightingColor|stopColor|stroke)\s*=\s*["'][^"']*$/;

function location(source, index) {
  const before = source.slice(0, index);
  const lines = before.split("\n");
  return { line: lines.length, column: lines.at(-1).length + 1 };
}

function isSvgPaintValue(source, index) {
  return svgPaintAttribute.test(source.slice(Math.max(0, index - 120), index));
}

export function findStyleViolations(source, { allowRawColors = false } = {}) {
  const violations = [];
  const coveredRanges = [];

  for (const match of source.matchAll(arbitraryText)) {
    const value = match[0].slice(6, -1);
    const rule = colorValue.test(value) ? "arbitrary-color" : "arbitrary-text";
    violations.push({ rule, value: match[0], ...location(source, match.index) });
    coveredRanges.push([match.index, match.index + match[0].length]);
  }

  for (const match of source.matchAll(colorOnlyArbitrary)) {
    violations.push({ rule: "arbitrary-color", value: match[0], ...location(source, match.index) });
    coveredRanges.push([match.index, match.index + match[0].length]);
  }

  for (const match of source.matchAll(arbitraryColor)) {
    if (!colorValue.test(match[1])) continue;
    violations.push({ rule: "arbitrary-color", value: match[0], ...location(source, match.index) });
    coveredRanges.push([match.index, match.index + match[0].length]);
  }

  if (!allowRawColors) {
    for (const match of source.matchAll(rawColor)) {
      if (coveredRanges.some(([start, end]) => match.index >= start && match.index < end)) continue;
      if (isSvgPaintValue(source, match.index)) continue;
      violations.push({ rule: "raw-color", value: match[0], ...location(source, match.index) });
    }
  }

  return violations.sort((left, right) => left.line - right.line || left.column - right.column);
}

async function sourceFiles(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== "paraglide") files.push(...await sourceFiles(path));
    } else if (sourceExtensions.has(extname(entry.name))) {
      files.push(path);
    }
  }
  return files;
}

async function main() {
  const findings = [];
  for (const file of await sourceFiles(sourceDir)) {
    const source = await readFile(file, "utf8");
    const allowRawColors = file === resolve(sourceDir, "theme.css");
    for (const violation of findStyleViolations(source, { allowRawColors })) {
      findings.push({ file: relative(uiDir, file), ...violation });
    }
  }

  if (findings.length === 0) {
    console.log("Style token lint passed.");
    return;
  }

  const counts = Object.groupBy(findings, ({ rule }) => rule);
  console.error(`Style token lint failed: ${findings.length} violations`);
  console.error(`  arbitrary text values: ${counts["arbitrary-text"]?.length ?? 0}`);
  console.error(`  arbitrary color classes: ${counts["arbitrary-color"]?.length ?? 0}`);
  console.error(`  raw color values: ${counts["raw-color"]?.length ?? 0}`);
  for (const finding of findings) {
    console.error(`${finding.file}:${finding.line}:${finding.column} [${finding.rule}] ${finding.value}`);
  }
  process.exitCode = 1;
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) await main();
