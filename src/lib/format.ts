// Pure display formatting shared by the sidebar, palette and sheets.

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

// Times read "14:02" as on the design boards, whatever the locale's default hour cycle.
const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
const monthDayFormat = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const monthDayYearFormat = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });
const fullFormat = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  hourCycle: "h23",
});
const secondsFormat = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hourCycle: "h23",
});

function startOfDay(at: number): number {
  const date = new Date(at);
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

/** Compact age for sidebar rows: "<1m", "6m", "3h", "2d". */
export function shortAge(since: number, now: number): string {
  const elapsed = Math.max(0, now - since);
  if (elapsed < MINUTE) return "<1m";
  if (elapsed < HOUR) return `${Math.floor(elapsed / MINUTE)}m`;
  if (elapsed < DAY) return `${Math.floor(elapsed / HOUR)}h`;
  return `${Math.floor(elapsed / DAY)}d`;
}

/** Spelled-out duration for sentences: "40 seconds", "6 min", "3 h". */
export function spokenAge(since: number, now: number): string {
  const elapsed = Math.max(0, now - since);
  if (elapsed < MINUTE) {
    const seconds = Math.max(1, Math.round(elapsed / 1000));
    return seconds === 1 ? "1 second" : `${seconds} seconds`;
  }
  if (elapsed < HOUR) return `${Math.floor(elapsed / MINUTE)} min`;
  if (elapsed < DAY) return `${Math.floor(elapsed / HOUR)} h`;
  const days = Math.floor(elapsed / DAY);
  return days === 1 ? "1 day" : `${days} days`;
}

/** Recency of a saved conversation: "just now", "5 min", "2 h", "yesterday", "6 d", "3 wk", "Aug 18". */
export function recency(at: number, now: number): string {
  const elapsed = Math.max(0, now - at);
  const days = Math.round((startOfDay(now) - startOfDay(at)) / DAY);
  if (days <= 0) {
    if (elapsed < MINUTE) return "just now";
    if (elapsed < HOUR) return `${Math.floor(elapsed / MINUTE)} min`;
    return `${Math.floor(elapsed / HOUR)} h`;
  }
  if (days === 1) return "yesterday";
  if (days < 14) return `${days} d`;
  if (days < 63) return `${Math.floor(days / 7)} wk`;
  return calendarDay(at, now);
}

/** Recency bucket headings for long saved lists: "Today", "Yesterday", "This week", "Earlier this month", "August". */
export function recencyBucket(at: number, now: number): string {
  const days = Math.round((startOfDay(now) - startOfDay(at)) / DAY);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days < 7) return "This week";
  const date = new Date(at);
  const today = new Date(now);
  if (date.getFullYear() === today.getFullYear() && date.getMonth() === today.getMonth()) return "Earlier this month";
  const month = date.toLocaleString(undefined, { month: "long" });
  return date.getFullYear() === today.getFullYear() ? month : `${month} ${date.getFullYear()}`;
}

/** "Sep 30", with the year when it is not the current one. */
export function calendarDay(at: number, now: number): string {
  return new Date(at).getFullYear() === new Date(now).getFullYear()
    ? monthDayFormat.format(at)
    : monthDayYearFormat.format(at);
}

/** "14:02" */
export function clockTime(at: number): string {
  return timeFormat.format(at);
}

/** "14:02:09" */
export function clockSeconds(at: number): string {
  return secondsFormat.format(at);
}

/** "Tue Sep 9, 10:41" */
export function fullDate(at: number): string {
  return fullFormat.format(at);
}

/**
 * Short model name for rows: "anthropic/claude-opus-5-5" → "opus 5.5". Version pairs written with
 * dashes become dotted; the provider and a leading "claude-" family prefix are dropped.
 */
export function modelShort(model: string): string {
  const name = model.slice(model.lastIndexOf("/") + 1).replace(/^claude-/, "");
  return name.replaceAll(/-(\d+)-(\d+)(?=$|-)/g, " $1.$2").replaceAll("-", " ");
}

/** Abbreviates the home directory as "~". */
export function tildePath(path: string, home: string): string {
  if (home === "" || home === "/") return path;
  if (path === home) return "~";
  return path.startsWith(`${home}/`) ? `~${path.slice(home.length)}` : path;
}

/** Drops trailing "/" characters ("/w/atlas///" → "/w/atlas"); a linear scan, no backtracking regex. */
export function trimTrailingSlashes(path: string): string {
  let end = path.length;
  while (end > 0 && path[end - 1] === "/") end--;
  return path.slice(0, end);
}

export function basename(path: string): string {
  const trimmed = trimTrailingSlashes(path);
  const name = trimmed.slice(trimmed.lastIndexOf("/") + 1);
  return name === "" ? path : name;
}

/** File sizes as the boards write them: "640 B", "18 KB", "14.2 MB" (1024-based). */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Two-letter project initials for the collapsed strip ("atlas" → "at"). */
export function projectInitials(name: string): string {
  const letters = name.replace(/[^\p{L}\p{N}]/gu, "");
  return (letters === "" ? name : letters).slice(0, 2);
}

/** Splits text around case-insensitive occurrences of the query for <mark> highlighting. */
export function highlight(text: string, query: string): { text: string; match: boolean }[] {
  const needle = query.trim().toLowerCase();
  if (needle === "") return [{ text, match: false }];
  const haystack = text.toLowerCase();
  const parts: { text: string; match: boolean }[] = [];
  let from = 0;
  for (;;) {
    const at = haystack.indexOf(needle, from);
    if (at < 0) break;
    if (at > from) parts.push({ text: text.slice(from, at), match: false });
    parts.push({ text: text.slice(at, at + needle.length), match: true });
    from = at + needle.length;
  }
  if (from < text.length) parts.push({ text: text.slice(from), match: false });
  return parts;
}

/** Case-insensitive substring match over several fields (the palette's local running-item filter). */
export function matchesQuery(query: string, ...fields: (string | null | undefined)[]): boolean {
  const needle = query.trim().toLowerCase();
  if (needle === "") return true;
  return fields.some((field) => field?.toLowerCase().includes(needle) === true);
}
