export enum Mode {
  Read = "read",
  Write = "write",
}

export interface Reader {
  readonly path: string;
  open?: boolean;
  read(size: number): string;
  get size(): number;
}

export type Options = {
  mode: Mode;
  timeout?: number;
  run(): void;
};

export class Handler {
  handle = (event: string): string => event;
  static defaults: Options = { mode: Mode.Read, run() {} };
  callback: () => void = () => {};
}
