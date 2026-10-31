// Where a link in a rendered document goes. The web view never navigates: in-page fragments
// scroll, http(s)/mailto open outside through native, relative Markdown files open as documents,
// and anything else does nothing.

export type LinkTarget =
  | { kind: "fragment"; id: string }
  | { kind: "external"; url: string }
  | { kind: "document"; path: string }
  | { kind: "none" };

const MARKDOWN = /\.(md|markdown)$/i;

function decode(text: string): string {
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
}

/** Joins and normalises `relative` against the directory `base` (absolute, “/”-separated). */
export function joinPath(base: string, relative: string): string {
  const parts = relative.startsWith("/") ? [] : base.split("/").filter((part) => part !== "");
  for (const part of relative.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return `/${parts.join("/")}`;
}

/** `documentPath` is the canonical absolute path of the document the link is in. */
export function linkTarget(documentPath: string, href: string): LinkTarget {
  if (href.startsWith("#")) return href.length > 1 ? { kind: "fragment", id: decode(href.slice(1)) } : { kind: "none" };
  if (/^(https?|mailto):/i.test(href)) return { kind: "external", url: href };
  if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith("//")) return { kind: "none" };
  const path = decode(href.split(/[?#]/)[0]);
  if (!MARKDOWN.test(path)) return { kind: "none" };
  return { kind: "document", path: joinPath(documentPath.slice(0, documentPath.lastIndexOf("/")), path) };
}
