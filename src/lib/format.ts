import type { NumberFormat } from "./api";

/** The separators this system writes numbers with, so reports read naturally. */
export function numberFormat(locale?: string): NumberFormat {
  const parts = new Intl.NumberFormat(locale).formatToParts(1234567.5);
  return {
    decimal: parts.find((p) => p.type === "decimal")?.value ?? ".",
    group: parts.find((p) => p.type === "group")?.value ?? ",",
  };
}
