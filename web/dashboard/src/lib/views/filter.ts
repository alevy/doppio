import { compareLocalDate, type LocalDate } from "doppio-dop";

/**
 * Test whether a date sits inside the [begin, end] window. Either
 * bound may be null to leave that side unbounded.
 */
export function dateInRange(
  date: LocalDate,
  begin: LocalDate | null,
  end: LocalDate | null,
): boolean {
  if (begin && compareLocalDate(date, begin) < 0) return false;
  if (end && compareLocalDate(date, end) > 0) return false;
  return true;
}
