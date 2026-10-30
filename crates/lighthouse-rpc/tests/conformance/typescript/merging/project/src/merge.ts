export class Account {
  id = 1;
}

export interface Account {
  label: string;
}

export namespace Account {
  export const limit = 10;
  export function make(): Account {
    return new Account() as Account;
  }
}

export enum Level {
  Low,
  High,
}

export namespace Level {
  export function parse(text: string): Level {
    return text === "high" ? Level.High : Level.Low;
  }
}

export function counter(): number {
  return counter.count++;
}

export namespace counter {
  export let count = 0;
}

export interface Options {
  a: number;
}

export interface Options {
  b?: string;
}
