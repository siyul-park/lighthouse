export interface Named {
  name: string;
}

/** A box of one value. */
export class Box<T extends Named> {
  value?: T;

  constructor(value: T) {
    this.value = value;
  }

  map<U extends Named>(change: (value: T) => U): Box<U> {
    return new Box(change(this.value as T));
  }
}

export type Pair<A, B> = { first: A; second?: B | null };

export function first<T>(items: T[]): T | undefined {
  return items[0];
}

export function describe<T extends Named>(box: Box<T>): string {
  return box.value ? box.value.name : "";
}

export const identity = <T,>(value: T): T => value;
