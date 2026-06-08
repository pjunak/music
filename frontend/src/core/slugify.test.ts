import { describe, expect, it } from "vitest";

import { slugify, uniqueSlug } from "./slugify";

const SLUG_RE = /^[a-z0-9][a-z0-9_-]*$/;

describe("slugify", () => {
  it("lowercases and dashes spaces", () => {
    expect(slugify("Deep Forest")).toBe("deep-forest");
  });

  it("collapses runs of separators into a single dash", () => {
    expect(slugify("Tavern   Brawl!! (loud)")).toBe("tavern-brawl-loud");
  });

  it("folds accents", () => {
    expect(slugify("Café del Mar")).toBe("cafe-del-mar");
  });

  it("trims leading/trailing separators so it starts alphanumeric", () => {
    expect(slugify("  --Boss Fight--  ")).toBe("boss-fight");
    expect(slugify("Boss Fight")).toMatch(SLUG_RE);
  });

  it("returns empty for a name with no alphanumerics", () => {
    expect(slugify("!!!")).toBe("");
    expect(slugify("🎲🎲")).toBe("");
  });

  it("keeps digits and existing dashes", () => {
    expect(slugify("Round 2 - Final")).toBe("round-2-final");
  });

  it("caps length and re-trims a dangling dash", () => {
    const long = "word ".repeat(40); // far over 64 chars
    const s = slugify(long);
    expect(s.length).toBeLessThanOrEqual(64);
    expect(s).toMatch(SLUG_RE);
    expect(s.endsWith("-")).toBe(false);
  });
});

describe("uniqueSlug", () => {
  it("returns the bare slug when free", () => {
    expect(uniqueSlug("Deep Forest", new Set())).toBe("deep-forest");
  });

  it("suffixes -2, -3 on collisions", () => {
    expect(uniqueSlug("Deep Forest", new Set(["deep-forest"]))).toBe("deep-forest-2");
    expect(
      uniqueSlug("Deep Forest", new Set(["deep-forest", "deep-forest-2"])),
    ).toBe("deep-forest-3");
  });

  it("accepts an array as the existing set", () => {
    expect(uniqueSlug("Deep Forest", ["deep-forest"])).toBe("deep-forest-2");
  });

  it("falls back when the name slugifies to empty", () => {
    expect(uniqueSlug("???", new Set(), "preset")).toBe("preset");
    expect(uniqueSlug("???", new Set(["preset"]), "preset")).toBe("preset-2");
  });

  it("always yields a valid slug", () => {
    for (const name of ["Deep Forest", "!!!", "Café", "🎲", "  x  "]) {
      expect(uniqueSlug(name, new Set(), "item")).toMatch(SLUG_RE);
    }
  });

  it("stays within the length cap even when suffixing a long name", () => {
    const long = "supercalifragilistic ".repeat(8); // well over 64 chars
    const base = slugify(long);
    const out = uniqueSlug(long, new Set([base]), "item");
    expect(out.length).toBeLessThanOrEqual(64);
    expect(out).toMatch(SLUG_RE);
    expect(out).not.toBe(base); // distinct from the colliding entry
  });
});
