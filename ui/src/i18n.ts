import { getLocale } from "./paraglide/runtime.js";

export const ltr = (value: string | number) => `\u2066${value}\u2069`;

export const fmtNumber = (value: number) => new Intl.NumberFormat(getLocale()).format(value);
