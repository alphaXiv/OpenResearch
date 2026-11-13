import { m } from "./paraglide/messages.js";
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

const BYTE_UNITS = [m.byte_size_b, m.byte_size_kib, m.byte_size_mib, m.byte_size_gib, m.byte_size_tib];

/** Binary sizes in IEC units, to three significant figures: "512 B", "3.97 GiB", "64 MiB". */
export function fmtBytes(bytes: number): string {
  let value = bytes;
  let exponent = 0;
  // 1023.5 rather than 1024, so a value that rounds up reads "1 MiB" instead of "1,024 KiB".
  while (value >= 1023.5 && exponent < BYTE_UNITS.length - 1) {
    value /= 1024;
    exponent += 1;
  }

  const digits = exponent === 0 || value >= 100 ? 0 : value >= 10 ? 1 : 2;
  const number = new Intl.NumberFormat(getLocale(), { maximumFractionDigits: digits }).format(value);
  return BYTE_UNITS[exponent]({ value: number });
}

/** Installed memory the way spec sheets and About This Mac print it: whole binary gigabytes labelled GB, "48 GB". */
export function fmtMemory(bytes: number): string {
  const gigabytes = Math.round(bytes / 1024 ** 3);
  return m.memory_size_gb({ value: new Intl.NumberFormat(getLocale()).format(gigabytes) });
}
