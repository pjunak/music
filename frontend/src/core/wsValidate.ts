import type { WsMessage } from "@/core/types";

/** Cheap structural validation for WsMessage frames. We don't ship a
 *  Zod-style schema runtime — the contract surface is small and changes
 *  rarely, so hand-rolled discriminator + key-field checks are enough.
 *
 *  The point is to catch protocol drift loudly: if the server starts
 *  sending an unknown `type` or omits a required field, log it once and
 *  drop the frame instead of letting downstream listeners hit
 *  `state.ambient.current_track_id` on `undefined` and tear down the UI.
 *
 *  Returns the typed message on success, or `null` if the payload doesn't
 *  match a known shape. */

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function isString(v: unknown): v is string {
  return typeof v === "string";
}

function isNumber(v: unknown): v is number {
  return typeof v === "number" && Number.isFinite(v);
}

/** Spot-check a few load-bearing PlayerState fields. Doesn't deep-validate
 *  every nested object — just enough to assert "looks like PlayerState
 *  rather than something else entirely" so the audio engine and UI don't
 *  blow up on missing top-level keys. */
function looksLikePlayerState(v: unknown): boolean {
  if (!isObject(v)) return false;
  if (!isObject(v.ambient)) return false;
  if (!isObject(v.connected_devices) && !Array.isArray(v.connected_devices)) {
    return false;
  }
  if (!Array.isArray(v.active_output_device_ids)) return false;
  if (typeof v.is_playing !== "boolean") return false;
  if (!isNumber(v.volume)) return false;
  return true;
}

export function validateWsMessage(raw: unknown): WsMessage | null {
  if (!isObject(raw)) return null;
  const t = raw.type;
  switch (t) {
    case "state_snapshot":
      if (!isString(raw.your_device_id)) return null;
      if (!looksLikePlayerState(raw.state)) return null;
      return raw as WsMessage;
    case "state_changed":
      if (!looksLikePlayerState(raw.state)) return null;
      return raw as WsMessage;
    case "sfx_fired":
      if (!isString(raw.soundboard_id)) return null;
      if (!isString(raw.item_path)) return null;
      if (!isNumber(raw.volume)) return null;
      return raw as WsMessage;
    case "error":
      if (!isString(raw.detail)) return null;
      return raw as WsMessage;
    default:
      return null;
  }
}
