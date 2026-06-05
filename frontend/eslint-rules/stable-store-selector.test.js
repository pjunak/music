import { RuleTester } from "eslint";
import { describe, it } from "vitest";

import rule from "./stable-store-selector.js";

// RuleTester drives the test framework via global describe/it (vitest globals).
RuleTester.describe = describe;
RuleTester.it = it;

const ruleTester = new RuleTester({
  languageOptions: { ecmaVersion: 2022, sourceType: "module" },
});

ruleTester.run("stable-store-selector", rule, {
  valid: [
    // Raw ref from the selector — the stable, correct pattern.
    "usePlayerStore((s) => s.state?.x);",
    // Default applied OUTSIDE the selector call is fine.
    "usePlayerStore((s) => s.state?.x) ?? [];",
    // Named selector — not analysable, not our concern.
    "usePlayerStore(selectThing);",
    // Primitive defaults are stable.
    "usePlayerStore((s) => s.count ?? 0);",
    "useUiStore((s) => s.name ?? null);",
    // useShallow makes an object selector safe — its arg is a CallExpression.
    "useStore(useShallow((s) => ({ a: s.a, b: s.b })));",
    // Not a store hook.
    "useMemo(() => [], []);",
  ],
  invalid: [
    {
      code: "usePlayerStore((s) => s.state?.x ?? []);",
      errors: [{ messageId: "unstable" }],
    },
    {
      code: "useUiStore((s) => ({ a: s.a, b: s.b }));",
      errors: [{ messageId: "unstable" }],
    },
    {
      code: "useStore((s) => []);",
      errors: [{ messageId: "unstable" }],
    },
    {
      code: "useDiagStore((s) => s.list || {});",
      errors: [{ messageId: "unstable" }],
    },
    {
      code: "usePlayerStore((s) => { return s.state?.x ?? []; });",
      errors: [{ messageId: "unstable" }],
    },
  ],
});
