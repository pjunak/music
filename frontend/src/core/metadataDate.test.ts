import { describe, expect, it } from "vitest";
import { trackReleaseDate, validMetadataDate } from "./metadataDate";
describe("metadata calendar dates", () => {
  it("accepts partial precision and real leap days", () => {
    for (const date of ["0001", "2025", "2025-10", "2024-02-29", "2000-02-29"]) expect(validMetadataDate(date), date).toBe(true);
    for (const date of ["", "0000", "2025-2", "2025-02-29", "1900-02-29", "2025-04-31", "2025-00", "2025-13", "2025-10-00", " 2025", "2025-10-17T00:00:00Z"]) expect(validMetadataDate(date), date).toBe(false);
  });
  it("uses old-server years without overriding explicit dates or clears", () => {
    expect(trackReleaseDate({ year: 2024 })).toBe("2024");
    expect(trackReleaseDate({ year: 2024, release_date: "2024-02-29" })).toBe("2024-02-29");
    expect(trackReleaseDate({ year: 2024, release_date: "" })).toBe("");
  });
});
