import { describe, expect, it } from "vitest";

import { isInteractiveTarget } from "./isInteractiveTarget";

describe("isInteractiveTarget", () => {
  it("treats native text-entry controls as interactive", () => {
    expect(isInteractiveTarget(document.createElement("input"))).toBe(true);
    expect(isInteractiveTarget(document.createElement("textarea"))).toBe(true);
    expect(isInteractiveTarget(document.createElement("select"))).toBe(true);
  });

  it("treats an ARIA slider (the seek bar) as interactive", () => {
    const slider = document.createElement("div");
    slider.setAttribute("role", "slider");
    expect(isInteractiveTarget(slider)).toBe(true);
  });

  it("treats a contentEditable element as interactive", () => {
    const editable = document.createElement("div");
    editable.contentEditable = "true";
    // jsdom doesn't always derive isContentEditable from the attribute, so
    // assert the property the guard actually reads.
    Object.defineProperty(editable, "isContentEditable", {
      value: true,
      configurable: true,
    });
    expect(isInteractiveTarget(editable)).toBe(true);
  });

  it("treats a plain div as non-interactive", () => {
    expect(isInteractiveTarget(document.createElement("div"))).toBe(false);
  });

  it("returns false for null", () => {
    expect(isInteractiveTarget(null)).toBe(false);
  });

  it("returns false for a non-HTMLElement target", () => {
    expect(isInteractiveTarget(window as unknown as EventTarget)).toBe(false);
  });
});
