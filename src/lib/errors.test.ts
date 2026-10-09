import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { beforeEach, describe, expect, it } from "vitest";
import { run, uiErrors } from "./errors.svelte";
import { settingsSet } from "./native";

describe("uiErrors", () => {
  beforeEach(() => {
    for (const entry of uiErrors.list) uiErrors.dismiss(entry.id);
  });

  it("keeps only the three newest messages, oldest first", () => {
    for (const message of ["a", "b", "c", "d"]) uiErrors.report(message);
    expect(uiErrors.list.map((entry) => entry.message)).toEqual(["b", "c", "d"]);
  });

  it("moves a repeated message to the end under a new id instead of duplicating it", () => {
    uiErrors.report("launch failed");
    uiErrors.report("other");
    const firstId = uiErrors.list[0].id;
    uiErrors.report("launch failed");
    expect(uiErrors.list.map((entry) => entry.message)).toEqual(["other", "launch failed"]);
    expect(uiErrors.list[1].id).toBeGreaterThan(firstId);
  });

  it("dismisses exactly the entry with the given id", () => {
    uiErrors.report("one");
    uiErrors.report("two");
    const [one, two] = uiErrors.list;
    uiErrors.dismiss(one.id);
    expect(uiErrors.list).toEqual([two]);
    uiErrors.dismiss(one.id);
    expect(uiErrors.list).toEqual([two]);
  });
});

describe("run", () => {
  beforeEach(() => {
    for (const entry of uiErrors.list) uiErrors.dismiss(entry.id);
  });

  it("resolves the action's value and reports nothing on success", async () => {
    await expect(run("Launch", () => Promise.resolve("item-1"))).resolves.toBe("item-1");
    expect(uiErrors.list).toEqual([]);
  });

  it("turns a rejection into a labelled message and resolves undefined", async () => {
    await expect(run("Launch", () => Promise.reject(new Error("no such tool")))).resolves.toBeUndefined();
    expect(uiErrors.list.map((entry) => entry.message)).toEqual(["Launch failed: no such tool"]);
  });

  it("reports a native command's string error verbatim after the label", async () => {
    // Tauri rejects invoke() with the command's Err(String) as a bare string.
    // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors
    mockIPC(() => Promise.reject("value out of range"));
    await run("Save setting", () => settingsSet("sidebar.width", 9000, "auto"));
    expect(uiErrors.list.map((entry) => entry.message)).toEqual(["Save setting failed: value out of range"]);
    clearMocks();
  });
});
