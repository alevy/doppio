// LocalDate is a calendar date with no time-of-day or timezone.
//
// We avoid `Date` for journal dates because `Date` carries an implicit
// timezone offset, which means a single Date value renders differently
// depending on the user's locale. Journal dates are inherently local
// (they're written by humans on a calendar) and should round-trip
// identically across machines.
export interface LocalDate {
  readonly year: number;
  readonly month: number; // 1..12
  readonly day: number; // 1..31
}

// Days from 0000-03-01 (Gregorian) to 1970-01-01.
const DAYS_TO_UNIX_EPOCH = 719_468;
// Days in each 400-year Gregorian cycle.
const DAYS_PER_CYCLE = 146_097;

/**
 * Convert epoch-days (signed days since 1970-01-01, matching the wire
 * format) to a LocalDate.
 *
 * The algorithm is Howard Hinnant's civil_from_days
 * (https://howardhinnant.github.io/date_algorithms.html#civil_from_days),
 * which handles negative inputs (pre-1970 dates) without special cases.
 */
export function localDateFromEpochDays(epochDays: number): LocalDate {
  const z = epochDays + DAYS_TO_UNIX_EPOCH;
  const era = Math.floor(z / DAYS_PER_CYCLE);
  const dayOfEra = z - era * DAYS_PER_CYCLE; // [0, 146096]
  const yearOfEra = Math.floor(
    (dayOfEra - Math.floor(dayOfEra / 1460) + Math.floor(dayOfEra / 36524) - Math.floor(dayOfEra / 146096)) / 365,
  ); // [0, 399]
  const year0 = yearOfEra + era * 400;
  const dayOfYear =
    dayOfEra -
    (365 * yearOfEra + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100)); // [0, 365]
  const mp = Math.floor((5 * dayOfYear + 2) / 153); // [0, 11]
  const day = dayOfYear - Math.floor((153 * mp + 2) / 5) + 1;
  const month = mp < 10 ? mp + 3 : mp - 9;
  const year = year0 + (month <= 2 ? 1 : 0);
  return { year, month, day };
}

/**
 * Inverse of `localDateFromEpochDays`. Useful when comparing journal
 * dates against external inputs (e.g. a date picker) that come in as
 * year/month/day triples.
 */
export function epochDaysFromLocalDate(d: LocalDate): number {
  const y = d.year - (d.month <= 2 ? 1 : 0);
  const era = Math.floor(y / 400);
  const yearOfEra = y - era * 400;
  const dayOfYear =
    Math.floor((153 * (d.month > 2 ? d.month - 3 : d.month + 9) + 2) / 5) +
    d.day -
    1;
  const dayOfEra =
    yearOfEra * 365 +
    Math.floor(yearOfEra / 4) -
    Math.floor(yearOfEra / 100) +
    dayOfYear;
  return era * DAYS_PER_CYCLE + dayOfEra - DAYS_TO_UNIX_EPOCH;
}

/**
 * Render a LocalDate as ISO 8601 (YYYY-MM-DD).
 */
export function localDateToString(d: LocalDate): string {
  const yy = d.year.toString().padStart(4, "0");
  const mm = d.month.toString().padStart(2, "0");
  const dd = d.day.toString().padStart(2, "0");
  return `${yy}-${mm}-${dd}`;
}

/**
 * Compare two LocalDates lexicographically: returns negative, zero, or
 * positive in line with `Array.prototype.sort` callbacks.
 */
export function compareLocalDate(a: LocalDate, b: LocalDate): number {
  if (a.year !== b.year) return a.year - b.year;
  if (a.month !== b.month) return a.month - b.month;
  return a.day - b.day;
}

/**
 * Convert a LocalDate to a `Date` at UTC midnight. The reverse
 * conversion (`fromJSDate`) reads UTC components, so the round-trip
 * is lossless. Convenient for handing dates to date-picker UIs that
 * expect a `Date`.
 */
export function localDateToJSDate(d: LocalDate): Date {
  return new Date(Date.UTC(d.year, d.month - 1, d.day));
}

/**
 * Read a `Date`'s UTC year/month/day components into a LocalDate.
 * Pairs with `localDateToJSDate` for round-tripping through UI inputs.
 */
export function localDateFromJSDate(d: Date): LocalDate {
  return {
    year: d.getUTCFullYear(),
    month: d.getUTCMonth() + 1,
    day: d.getUTCDate(),
  };
}
