// Frontmatter as the reader shows it (board 31): one collapsed mono line (“frontmatter · 5 fields ·
// status: draft”), expandable to the fields as written. Never a table in the prose. Only top-level
// keys are split out; nested and list values stay as the source wrote them.

export interface FrontmatterField {
  key: string;
  value: string;
}

export interface Frontmatter {
  format: "yaml" | "toml";
  fields: FrontmatterField[];
  /** The source between the fences, for fields that do not split into key/value lines. */
  raw: string;
}

/** Key names, checked after splitting at the separator (single quantifiers, no backtracking). */
const YAML_KEY_NAME = /^\w[\w .-]*$/;
const TOML_KEY_NAME = /^\w[\w.-]*$/;
const TOML_TABLE = /^\[{1,2}[^\]]+\]{1,2}$/;

function unquote(value: string): string {
  const trimmed = value.trim();
  if (trimmed.length >= 2) {
    const first = trimmed[0];
    if ((first === '"' || first === "'") && trimmed.at(-1) === first) return trimmed.slice(1, -1);
  }
  return trimmed;
}

/** A top-level `key: value` (YAML) or `key = value` (TOML) line as a field, else null. */
function splitField(format: "yaml" | "toml", line: string): FrontmatterField | null {
  if (/^\s/.test(line) || line.startsWith("- ")) return null;
  const at = line.indexOf(format === "yaml" ? ":" : "=");
  if (at <= 0) return null;
  const key = line.slice(0, at).trim();
  const rest = line.slice(at + 1);
  if (!(format === "yaml" ? YAML_KEY_NAME : TOML_KEY_NAME).test(key)) return null;
  // YAML needs whitespace (or nothing) after the colon: `see: http://x` splits, `http://x` does not.
  if (format === "yaml" && rest !== "" && !/^\s/.test(rest)) return null;
  return { key, value: unquote(rest) };
}

/** A TOML `[table]` header line as a field with no value, else null. */
function tableField(format: "yaml" | "toml", line: string): FrontmatterField | null {
  const text = line.trim();
  return format === "toml" && TOML_TABLE.test(text) ? { key: text, value: "" } : null;
}

/** Nested, list and continuation lines join the previous value as written. */
function appendLine(fields: FrontmatterField[], text: string): void {
  const last = fields.at(-1);
  if (last === undefined) fields.push({ key: "", value: text });
  else last.value = last.value === "" ? text : `${last.value} ${text}`;
}

/** Splits top-level `key: value` (YAML) or `key = value` (TOML) lines; continuation lines join the previous value. */
export function parseFrontmatter(format: "yaml" | "toml", raw: string): Frontmatter {
  const fields: FrontmatterField[] = [];
  for (const line of raw.split("\n")) {
    if (line.trim() === "" || line.trimStart().startsWith("#")) continue;
    const field = splitField(format, line) ?? tableField(format, line);
    if (field === null) appendLine(fields, line.trim());
    else fields.push(field);
  }
  return { format, fields, raw };
}

/** The collapsed line: “frontmatter · 5 fields · status: draft” (status, else title, when present). */
export function frontmatterSummary(frontmatter: Frontmatter): string {
  const count = frontmatter.fields.length;
  const parts = ["frontmatter", count === 1 ? "1 field" : `${count} fields`];
  const featured = ["status", "title"]
    .map((key) => frontmatter.fields.find((field) => field.key.toLowerCase() === key))
    .find((field) => field !== undefined && field.value !== "");
  if (featured !== undefined) parts.push(`${featured.key}: ${featured.value}`);
  return parts.join(" · ");
}
