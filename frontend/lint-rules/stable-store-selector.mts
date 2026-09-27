import type { Rule } from "eslint";
import type { Expression, Super } from "estree";

/**
 * Custom lint rule: `local/stable-store-selector`.
 *
 * Flags zustand selectors that return a freshly-built array/object on every
 * call — the exact footgun that loops `useSyncExternalStore` to React #185
 * ("Maximum update depth exceeded") when the underlying state is null:
 *
 *     usePlayerStore((s) => s.state?.x ?? [])     // fresh [] each call
 *     useUiStore((s) => ({ a: s.a, b: s.b }))     // fresh {} each call
 *
 * The rule uses the ESLint-compatible plugin API implemented by Oxlint.
 */

function unstableReturn(node: Expression | Super | null | undefined): "array" | "object" | null {
  if (node === null || node === undefined) return null;
  if (node.type === "ArrayExpression") return "array";
  if (node.type === "ObjectExpression") return "object";
  if (
    node.type === "LogicalExpression" &&
    (node.operator === "??" || node.operator === "||")
  ) {
    if (node.right.type === "ArrayExpression") return "array";
    if (node.right.type === "ObjectExpression") return "object";
  }
  return null;
}

const stableStoreSelector: Rule.RuleModule = {
  meta: {
    type: "problem",
    docs: {
      description:
        "Disallow zustand selectors that return a fresh array/object (React #185 loop risk when state is null).",
    },
    schema: [],
    messages: {
      unstable:
        "Selector returns a fresh {{kind}} on every call; when the store value is null this loops useSyncExternalStore to React #185. Default OUTSIDE the selector (e.g. `useXStore((s) => s.state?.x) ?? []`) or wrap with `useShallow`.",
    },
  },
  create(context) {
    function report(expr: Expression | Super | null | undefined) {
      if (expr == null) return;
      const kind = unstableReturn(expr);
      if (kind !== null) {
        context.report({ node: expr, messageId: "unstable", data: { kind } });
      }
    }

    return {
      CallExpression(node) {
        const callee = node.callee;
        if (callee.type !== "Identifier" || !/^use.*Store$/.test(callee.name)) {
          return;
        }

        const arg = node.arguments[0];
        if (
          arg === undefined ||
          (arg.type !== "ArrowFunctionExpression" &&
            arg.type !== "FunctionExpression")
        ) {
          return;
        }

        if (arg.body.type === "BlockStatement") {
          for (const statement of arg.body.body) {
            if (statement.type === "ReturnStatement") report(statement.argument);
          }
        } else {
          report(arg.body);
        }
      },
    };
  },
};

export default stableStoreSelector;
