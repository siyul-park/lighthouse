import { make, type Shape } from "./types";

export function sides(): number {
  const shape: Shape = make();
  return shape.sides;
}
