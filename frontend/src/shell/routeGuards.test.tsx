import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { AuthStatus } from "@/core/auth";
import { useAuthStore } from "@/core/auth";
import { useUiTransient } from "@/core/uiTransient";

import { indexTarget } from "./indexTarget";
import { LoginRedirect, Protected } from "./routeGuards";

function setStatus(status: AuthStatus) {
  useAuthStore.setState({
    status,
    user: status === "authenticated" ? { id: 1, username: "dm" } : null,
  });
}

beforeEach(() => {
  useUiTransient.setState({ loginOpen: false });
});

afterEach(() => {
  useAuthStore.setState({ status: "unknown", user: null });
  useUiTransient.setState({ loginOpen: false });
});

describe("indexTarget", () => {
  it("maps auth status to the right index target", () => {
    expect(indexTarget("unknown")).toBe("spinner");
    expect(indexTarget("authenticated")).toBe("console");
    expect(indexTarget("anonymous")).toBe("tv");
  });
});

describe("Protected", () => {
  it("renders the gated content when authenticated", () => {
    setStatus("authenticated");
    render(
      <MemoryRouter>
        <Protected>
          <div>secret content</div>
        </Protected>
      </MemoryRouter>,
    );
    expect(screen.getByText("secret content")).toBeInTheDocument();
  });

  it("shows a spinner (not the gate, no redirect) while auth is unknown", () => {
    setStatus("unknown");
    render(
      <MemoryRouter>
        <Protected>
          <div>secret content</div>
        </Protected>
      </MemoryRouter>,
    );
    expect(screen.getByRole("status")).toBeInTheDocument();
    expect(screen.queryByText("secret content")).not.toBeInTheDocument();
    expect(screen.queryByText(/sign in required/i)).not.toBeInTheDocument();
  });

  it("shows an inline sign-in gate (URL doesn't move) when anonymous; its button opens the modal", async () => {
    setStatus("anonymous");
    render(
      <MemoryRouter>
        <Protected>
          <div>secret content</div>
        </Protected>
      </MemoryRouter>,
    );
    expect(screen.getByText(/sign in required/i)).toBeInTheDocument();
    expect(screen.queryByText("secret content")).not.toBeInTheDocument();
    expect(useUiTransient.getState().loginOpen).toBe(false);

    await userEvent.click(screen.getByRole("button", { name: /sign in/i }));
    expect(useUiTransient.getState().loginOpen).toBe(true);
  });
});

describe("LoginRedirect", () => {
  it("opens the login modal for an anonymous visitor (back-compat /login)", () => {
    setStatus("anonymous");
    render(
      <MemoryRouter initialEntries={["/login"]}>
        <LoginRedirect />
      </MemoryRouter>,
    );
    expect(useUiTransient.getState().loginOpen).toBe(true);
  });

  it("does NOT open the modal for an already-authenticated visitor", () => {
    setStatus("authenticated");
    render(
      <MemoryRouter initialEntries={["/login"]}>
        <LoginRedirect />
      </MemoryRouter>,
    );
    expect(useUiTransient.getState().loginOpen).toBe(false);
  });
});
