import { Suspense } from "react";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { lazyNamed } from "./lazyNamed";

function LoadedView() {
  return <p>Loaded named view</p>;
}

describe("lazyNamed", () => {
  it("selects and renders a named component from a dynamic module", async () => {
    const DeferredView = lazyNamed(
      async () => ({ LoadedView, unrelated: "value" }),
      (module) => module.LoadedView,
    );

    render(
      <Suspense fallback={<p>Loading view</p>}>
        <DeferredView />
      </Suspense>,
    );

    expect(await screen.findByText("Loaded named view")).toBeInTheDocument();
  });
});
