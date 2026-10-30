// Header comment kept apart from the declaration.

/**
 * Documented.
 */
@sealed
export class Thing {
  /** Field doc. */
  value = 1; // trailing

  // leading line
  // leading line two
  method(): number {
    return this.value;
  }
}

export const a = 1,
  b = 2;

/** One. */
export const single = 3; // after single

function sealed(target: unknown): void {
  void target;
}

export enum Color {
  Red, // red
  Green,
}
