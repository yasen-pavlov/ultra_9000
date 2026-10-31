// Tests run in Node (Vitest), but the project compiles for the web view without Node's types.
// The one Node API a test needs: reading a shipped style sheet, which Vitest does not load (it
// stubs CSS imports, `?raw` included).
declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf8"): string;
}
