import { describe, expect, it } from "vitest";
import { frontmatterSummary, parseFrontmatter } from "./frontmatter";

describe("frontmatter header", () => {
  it("splits top-level YAML keys, keeping nested and list values as written", () => {
    const frontmatter = parseFrontmatter(
      "yaml",
      'title: "Importer rollover"\nauthor: omp, opus 5.5\nreviewers:\n  - ana\n  - ben\nstatus: draft',
    );
    expect(frontmatter.fields).toEqual([
      { key: "title", value: "Importer rollover" },
      { key: "author", value: "omp, opus 5.5" },
      { key: "reviewers", value: "- ana - ben" },
      { key: "status", value: "draft" },
    ]);
    expect(frontmatterSummary(frontmatter)).toBe("frontmatter · 4 fields · status: draft");
  });

  it("reads TOML keys and tables", () => {
    const frontmatter = parseFrontmatter("toml", 'title = "Plan"\n[extra]\nowner = "ana"');
    expect(frontmatter.fields.map((field) => field.key)).toEqual(["title", "[extra]", "owner"]);
    expect(frontmatterSummary(frontmatter)).toBe("frontmatter · 3 fields · title: Plan");
  });
});
