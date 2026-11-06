import type { ReactElement } from "react";

const MD_RE = /\.(md|mdx|markdown)$/i;
const LATEX_RE = /\.tex$/i;
const HTML_RE = /\.html?$/i;
const CSV_RE = /\.[ct]sv$/i;
const IMAGE_RE = /\.(apng|avif|bmp|gif|heic|heif|ico|jpe?g|jfif|jxl|pbm|pgm|png|pnm|ppm|svg|tiff?|webp)$/i;

export function isImageFile(name: string): boolean {
  return IMAGE_RE.test(name);
}

export function isMarkdownFile(name: string): boolean {
  return MD_RE.test(name);
}

export function isLatexFile(name: string): boolean {
  return LATEX_RE.test(name);
}

export function isHtmlFile(name: string): boolean {
  return HTML_RE.test(name);
}

export function isCsvFile(name: string): boolean {
  return CSV_RE.test(name);
}

type Glyph =
  | "python" | "notebook" | "markdown" | "json" | "config" | "lock" | "git" | "shell"
  | "ts" | "js" | "react" | "rust" | "css" | "code" | "yaml" | "image" | "spreadsheet"
  | "data" | "archive" | "pdf" | "document";

type Tone = "blue" | "amber" | "orange" | "red" | "green" | "purple" | "teal" | "subtext" | "muted";

// Compound selectors so tree rows' `[&_>_svg]:text-subtext` can't override the tone.
const TONE_CLASS = "[&.blue]:text-accent-blue [&.amber]:text-accent-amber [&.orange]:text-accent-orange [&.red]:text-accent-red [&.green]:text-accent-green [&.purple]:text-accent-purple [&.teal]:text-accent-teal [&.subtext]:text-subtext [&.muted]:text-muted";

// First match wins, so exact names and compound suffixes precede broad extension groups.
const RULES: [RegExp, Glyph, Tone][] = [
  [/^\.git(ignore|attributes|modules|keep)$/i, "git", "orange"],
  [/(\.lockb?|^package-lock\.json|^pnpm-lock\.yaml)$/i, "lock", "muted"],
  [/^\.(env(\..+)?|envrc|python-version|nvmrc|node-version|editorconfig|prettierrc|npmrc|flake8)$/i, "config", "subtext"],
  [/^(dockerfile(\..+)?|.+\.dockerfile|\.dockerignore|makefile|procfile)$/i, "config", "blue"],
  [/\.(py|pyi|pyw)$/i, "python", "blue"],
  [/\.ipynb$/i, "notebook", "orange"],
  [MD_RE, "markdown", "blue"],
  [/\.(json|jsonc|jsonl|json5)$/i, "json", "amber"],
  [/\.(toml|ini|cfg|conf|properties)$/i, "config", "subtext"],
  [/\.ya?ml$/i, "yaml", "purple"],
  [/\.(sh|bash|zsh|fish|ps1|bat|cmd)$/i, "shell", "green"],
  [/\.(tsx|jsx)$/i, "react", "teal"],
  [/\.(ts|mts|cts)$/i, "ts", "blue"],
  [/\.(js|mjs|cjs)$/i, "js", "amber"],
  [/\.rs$/i, "rust", "orange"],
  [/\.(css|scss|sass|less)$/i, "css", "blue"],
  [HTML_RE, "code", "orange"],
  [/\.(c|h|cc|cpp|hpp|cu|cuh)$/i, "code", "purple"],
  [/\.(go|swift|kt|java|scala|rb|php|lua|jl|r|sql|proto)$/i, "code", "teal"],
  [IMAGE_RE, "image", "purple"],
  [/\.(csv|tsv|xlsx?|ods)$/i, "spreadsheet", "green"],
  [/\.(parquet|arrow|feather|h5|hdf5|npy|npz|pkl|pickle|pt|pth|ckpt|safetensors|gguf|bin|onnx|db|sqlite3?)$/i, "data", "teal"],
  [/\.(7z|bz2|gz|rar|tar|tgz|xz|zip|zst)$/i, "archive", "amber"],
  [/\.pdf$/i, "pdf", "red"],
  [/\.(tex|bib|sty|cls)$/i, "document", "teal"],
  [/\.(docx?|log|rtf|txt|out|err)$/i, "document", "subtext"],
];

const PAGE = (
  <>
    <path d="M3 1.5h6l4 4v9H3v-13Z" fill="currentColor" opacity=".2" />
    <path d="M9 1.5v4h4" fill="none" stroke="currentColor" strokeWidth="1.2" />
  </>
);

function glyphFor(glyph: Glyph): ReactElement {
  switch (glyph) {
    case "python": {
      const half = "M7.9 1.5C5.6 1.5 5.2 2.5 5.2 3.6V5h3.1v.7H3.7c-1.3 0-2.2.9-2.2 2.7s.8 2.6 2.1 2.6h.8V9.4c0-1.2.9-2.1 2.1-2.1h2.9c1 0 1.8-.8 1.8-1.8V3.6c0-1.1-1-2.1-3.3-2.1Zm-1.3.9a.7.7 0 1 1 0 1.4.7.7 0 0 1 0-1.4Z";
      return (
        <>
          <path d={half} fill="currentColor" fillRule="evenodd" />
          <path d={half} className="text-accent-amber" fill="currentColor" fillRule="evenodd" transform="rotate(180 8 8)" />
        </>
      );
    }
    case "notebook":
      return (
        <>
          <path d="M2.5 6a5.8 3.2 0 0 1 11 0M2.5 10a5.8 3.2 0 0 0 11 0" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
          <circle cx="12.6" cy="2.6" r="1.1" fill="currentColor" />
          <circle cx="3.2" cy="13.6" r="1.1" fill="currentColor" />
        </>
      );
    case "markdown":
      return (
        <>
          <path d="M1 3h14v10H1z" fill="currentColor" opacity=".18" />
          <path d="M2.6 10.5v-5h1.2l1.6 2 1.6-2h1.2v5H6.8V7.6L5.4 9.3 4 7.6v2.9H2.6Zm8.5-5v2.4h1.3L10.5 10 8.6 7.9h1.3V5.5h1.2Z" fill="currentColor" />
        </>
      );
    case "json": {
      const brace = "M5.6 2.5c-1.4 0-1.8.7-1.8 1.9v1.7c0 1-.4 1.5-1.3 1.9.9.4 1.3.9 1.3 1.9v1.7c0 1.2.4 1.9 1.8 1.9";
      return (
        <>
          <path d={brace} fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
          <path d={brace} fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" transform="matrix(-1 0 0 1 16 0)" />
          <circle cx="8" cy="8" r="1" fill="currentColor" />
        </>
      );
    }
    case "config":
      return (
        <>
          <circle cx="8" cy="8" r="3.6" fill="none" stroke="currentColor" strokeWidth="2.2" />
          <path d="M8 1.4v2.2M8 12.4v2.2M1.4 8h2.2M12.4 8h2.2M3.3 3.3l1.6 1.6M11.1 11.1l1.6 1.6M3.3 12.7l1.6-1.6M11.1 4.9l1.6-1.6" stroke="currentColor" strokeWidth="2" />
        </>
      );
    case "lock":
      return (
        <>
          <path d="M5 7.2V5.2a3 3 0 0 1 6 0v2" fill="none" stroke="currentColor" strokeWidth="1.5" />
          <rect x="3" y="7" width="10" height="7.5" rx="1.5" fill="currentColor" />
        </>
      );
    case "git":
      return (
        <>
          <path d="M8 1.5 14.5 8 8 14.5 1.5 8Z" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinejoin="round" />
          <path d="M5.4 4.3 8 6.6v4M8 6.6l2.3 2.1" fill="none" stroke="currentColor" strokeWidth="1.2" />
          <circle cx="8" cy="6.6" r="1.1" fill="currentColor" />
          <circle cx="8" cy="10.8" r="1.1" fill="currentColor" />
          <circle cx="10.4" cy="8.8" r="1.1" fill="currentColor" />
        </>
      );
    case "shell":
      return (
        <>
          <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" fill="currentColor" opacity=".18" />
          <path d="m4 6 2.2 2L4 10M7.8 10.5h4" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
        </>
      );
    case "ts":
    case "js":
      return (
        <>
          <rect x="1.5" y="1.5" width="13" height="13" rx="1.5" fill="currentColor" />
          <text x="13.4" y="13.2" textAnchor="end" fontSize="7" fontWeight="700" fontFamily="system-ui, sans-serif" className={glyph === "ts" ? "fill-white" : "fill-black"}>
            {glyph === "ts" ? "TS" : "JS"}
          </text>
        </>
      );
    case "react":
      return (
        <>
          <g fill="none" stroke="currentColor" strokeWidth="1">
            <ellipse cx="8" cy="8" rx="6.5" ry="2.5" />
            <ellipse cx="8" cy="8" rx="6.5" ry="2.5" transform="rotate(60 8 8)" />
            <ellipse cx="8" cy="8" rx="6.5" ry="2.5" transform="rotate(120 8 8)" />
          </g>
          <circle cx="8" cy="8" r="1.3" fill="currentColor" />
        </>
      );
    case "rust":
      return (
        <>
          <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" strokeWidth="1.3" strokeDasharray="1.5 1.013" />
          <text x="8" y="10.6" textAnchor="middle" fontSize="7.5" fontWeight="700" fontFamily="system-ui, sans-serif" fill="currentColor">R</text>
        </>
      );
    case "css":
      return <path d="M6.3 2.5 5 13.5M11 2.5 9.7 13.5M2.8 6h11M2.2 10h11" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />;
    case "code":
      return <path d="M6.2 3 1.8 8l4.4 5 1.3-1.2L4.2 8l3.3-3.8L6.2 3Zm3.6 0-1.3 1.2L11.8 8l-3.3 3.8 1.3 1.2 4.4-5-4.4-5Z" fill="currentColor" />;
    case "yaml":
      return (
        <>
          <path d="M2.5 3.5h6M5 8h8.5M5 12.5h6" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          <circle cx="3" cy="8" r=".9" fill="currentColor" />
          <circle cx="3" cy="12.5" r=".9" fill="currentColor" />
        </>
      );
    case "image":
      return (
        <>
          <rect x="1.5" y="2" width="13" height="12" rx="2" fill="currentColor" opacity=".18" />
          <circle cx="5" cy="5.5" r="1.4" fill="currentColor" />
          <path d="m2.8 12 3.3-3.5 2.2 2 2.1-2.5 2.8 4H2.8Z" fill="currentColor" />
        </>
      );
    case "spreadsheet":
      return (
        <>
          <rect x="2" y="1.5" width="12" height="13" rx="1.5" fill="currentColor" opacity=".2" />
          <path d="M3.5 4.5h9M3.5 8h9M3.5 11.5h9M7 3v10M10.5 3v10" stroke="currentColor" strokeWidth="1.1" />
        </>
      );
    case "data": {
      const body = "M2.5 3.8v8.4c0 1.2 2.5 2.3 5.5 2.3s5.5-1.1 5.5-2.3V3.8";
      return (
        <>
          <path d={body} fill="currentColor" opacity=".2" />
          <ellipse cx="8" cy="3.8" rx="5.5" ry="2.3" fill="none" stroke="currentColor" strokeWidth="1.2" />
          <path d={`${body}M2.5 8c0 1.2 2.5 2.3 5.5 2.3s5.5-1.1 5.5-2.3`} fill="none" stroke="currentColor" strokeWidth="1.2" />
        </>
      );
    }
    case "archive":
      return (
        <>
          <path d="M2 2h12v12H2z" fill="currentColor" opacity=".18" />
          <path d="M7 2h2v2H7V2Zm0 3h2v2H7V5Zm0 3h2v2H7V8Zm-0.5 3h3v2h-3v-2Z" fill="currentColor" />
        </>
      );
    case "pdf":
      return (
        <>
          {PAGE}
          <text x="8" y="12.6" textAnchor="middle" fontSize="4.6" fontWeight="800" fontFamily="system-ui, sans-serif" fill="currentColor">PDF</text>
        </>
      );
    case "document":
      return (
        <>
          {PAGE}
          <path d="M5 8h6M5 10.5h6M5 13h4" stroke="currentColor" strokeWidth="1.2" />
        </>
      );
  }
}

export function FileTypeIcon({ name }: { name: string }) {
  const base = name.slice(name.lastIndexOf("/") + 1);
  const rule = RULES.find(([re]) => re.test(base));
  const glyph = rule?.[1] ?? "document";
  const tone = rule?.[2] ?? "subtext";
  return (
    <svg className={`file-tree-icon w-[15px] h-[15px] shrink-0 overflow-visible select-none ${TONE_CLASS} ${tone}`} viewBox="0 0 16 16" aria-hidden="true">
      {glyphFor(glyph)}
    </svg>
  );
}
