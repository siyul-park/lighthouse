import ts from "typescript";
import type { Lines } from "./extent/lines.ts";
import type { Comment, Symbol as Sym } from "./sdk/wire.ts";

const GENERATED = /@generated\b/;

interface Range {
  pos: number;
  end: number;
  multiline: boolean;
}

/** Every comment of the file, with adjacent line comments joined, and the symbol each documents. */
export function commentsOf(file: ts.SourceFile, lines: Lines, symbols: readonly Sym[]): Comment[] {
  const startingAt = new Map<number, string>();
  for (const symbol of symbols) {
    if (!startingAt.has(symbol.span.start.line)) startingAt.set(symbol.span.start.line, symbol.id);
  }
  const text = file.text;
  const comments: Comment[] = [];
  for (const group of groupRanges(lines, ranges(file))) {
    const from = group[0]!.pos;
    const to = group[group.length - 1]!.end;
    const span = lines.span(from, to);
    const lineStart = lines.starts[span.start.line - 1] ?? 0;
    const alone = text.slice(lineStart, from).trim() === "";
    const attached = alone ? startingAt.get(span.end.line + 1) : undefined;
    comments.push({
      span,
      text: text.slice(from, to).replaceAll("\r\n", "\n"),
      ...(attached === undefined ? {} : { attached_to: attached }),
    });
  }
  return comments;
}

/** Whether a comment above the first statement marks the file as generated. */
export function isGenerated(file: ts.SourceFile): boolean {
  const first = file.statements[0];
  const end = first === undefined ? file.text.length : first.getStart(file);
  for (const range of ranges(file)) {
    if (range.pos >= end) break;
    const text = file.text.slice(range.pos, range.end);
    if (GENERATED.test(text) || text.toUpperCase().includes("DO NOT EDIT")) return true;
  }
  return false;
}

/** All comment ranges of a file, by position: those in the gaps between its tokens. */
function ranges(file: ts.SourceFile): Range[] {
  const found = new Map<number, Range>();
  const collect = (pos: number): void => {
    const add = (start: number, end: number, kind: ts.CommentKind): void => {
      found.set(start, { pos: start, end, multiline: kind === ts.SyntaxKind.MultiLineCommentTrivia });
    };
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
