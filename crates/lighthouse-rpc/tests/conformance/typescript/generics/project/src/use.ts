import { Box, first, identity, type Named, type Pair } from "./box";

export function run(items: Named[]): Pair<Named, Box<Named>> {
  const head = first(items) ?? identity({ name: "x" });
  const box = new Box(head);
  return { first: head, second: box.map((v) => v) };
}
