import type { Position, Span } from "./wire.ts";

const NON_ASCII = /[^\u0000-\u007f]/;

/** The lines of a text. A column counts UTF-8 bytes, as the protocol says. */
export class Lines {
  readonly text: string;
  readonly starts: number[];
  private readonly ascii: boolean;

  constructor(text: string) {
    this.text = text;
    this.ascii = !NON_ASCII.test(text);
    const starts = [0];
    for (let i = 0; i < text.length; i++) {
      if (text.charCodeAt(i) === 10) starts.push(i + 1);
    }
    this.starts = starts;
  }

  /** The 1-based line and byte column of a UTF-16 offset. */
  position(offset: number): Position {
    const at = Math.max(0, Math.min(offset, this.text.length));
    let low = 0;
    let high = this.starts.length - 1;
    while (low < high) {
      const mid = (low + high + 1) >> 1;
      if ((this.starts[mid] ?? 0) <= at) low = mid;
      else high = mid - 1;
    }
    const start = this.starts[low] ?? 0;
    const col = this.ascii ? at - start : Buffer.byteLength(this.text.slice(start, at));
    return { line: low + 1, col: col + 1 };
  }

  span(start: number, end: number): Span {
    return { start: this.position(start), end: this.position(Math.max(start, end)) };
  }

  /** The 1-based line of an offset. */
  line(offset: number): number {
    return this.position(offset).line;
  }
}

/** The byte offsets at which the lines of a text begin. */
export function byteLineStarts(text: Buffer): number[] {
  const starts = [0];
  for (let i = 0; i < text.length; i++) {
    if (text[i] === 10) starts.push(i + 1);
  }
  return starts;
}

/** The position of a byte offset, given the byte starts of the lines. */
export function positionAtByte(starts: readonly number[], offset: number): Position {
  let low = 0;
  let high = starts.length - 1;
  while (low < high) {
    const mid = (low + high + 1) >> 1;
    if ((starts[mid] ?? 0) <= offset) low = mid;
    else high = mid - 1;
  }
  return { line: low + 1, col: offset - (starts[low] ?? 0) + 1 };
}
