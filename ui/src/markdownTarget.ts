export interface MarkdownTarget {
  path: string;
  query: string;
  hash: string;
}

export function isExternalMarkdownTarget(target: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(target) || target.startsWith("//");
}

export function resolveMarkdownTarget(
  folder: string,
  target: string,
  preserveLeadingSlash = false,
): MarkdownTarget | null {
  const hashAt = target.indexOf("#");
  const beforeHash = hashAt === -1 ? target : target.slice(0, hashAt);
  const hash = hashAt === -1 ? "" : target.slice(hashAt);
  const queryAt = beforeHash.indexOf("?");
  const encodedPath = queryAt === -1 ? beforeHash : beforeHash.slice(0, queryAt);
  const query = queryAt === -1 ? "" : beforeHash.slice(queryAt + 1);

  let pathname: string;
  try {
    pathname = decodeURI(encodedPath);
  } catch {
    return null;
  }
  if (!pathname || pathname.includes("\0")) return null;

  const leadingSlash = pathname.startsWith("/");
  const parts = leadingSlash ? [] : folder.split("/").filter(Boolean);
  for (const part of pathname.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (parts.length === 0) return null;
      parts.pop();
      continue;
    }
    parts.push(part);
  }

  if (parts.length === 0) return null;
  return {
    path: `${preserveLeadingSlash && (leadingSlash || folder.startsWith("/")) ? "/" : ""}${parts.join("/")}`,
    query,
    hash,
  };
}

export function markdownTargetUrl(url: string, target: MarkdownTarget): string {
  return `${url}${target.query ? `&${target.query}` : ""}${target.hash}`;
}
