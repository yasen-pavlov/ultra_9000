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

const YAML_KEY = /^([A-Za-z0-9_][\w .-]*?)\s*:(?:\s+(.*))?$/;
const TOML_KEY = /^([A-Za-z0-9_][\w.-]*)\s*=\s*(.*)$/;
const TOML_TABLE = /^\[{1,2}[^\]]+\]{1,2}$/;

function unquote(value: string): string {
  const trimmed = value.trim();
  if (trimmed.length >= 2) {
    const first = trimmed[0];
    if ((first === '"' || first === "'") && trimmed.at(-1) === first) return trimmed.slice(1, -1);
  }
  return trimmed;
}

/** Splits top-level `key: value` (YAML) or `key = value` (TOML) lines; continuation lines join the previous value. */
export function parseFrontmatter(format: "yaml" | "toml", raw: string): Frontmatter {
  const fields: FrontmatterField[] = [];
  const pattern = format === "yaml" ? YAML_KEY : TOML_KEY;
  for (const line of raw.split("\n")) {
    if (line.trim() === "" || line.trimStart().startsWith("#")) continue;
    const topLevel = !/^\s/.test(line) && !line.startsWith("- ");
    const match = topLevel ? pattern.exec(line) : null;
    if (match !== null) {
      fields.push({ key: match[1], value: unquote(match[2] ?? "") });
      continue;
    }
    if (format === "toml" && TOML_TABLE.test(line.trim())) {
      fields.push({ key: line.trim(), value: "" });
      continue;
    }
    const last = fields.at(-1);
    if (last === undefined) fields.push({ key: "", value: line.trim() });
    else last.value = last.value === "" ? line.trim() : `${last.value} ${line.trim()}`;
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
