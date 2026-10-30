const CANDIDATE_DELIMITERS = [",", ";", "\t", "|"];

/** `.tsv` is tab-separated; for anything else the header line's most frequent
 * unquoted candidate wins, with comma on ties. */
export function detectDelimiter(path: string, text: string): string {
  if (/\.tsv$/i.test(path)) return "\t";

  const counts = new Map<string, number>();
  let quoted = false;
  for (const char of text) {
    if (char === '"') quoted = !quoted;
    else if (!quoted && (char === "\n" || char === "\r")) break;
    else if (!quoted && CANDIDATE_DELIMITERS.includes(char)) counts.set(char, (counts.get(char) ?? 0) + 1);
  }

  return CANDIDATE_DELIMITERS.reduce((best, candidate) =>
    (counts.get(candidate) ?? 0) > (counts.get(best) ?? 0) ? candidate : best,
  );
}

/** RFC 4180 records: quoted fields may hold delimiters, newlines, and `""`.
 * Blank lines are skipped and rows are not padded to a common width. */
export function parseCsv(text: string, delimiter: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;

  const endField = () => {
    row.push(field);
    field = "";
  };
  const endRow = () => {
    endField();
    if (row.length > 1 || row[0] !== "") rows.push(row);
    row = [];
  };

  const start = text.charCodeAt(0) === 0xfeff ? 1 : 0;
  for (let i = start; i < text.length; i++) {
    const char = text[i];
    if (quoted) {
      if (char !== '"') field += char;
      else if (text[i + 1] === '"') {
        field += '"';
        i++;
      } else quoted = false;
    } else if (char === '"' && field === "") quoted = true;
    else if (char === delimiter) endField();
    else if (char === "\n") endRow();
    else if (char === "\r") {
      endRow();
      if (text[i + 1] === "\n") i++;
    } else field += char;
  }
  if (field !== "" || row.length > 0) endRow();

  return rows;
}
