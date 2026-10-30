/** Parses. */
export function parse(input: string): number;
export function parse(input: number): string;
export function parse(input: string | number): number | string {
  return typeof input === "string" ? Number(input) : String(input);
}

export class Writer {
  constructor(path: string);
  constructor(fd: number);
  constructor(readonly target: string | number) {}

  write(text: string): void;
  write(bytes: Uint8Array): void;
  write(data: string | Uint8Array): void {
    void data;
  }
}

export function use(): void {
  parse("1");
  new Writer(1).write("x");
}
