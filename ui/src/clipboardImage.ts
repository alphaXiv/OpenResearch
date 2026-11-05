export async function readClipboardImage(
  clipboard: Pick<Clipboard, "read"> | undefined =
    typeof navigator === "undefined" ? undefined : navigator.clipboard,
): Promise<File | null> {
  if (!clipboard?.read) {
    return null;
  }
  try {
    const items = await clipboard.read();
    for (const item of items) {
      const type = item.types.find((candidate) => candidate.startsWith("image/"));
      if (!type) continue;
      const blob = await item.getType(type);
      if (blob.size === 0) continue;
      const extension =
        type === "image/jpeg"
          ? "jpg"
          : type.slice("image/".length).replace(/[^a-z0-9]+/gi, "") || "png";
      return new File([blob], `pasted-image.${extension}`, {
        type,
        lastModified: Date.now(),
      });
    }
  } catch {
    return null;
  }
  return null;
}
