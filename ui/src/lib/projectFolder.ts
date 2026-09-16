/** Compose one new child of a selected parent without interpreting it as a path. */
export function childProjectPath(parent: string, name: string): string | null {
  const child = name.trim();
  if (!child || child === "." || child === ".." || /[\\/\x00-\x1f]/.test(child)) return null;

  // Use the server's path, not the browser OS (the dashboard may be forwarded).
  const windows = /^[a-z]:[\\/]/i.test(parent) || parent.startsWith("\\\\");
  if (windows && (
    /[<>:"|?*]/.test(child) || child.endsWith(".") ||
    /^(con|prn|aux|nul|com[1-9¹²³]|lpt[1-9¹²³])(?:\.|$)/i.test(child)
  )) return null;

  const separator = windows ? "\\" : "/";
  const trimmedParent = parent.replace(windows ? /[\\/]+$/ : /\/+$/, "");
  return `${trimmedParent}${separator}${child}`;
}
