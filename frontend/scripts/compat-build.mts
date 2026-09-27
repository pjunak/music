import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { stripTypeScriptTypes } from "node:module";

import { parse } from "acorn";
import type { Plugin, ResolvedConfig } from "vite";

export const COMPAT_MODE_URL = "/compat-mode.js";
export const WATCHDOG_MARKER = '<script data-compat-watchdog></script>';

export interface CompatAssets {
  compatMode: string;
  bootWatchdog: string;
}

function generateClassicScript(sourcePath: string): string {
  const source = readFileSync(sourcePath, "utf8");
  const generated = stripTypeScriptTypes(source, { mode: "strip" });
  parse(generated, { ecmaVersion: 5 });
  return generated;
}

export function generateCompatAssets(frontendRoot = process.cwd()): CompatAssets {
  const compatRoot = resolve(frontendRoot, "compat");
  return {
    compatMode: generateClassicScript(resolve(compatRoot, "compat-mode.ts")),
    bootWatchdog: generateClassicScript(resolve(compatRoot, "boot-watchdog.ts")),
  };
}

export function compatBuildPlugin(): Plugin {
  let resolvedConfig: ResolvedConfig;

  function assets(): CompatAssets {
    return generateCompatAssets(resolvedConfig.root);
  }

  return {
    name: "music-compat-build",
    enforce: "pre",
    configResolved(config) {
      resolvedConfig = config;
    },
    configureServer(server) {
      server.middlewares.use(COMPAT_MODE_URL, function (_request, response) {
        try {
          response.statusCode = 200;
          response.setHeader("Content-Type", "text/javascript; charset=utf-8");
          response.end(assets().compatMode);
        } catch (error) {
          server.config.logger.error(
            error instanceof Error ? error.message : String(error),
          );
          response.statusCode = 500;
          response.end("Compatibility script generation failed");
        }
      });
    },
    transformIndexHtml(html) {
      if (!html.includes(WATCHDOG_MARKER)) {
        throw new Error(`index.html must contain ${WATCHDOG_MARKER}`);
      }
      return html.replace(
        WATCHDOG_MARKER,
        `<script>${assets().bootWatchdog}</script>`,
      );
    },
    generateBundle() {
      this.emitFile({
        type: "asset",
        fileName: COMPAT_MODE_URL.slice(1),
        source: assets().compatMode,
      });
    },
  };
}
