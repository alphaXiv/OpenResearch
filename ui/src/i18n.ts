import { getLocale } from "./paraglide/runtime.js";

// LRI/FSI through PDI keep interpolated text from reordering its surrounding sentence.
export const ltr = (value: string | number) => `\u2066${value}\u2069`;
export const autoDir = (value: string | number) => `\u2068${value}\u2069`;

export const fmtNumber = (value: number) => new Intl.NumberFormat(getLocale()).format(value);

/** "14:02" today, "Oct 28, 14:02" otherwise. */
export function fmtClock(ms: number): string {
  const sameDay = new Date(ms).toDateString() === new Date().toDateString();
  const options: Intl.DateTimeFormatOptions = sameDay
    ? { hour: "numeric", minute: "2-digit" }
    : { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" };
  return new Intl.DateTimeFormat(getLocale(), options).format(ms);
}

const BYTE_UNITS = [["gigabyte", 1024 ** 3], ["megabyte", 1024 ** 2], ["kilobyte", 1024]] as const;

/** Binary sizes with the familiar unit names: "3.97 GB", "64 MB". */
export function fmtBytes(bytes: number): string {
  const [unit, size] = BYTE_UNITS.find(([, size]) => bytes >= size) ?? ["byte", 1];
  const value = bytes / size;
  return new Intl.NumberFormat(getLocale(), {
    style: "unit",
    unit,
    unitDisplay: size > 1 ? "short" : "long",
    maximumFractionDigits: value < 10 && size > 1 ? 2 : 0,
  }).format(value);
}
