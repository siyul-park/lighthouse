import { thing } from "missing-package";
import { gone } from "./gone";
import { shared } from "./shared";

export function run(x: any): number {
  thing();
  gone.call();
  x.dynamic();
  x.field;
  return shared.value;
}
