import Legacy from "./legacy.cjs";
import { fromCjs } from "./shared.cjs";
import { fromEsm } from "./esm.mjs";

export function both(): string {
  return new Legacy().run() + fromCjs() + fromEsm();
}
