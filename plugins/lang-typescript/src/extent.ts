import ts from "typescript";
import type { Decl } from "./decls.ts";
import type { Lines } from "./positions.ts";
import type { Span } from "./wire.ts";

/** The whole of a declaration: the comments that lead it without a blank line, through its last token and a comment that trails it. */
export function extentOf(decl: Decl, file: ts.SourceFile, lines: Lines): Span | undefined {
  const nodes = extentNodes(decl);
  if (nodes === undefined) return undefined;
  const first = nodes[0]!;
  const last = nodes[nodes.length - 1]!;
  let from = first.getStart(file);
  let to = last.end;
  const leading = ts.getLeadingCommentRanges(file.text, first.pos) ?? [];
  for (let i = leading.length - 1; i >= 0; i--) {
    const comment = leading[i]!;
    if (lines.line(comment.end) + 1 >= lines.line(from)) from = comment.pos;
    else break;
  }
  for (const comment of ts.getTrailingCommentRanges(file.text, last.end) ?? []) to = Math.max(to, comment.end);
  return lines.span(from, to);
}

/** The nodes an extent runs over: all the overloads of a function, else the first declaration alone. */
function extentNodes(decl: Decl): ts.Node[] | undefined {
  const marked = decl.extents;
  if (marked.length === 0 || marked[0] === undefined) return undefined;
  const sameKind = decl.nodes.every((node) => node.kind === decl.nodes[0]!.kind);
  if ((decl.kind === "function" || decl.kind === "method") && sameKind && marked.every((node) => node !== undefined)) {
    return marked as ts.Node[];
  }
  return [marked[0]];
}

/** The documentation comment of a declaration: the last JSDoc block before it, without its markers. */
export function docOf(decl: Decl, file: ts.SourceFile): string | undefined {
  const node = decl.extents[0] ?? decl.node;
  const docs = ts.getJSDocCommentsAndTags(node).filter(ts.isJSDoc);
  const last = docs[docs.length - 1];
  if (last === undefined || last.getSourceFile() !== file) return undefined;
  const raw = file.text.slice(last.getStart(file), last.end);
  const body = raw
    .replace(/^\/\*\*+/, "")
    .replace(/\*+\/$/, "")
    .split(/\r?\n/)
    .map((line) => line.replace(/^\s*\*? ?/, ""))
    .join("\n")
    .trim();
  return body === "" ? undefined : body;
}
