import ts from "typescript";
import type { Lines } from "./extent/lines.ts";
import type { Comment, Symbol as Sym } from "./sdk/wire.ts";

interface Range {
  pos: number;
  end: number;
  multiline: boolean;
}

/** Every comment of the file, with adjacent line comments joined, and the symbol each documents. */
export function commentsOf(file: ts.SourceFile, lines: Lines, symbols: readonly Sym[]): Comment[] {
  const startingAt = new Map([...symbols].reverse().map((symbol) => [symbol.span.start.line, symbol.id]));
  const text = file.text;
  return groupRanges(lines, ranges(file)).map((group): Comment => {
    const from = group[0]!.pos;
    const to = group[group.length - 1]!.end;
    const span = lines.span(from, to);
    const lineStart = lines.starts[span.start.line - 1] ?? 0;
    const alone = text.slice(lineStart, from).trim() === "";
    const attached = alone ? startingAt.get(span.end.line + 1) : undefined;
    return {
      span,
      text: text.slice(from, to).replaceAll("\r\n", "\n"),
      ...(attached === undefined ? {} : { attached_to: attached }),
    };
  });
}

/** All comment ranges of a file, by position: those in the gaps between its tokens. */
function ranges(file: ts.SourceFile): Range[] {
  const found = new Map<number, Range>();
  const add = (start: number, end: number, kind: ts.CommentKind): void => {
    found.set(start, { pos: start, end, multiline: kind === ts.SyntaxKind.MultiLineCommentTrivia });
  };
  const collect = (pos: number): void => {
    ts.forEachLeadingCommentRange(file.text, pos, add);
    ts.forEachTrailingCommentRange(file.text, pos, add);
  };
  const visit = (node: ts.Node): void => {
    if (node.kind >= ts.SyntaxKind.FirstJSDocNode && node.kind <= ts.SyntaxKind.LastJSDocNode) return;
    const children = node.getChildren(file);
    if (children.length === 0) {
      if (node.kind !== ts.SyntaxKind.JsxText && node.kind !== ts.SyntaxKind.JsxTextAllWhiteSpaces) collect(node.pos);
      return;
    }
    for (const child of children) visit(child);
  };
  visit(file);
  return [...found.values()].sort((a, b) => a.pos - b.pos);
}

/** Groups adjacent line comments: each on a line of its own, on consecutive lines. */
function groupRanges(lines: Lines, all: readonly Range[]): Range[][] {
  const groups: Range[][] = [];
  let previous: Range | undefined;
  for (const range of all) {
    const joins =
      previous !== undefined &&
      !range.multiline &&
      !previous.multiline &&
      standsAlone(lines, range) &&
      standsAlone(lines, previous) &&
      lines.line(range.pos) === lines.line(previous.end) + 1;
    if (joins) groups[groups.length - 1]!.push(range);
    else groups.push([range]);
    previous = range;
  }
  return groups;
}

/** Whether a comment stands on a line with nothing before it. */
function standsAlone(lines: Lines, range: Range): boolean {
  const start = lines.starts[lines.line(range.pos) - 1] ?? 0;
  return lines.text.slice(start, range.pos).trim() === "";
}
