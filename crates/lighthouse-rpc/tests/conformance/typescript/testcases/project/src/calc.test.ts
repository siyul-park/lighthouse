import { describe, expect, it, test } from "vitest";
import { add, Calc } from "./calc";

const rows = [
  { a: 1, b: 2, sum: 3 },
  { a: 2, b: 3, sum: 5 },
];

type Row = { a: number };

function check(value: number): void {
  expect(value).toBeGreaterThan(0);
}

const FIXTURE = { a: 1 };

describe("add", () => {
  it("adds", () => {
    check(add(1, 2));
  });

  describe("nested", () => {
    it("adds again", () => {
      expect(add(2, 2)).toBe(4);
    });
  });
});

test("Calc", () => {
  expect(new Calc().mul(2, 3)).toBe(6);
});

it("table", () => {
  for (const row of rows) {
    expect(add(row.a, row.b)).toBe(row.sum);
  }
});

it.each([[1, 2]])("each %i", (a, b) => {
  expect(add(a, b)).toBe(3);
});

export type { Row };
export { FIXTURE };
