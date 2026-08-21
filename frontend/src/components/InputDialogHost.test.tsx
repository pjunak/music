import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { InputDialogHost } from "./InputDialogHost";
import { inputDialog } from "./inputDialog";

describe("InputDialogHost", () => {
  it("uses a password field and clears its value after the dialog closes", async () => {
    const user = userEvent.setup();
    render(<InputDialogHost />);

    let passwordResult!: Promise<string | null>;
    act(() => {
      passwordResult = inputDialog({
        title: "Confirm destructive action",
        label: "Current password",
        type: "password",
      });
    });
    const password = screen.getByLabelText("Current password");
    expect(password).toHaveAttribute("type", "password");
    await user.type(password, "private-value");
    await user.click(screen.getByRole("button", { name: "OK" }));
    await expect(passwordResult).resolves.toBe("private-value");

    let textResult!: Promise<string | null>;
    act(() => {
      textResult = inputDialog({
        title: "Open another prompt",
        label: "Value",
      });
    });
    const text = screen.getByLabelText("Value");
    expect(text).toHaveAttribute("type", "text");
    expect(text).toHaveValue("");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    await expect(textResult).resolves.toBeNull();
  });
});
