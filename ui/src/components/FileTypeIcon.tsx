import type { ReactNode } from "react";

const IMAGE_RE = /\.(png|jpe?g|gif|webp|svg)$/i;
const MD_RE = /\.(md|mdx|markdown)$/i;
const SPREADSHEET_RE = /\.(csv|tsv|xlsx?|ods)$/i;
const CODE_RE = /\.(c|cc|cpp|css|go|html?|java|js|jsx|json|mjs|py|rs|sh|toml|ts|tsx|ya?ml)$/i;
const ARCHIVE_RE = /\.(7z|bz2|gz|rar|tar|tgz|zip)$/i;
const PDF_RE = /\.pdf$/i;
const DOCUMENT_RE = /\.(docx?|log|rtf|txt)$/i;

export function isImageFile(name: string): boolean {
  return IMAGE_RE.test(name);
}

export function isMarkdownFile(name: string): boolean {
  return MD_RE.test(name);
}

export function FileTypeIcon({ name }: { name: string }) {
  const kind = isMarkdownFile(name)
    ? "markdown"
    : isImageFile(name)
      ? "image"
      : SPREADSHEET_RE.test(name)
        ? "spreadsheet"
        : CODE_RE.test(name)
          ? "code"
          : ARCHIVE_RE.test(name)
            ? "archive"
            : PDF_RE.test(name)
              ? "pdf"
              : DOCUMENT_RE.test(name)
                ? "document"
                : "file";

  let glyph: ReactNode;
  if (kind === "markdown") {
    glyph = (
      <>
        <path d="M1 3h14v10H1z" fill="currentColor" opacity=".18" />
        <path d="M2.6 10.5v-5h1.2l1.6 2 1.6-2h1.2v5H6.8V7.6L5.4 9.3 4 7.6v2.9H2.6Zm8.5-5v2.4h1.3L10.5 10 8.6 7.9h1.3V5.5h1.2Z" fill="currentColor" />
      </>
    );
  } else if (kind === "image") {
    glyph = (
      <>
        <rect x="1.5" y="2" width="13" height="12" rx="2" fill="currentColor" opacity=".18" />
        <circle cx="5" cy="5.5" r="1.4" fill="currentColor" />
        <path d="m2.8 12 3.3-3.5 2.2 2 2.1-2.5 2.8 4H2.8Z" fill="currentColor" />
      </>
    );
  } else if (kind === "spreadsheet") {
    glyph = (
      <>
        <rect x="2" y="1.5" width="12" height="13" rx="1.5" fill="currentColor" opacity=".2" />
        <path d="M3.5 4.5h9M3.5 8h9M3.5 11.5h9M7 3v10M10.5 3v10" stroke="currentColor" strokeWidth="1.1" />
      </>
    );
  } else if (kind === "code") {
    glyph = <path d="M6.2 3 1.8 8l4.4 5 1.3-1.2L4.2 8l3.3-3.8L6.2 3Zm3.6 0-1.3 1.2L11.8 8l-3.3 3.8 1.3 1.2 4.4-5-4.4-5Z" fill="currentColor" />;
  } else if (kind === "archive") {
    glyph = (
      <>
        <path d="M2 2h12v12H2z" fill="currentColor" opacity=".18" />
        <path d="M7 2h2v2H7V2Zm0 3h2v2H7V5Zm0 3h2v2H7V8Zm-.5 3h3v2h-3v-2Z" fill="currentColor" />
      </>
    );
  } else {
    glyph = (
      <>
        <path d="M3 1.5h6l4 4v9H3v-13Z" fill="currentColor" opacity=".2" />
        <path d="M9 1.5v4h4" fill="none" stroke="currentColor" strokeWidth="1.2" />
        <path d="M5 8h6M5 10.5h6M5 13h4" stroke="currentColor" strokeWidth="1.2" />
      </>
    );
  }

  return (
    <svg className={`file-tree-icon [width:15px] [height:15px] [flex-shrink:0] [color:var(--muted)] [overflow:visible] [&.markdown]:[color:var(--accent-blue)] [&.image]:[color:var(--accent-purple)] [&.spreadsheet]:[color:var(--accent-green)] [&.code]:[color:var(--accent-orange)] [&.archive]:[color:var(--accent-amber)] [&.pdf]:[color:var(--accent-red)] [&.document]:[color:var(--subtext)] ${kind}`} viewBox="0 0 16 16" aria-hidden="true">
      {glyph}
    </svg>
  );
}
