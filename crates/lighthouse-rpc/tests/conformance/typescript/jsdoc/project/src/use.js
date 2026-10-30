import { Grid, move } from "./shapes.js";

export function run() {
  const grid = new Grid(2);
  return move(grid.cells[0], grid.count());
}
