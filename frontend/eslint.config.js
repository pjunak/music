import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import globals from "globals";
import tseslint from "typescript-eslint";

import stableStoreSelector from "./eslint-rules/stable-store-selector.js";

export default tseslint.config(
  { ignores: ["dist", "node_modules", ".vite", "*.config.js"] },
  {
    files: ["**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
      // Local rule: forbid the React #185 unstable-selector footgun.
      local: { rules: { "stable-store-selector": stableStoreSelector } },
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // v7 folded the React-Compiler rules into `recommended`. They flag
      // intentional patterns (prop→local-state mirrors, etc.) across the app;
      // keep the classic rules-of-hooks + exhaustive-deps contract and adopt
      // these deliberately as their own refactor rather than as a lint bump.
      "react-hooks/set-state-in-effect": "off",
      "react-hooks/use-memo": "off",
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
      "@typescript-eslint/consistent-type-imports": "error",
      "local/stable-store-selector": "error",
    },
  },
);
