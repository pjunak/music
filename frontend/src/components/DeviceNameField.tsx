import { useState } from "react";

import { defaultDeviceName, useUiStore } from "@/core/uiStore";
import { wsClient } from "@/core/ws";

import { EditIcon } from "./icons";

/** Editable name for *this* browser session, used by the operator's
 *  Outputs picker to identify which TV / phone / desktop is which.
 *
 *  No visible legend by design — sits in the top-left corner of the app
 *  shell and identifies itself via the placeholder ("Living-room TV"
 *  etc., resolved from `defaultDeviceName()`) and a hover tooltip.
 *
 *  Commits on blur or Enter, then re-`register`s on the WS so the new
 *  name reaches the server (and any operator devices watching the
 *  outputs picker). */

export function DeviceNameField() {
  const deviceName = useUiStore((s) => s.deviceName);
  const setDeviceName = useUiStore((s) => s.setDeviceName);
  const [localName, setLocalName] = useState(deviceName ?? "");

  function commit() {
    const trimmed = localName.trim();
    const next = trimmed === "" ? null : trimmed;
    if (next !== deviceName) {
      setDeviceName(next);
      wsClient.sendRegister();
    }
  }

  return (
    <span className="device-name-field-wrap">
      <input
        className="device-name-field"
        type="text"
        value={localName}
        placeholder={defaultDeviceName()}
        onChange={(e) => setLocalName(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
        }}
        title="This device's name in the operator's outputs picker. Edit and press Enter (or click away) to apply."
        aria-label="This device's name"
      />
      <EditIcon className="device-name-field-icon" aria-hidden="true" />
    </span>
  );
}
