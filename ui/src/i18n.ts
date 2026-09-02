import { getLocale } from "./paraglide/runtime.js";

// LRI/FSI through PDI keep interpolated text from reordering its surrounding sentence.
export const ltr = (value: string | number) => `\u2066${value}\u2069`;
export const autoDir = (value: string | number) => `\u2068${value}\u2069`;

export const fmtNumber = (value: number) => new Intl.NumberFormat(getLocale()).format(value);
