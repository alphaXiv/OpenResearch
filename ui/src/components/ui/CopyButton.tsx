import { Check, Copy } from "lucide-react";
import { useState } from "react";
import { IconButton, type IconButtonProps } from "./IconButton";

/** A small icon button that copies `text` to the clipboard, flashing a
 *  checkmark for 1.5s on success. Mirrors the copy affordance on Md.tsx's
 *  code blocks; `title` is caller-supplied so this stays free of i18n
 *  imports like the rest of `ui/`. */
export function CopyButton({
  text,
  title,
  size = "small",
  ...props
}: { text: string; title: string } & Omit<IconButtonProps, "title" | "onClick" | "children">) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    navigator.clipboard?.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };
  return (
    <IconButton size={size} title={title} aria-label={title} onClick={copy} {...props}>
      {copied ? <Check size={12} /> : <Copy size={12} />}
    </IconButton>
  );
}
