/** A message's ref (`msg_` and the start of its id) or a chat's id, as agents write them. */
export const CITED_ID = /(?<![\w-])(?:msg_[0-9a-f]{8}|chat_[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})(?![\w-])/g;

/** How a message's ref reads while its words load. */
export const refLabel = (cited: string) => `#${cited.slice("msg_".length)}`;

/** A message's first line as plain text, for a one-line preview. */
export function firstLine(text: string): string {
  const line = text.split("\n").find((candidate) => candidate.trim()) ?? "";
  return line
    .replace(/^[\s#>*_-]+/, "")
    .replace(/\*+|__|`/g, "")
    .trim();
}
