import { describe, expect, it } from "vitest";

import { deviceDisplayName, parseDeviceVisual } from "./deviceVisual";

describe("parseDeviceVisual", () => {
  it("maps the auto-generated platform names to class + OS", () => {
    expect(parseDeviceVisual("Windows PC · Firefox")).toEqual({
      klass: "desktop",
      os: "windows",
    });
    expect(parseDeviceVisual("Mac · Chrome")).toEqual({
      klass: "desktop",
      os: "apple",
    });
    expect(parseDeviceVisual("Linux · Firefox")).toEqual({
      klass: "desktop",
      os: "linux",
    });
    expect(parseDeviceVisual("iPhone · Safari")).toEqual({
      klass: "phone",
      os: "apple",
    });
    expect(parseDeviceVisual("Android phone · Chrome")).toEqual({
      klass: "phone",
      os: "android",
    });
    // The more specific "tablet" keyword must win over "phone"/mobile defaults.
    expect(parseDeviceVisual("Android tablet · Chrome")).toEqual({
      klass: "tablet",
      os: "android",
    });
    expect(parseDeviceVisual("iPad · Safari")).toEqual({
      klass: "tablet",
      os: "apple",
    });
  });

  it("recognises custom device names", () => {
    expect(parseDeviceVisual("Living Room TV")).toEqual({
      klass: "tv",
      os: null,
    });
    expect(parseDeviceVisual("Kitchen Speaker")).toEqual({
      klass: "speaker",
      os: null,
    });
    // A TV wins over the OS-implied phone default even when the OS is known.
    expect(parseDeviceVisual("Android TV")).toMatchObject({ klass: "tv" });
  });

  it("falls back to unknown for opaque / empty labels", () => {
    expect(parseDeviceVisual("DM")).toEqual({ klass: "unknown", os: null });
    expect(parseDeviceVisual("")).toEqual({ klass: "unknown", os: null });
    expect(parseDeviceVisual(null)).toEqual({ klass: "unknown", os: null });
  });

  it("still parses the legacy emoji labels", () => {
    expect(parseDeviceVisual("🪟 PC · Edge")).toMatchObject({ os: "windows" });
    expect(parseDeviceVisual("🤖 phone")).toMatchObject({
      os: "android",
      klass: "phone",
    });
  });
});

describe("deviceDisplayName", () => {
  it("strips the platform half the icon already shows", () => {
    expect(deviceDisplayName("Windows PC · Firefox")).toBe("Firefox");
    expect(deviceDisplayName("iPhone · Safari")).toBe("Safari");
    expect(deviceDisplayName("Android phone · Chrome")).toBe("Chrome");
  });

  it("keeps custom and platform-only names intact", () => {
    expect(deviceDisplayName("Living Room TV")).toBe("Living Room TV");
    expect(deviceDisplayName("Windows PC")).toBe("Windows PC");
    expect(deviceDisplayName("DM")).toBe("DM");
  });

  it("falls back to 'This device' when empty", () => {
    expect(deviceDisplayName("")).toBe("This device");
    expect(deviceDisplayName(null)).toBe("This device");
  });
});
