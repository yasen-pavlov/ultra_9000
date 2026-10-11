import { describe, expect, it } from "vitest";
import { itemState } from "../test/fixtures";
import { closeQuestion, type CloseQuestion, type CloseQuestionInput } from "./confirm";

const text = (question: CloseQuestion) =>
  question.body.map((paragraph) => paragraph.parts.map((part) => part.text).join(""));
const strong = (question: CloseQuestion) =>
  question.body.flatMap((paragraph) => paragraph.parts.filter((part) => part.strong).map((part) => part.text));

function ask(overrides: Partial<CloseQuestionInput>): CloseQuestion {
  return closeQuestion({
    item: itemState({ title: "Refactor retries" }),
    reason: "agent-working",
    detail: null,
    toolLabel: "OMP",
    toolPath: null,
    project: "harbor",
    ...overrides,
  });
}

describe("close question copy", () => {
  it("a working agent: the turn's work is lost, the conversation stays saved", () => {
    const question = ask({});
    expect(question.title).toBe("End “Refactor retries”?");
    expect(question.endLabel).toBe("End agent");
    expect(text(question)).toEqual([
      "OMP is in the middle of a turn. Ending now stops it: this turn's unfinished work is lost. Changes it already wrote to files stay written.",
      "The conversation up to the last completed turn stays saved in harbor, so you can resume it.",
    ]);
    expect(strong(question)).toEqual(["this turn's unfinished work is lost"]);
    expect(question.body.map((paragraph) => paragraph.dim)).toEqual([false, true]);
  });

  it("a waiting agent names what it waits for", () => {
    const waitingFor = (attention: "approval" | "question" | null) =>
      text(ask({ reason: "agent-needs-you", item: itemState({ attention }) }))[0];
    expect(waitingFor("approval")).toContain("is waiting for your approval.");
    expect(waitingFor("question")).toContain("is waiting for your answer to a question.");
    expect(waitingFor(null)).toContain("is waiting for your reply.");
  });

  it("a shell names its executable and job; the job falls back when unknown", () => {
    const question = ask({ reason: "foreground-job", detail: "cargo", toolLabel: "Shell", toolPath: "/bin/zsh" });
    expect(question.endLabel).toBe("End shell");
    expect(text(question)[0]).toMatch(/^zsh is running cargo\. Ending the shell ends that job too/);
    expect(strong(question)).toEqual(["cargo", "This can't be undone."]);
    expect(text(ask({ reason: "foreground-job", toolLabel: "Shell" }))[0]).toMatch(
      /^Shell is running a foreground job\./,
    );
    expect(text(ask({ reason: "foreground-job", item: null }))[0]).toMatch(/^The shell is running/);
  });

  it("Neovim warns about unsaved buffers", () => {
    const question = ask({ reason: "neovim", toolLabel: "Neovim" });
    expect(question.endLabel).toBe("End Neovim");
    expect(strong(question)).toEqual(["unsaved changes may be lost"]);
  });

  it("an unmonitored agent says it cannot tell", () => {
    expect(text(ask({ reason: "unmonitored-agent", toolLabel: "Codex" }))).toEqual([
      "Codex reports no turn state, so Ultra 9000 can't tell whether it is in the middle of something. Ending it stops the process.",
      "Codex keeps its own saved conversation; resume it from the palette.",
    ]);
  });

  it("an item that vanished reads as this item", () => {
    expect(ask({ item: null }).title).toBe("End “this item”?");
  });
});
