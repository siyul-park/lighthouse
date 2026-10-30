import { sub } from "../calc";

describe("sub", () => {
  it("subtracts", () => {
    if (sub(3, 1) !== 2) throw new Error("no");
  });
});
