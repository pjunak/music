import { describe, expect, it } from "vitest";

import { validateWsMessage } from "./wsValidate";

/** Sanity tests for the WS frame guard. The frontend trusts every other
 *  payload to match the discriminated union in `types.ts`; this helper is
 *  what catches a backend protocol drift before listeners see corrupted
 *  state. */

const VALID_PLAYER_STATE = {
  ambient: { current_track_id: 1, queue: [], history: [], position_ms: 0, loop: "off" },
  connected_devices: [],
  active_output_device_ids: [],
  is_playing: false,
  volume: 0.8,
  revision: 0,
};

describe("validateWsMessage", () => {
  it("accepts a well-formed state_snapshot", () => {
    const msg = validateWsMessage({
      type: "state_snapshot",
      your_device_id: "dev-abc",
      state: VALID_PLAYER_STATE,
    });
    expect(msg).not.toBeNull();
    expect(msg?.type).toBe("state_snapshot");
  });

  it("rejects a state_snapshot missing your_device_id", () => {
    expect(
      validateWsMessage({ type: "state_snapshot", state: VALID_PLAYER_STATE }),
    ).toBeNull();
  });

  it("rejects state_changed when state isn't a PlayerState shape", () => {
    expect(
      validateWsMessage({ type: "state_changed", state: { wrong: true } }),
    ).toBeNull();
  });

  it("accepts an error frame with a string detail", () => {
    expect(
      validateWsMessage({ type: "error", detail: "session expired" }),
    ).not.toBeNull();
  });

  it("rejects an unknown type", () => {
    expect(validateWsMessage({ type: "future_event", foo: 1 })).toBeNull();
  });

  it("rejects non-objects (null, arrays, primitives)", () => {
    expect(validateWsMessage(null)).toBeNull();
    expect(validateWsMessage([])).toBeNull();
    expect(validateWsMessage("hello")).toBeNull();
    expect(validateWsMessage(42)).toBeNull();
  });

  it("accepts sfx_fired with valid soundboard/item/volume", () => {
    expect(
      validateWsMessage({
        type: "sfx_fired",
        soundboard_id: "tavern",
        item_path: "dnd/door.ogg",
        volume: 0.5,
      }),
    ).not.toBeNull();
  });

  it("rejects sfx_fired with a non-numeric volume", () => {
    expect(
      validateWsMessage({
        type: "sfx_fired",
        soundboard_id: "tavern",
        item_path: "dnd/door.ogg",
        volume: "loud",
      }),
    ).toBeNull();
  });
});
