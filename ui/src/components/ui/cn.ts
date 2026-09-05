import { extendTailwindMerge } from "tailwind-merge";

const twMerge = extendTailwindMerge({ extend: { theme: { text: ["menu"] } } });

export function cn(...classes: Array<string | false | null | undefined>) {
  return twMerge(...classes);
}
