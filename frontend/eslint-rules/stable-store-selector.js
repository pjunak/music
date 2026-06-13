/**
 * Custom ESLint rule: `local/stable-store-selector`.
 *
 * Flags zustand selectors that return a freshly-built array/object on every
 * call — the exact footgun that loops `useSyncExternalStore` to React #185
 * ("Maximum update depth exceeded") when the underlying state is null:
 *
 *     usePlayerStore((s) => s.state?.x ?? [])     // ← fresh [] each call
 *     useUiStore((s) => ({ a: s.a, b: s.b }))     // ← fresh {} each call
 *
 * The fix is to default OUTSIDE the selector (return the raw ref / a primitive,
 * apply `?? []` after the call) or wrap with `useShallow`. See the comment in
 * PlayerView.tsx and CLAUDE.md ("Stable zustand selectors").
 *
 * Precision over recall: we only flag the two unambiguous cases (a literal, or
 * a `??`/`||` whose default is a literal). A `useShallow(...)` first arg is a
 * CallExpression, not an arrow, so the safe shallow-equality pattern is allowed.
 */

function unstableReturn(node) {
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

export default {
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
    function report(expr) {
      const kind = unstableReturn(expr);
      if (kind !== null) {
        context.report({ node: expr, messageId: "unstable", data: { kind } });
      }
    }
    return {
      CallExpression(node) {
        const callee = node.callee;
        // Only `useXxxStore(...)` calls — the zustand hook convention.
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
          // Explicit-return selector: check each top-level `return`.
          for (const stmt of arg.body.body) {
            if (stmt.type === "ReturnStatement") report(stmt.argument);
          }
        } else {
          // Implicit-return arrow: the body IS the returned expression.
          report(arg.body);
        }
      },
    };
  },
};
