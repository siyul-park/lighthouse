import main, { VERSION, toSlug as s } from "./index";
import * as util from "./util";
import type { Id } from "./util";
import "./side";
import { readFileSync } from "node:fs";

export const lazy = (): Promise<typeof util> => import("./util");

export function use(id: Id): string {
  readFileSync("x");
  return s(util.slug(id)) + VERSION + main();
}
