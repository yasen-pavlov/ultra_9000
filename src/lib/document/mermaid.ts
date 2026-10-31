// Mermaid diagrams, loaded on first use (never in the initial bundle) and run in strict security
// mode. A diagram that fails falls back to its source with a one-line reason (board 32).
import type { Mermaid } from "mermaid";

let loading: Promise<Mermaid> | null = null;
let sequence = 0;

function load(): Promise<Mermaid> {
  loading ??= import("mermaid").then(({ default: mermaid }) => {
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: "base",
      fontFamily: "var(--ui)",
      themeVariables: {
        darkMode: true,
        background: "#212121",
        primaryColor: "#2b2b2b",
        primaryBorderColor: "#474747",
        primaryTextColor: "#e4ebee",
        secondaryColor: "#272727",
        tertiaryColor: "#1c1c1c",
        lineColor: "#8c8b8b",
        textColor: "#b0bec5",
        mainBkg: "#2b2b2b",
        nodeBorder: "#474747",
        clusterBkg: "#1c1c1c",
        edgeLabelBackground: "#212121",
      },
    });
    return mermaid;
  });
  loading.catch(() => {
    loading = null;
  });
  return loading;
}

/** The first line of Mermaid's error: “Parse error on line 3:” → “parse error on line 3”. */
export function mermaidReason(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  const first = message.split("\n")[0].replace(/:$/, "").trim();
  if (first === "") return "could not render";
  return first[0].toLowerCase() + first.slice(1);
}

export async function renderMermaid(source: string): Promise<{ svg: string } | { error: string }> {
  sequence += 1;
  const id = `u9k-mermaid-${sequence}`;
  try {
    const mermaid = await load();
    const { svg } = await mermaid.render(id, source);
    return { svg };
  } catch (error) {
    // A failed render can leave its scratch container in the body.
    document.getElementById(`d${id}`)?.remove();
    return { error: mermaidReason(error) };
  }
}
