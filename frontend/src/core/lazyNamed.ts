import { lazy } from "react";
import type { ComponentType, LazyExoticComponent } from "react";

/** Lazily load a named component without adding one-off default-export wrappers. */
export function lazyNamed<Module, Props>(
  load: () => Promise<Module>,
  select: (module: Module) => ComponentType<Props>,
): LazyExoticComponent<ComponentType<Props>> {
  return lazy(async () => ({ default: select(await load()) }));
}
