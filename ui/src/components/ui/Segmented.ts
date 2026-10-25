/** A segmented toggle: put it on the group; mark the selected `<button>` with `.active`. */
export const SEGMENTED_CLASS_NAME = [
  "seg inline-flex items-center gap-0.5 p-0.5 rounded-md bg-hover-subtle",
  "[&_button]:inline-flex [&_button]:items-center [&_button]:gap-1.5 [&_button]:h-6 [&_button]:px-2.5 [&_button]:rounded-sm",
  "[&_button]:text-sm [&_button]:font-medium [&_button]:text-subtext [&_button]:transition-[background,color] [&_button]:duration-120 [&_button]:ease-standard",
  "[&_button:not(:disabled):hover]:text-text [&_button.active]:bg-background [&_button.active]:text-text [&_button.active]:shadow-segment",
  "[&_button:disabled]:text-muted [&_button:disabled]:cursor-default",
  "[&_button:focus-visible]:outline-2 [&_button:focus-visible]:outline-solid [&_button:focus-visible]:outline-focus [&_button:focus-visible]:outline-offset-2",
].join(" ");
