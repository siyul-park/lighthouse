import ts from "typescript";
import { Lines } from "./positions.ts";

/** The reason a source file is not fully analysed: its first syntax error, with its position. */
export function syntaxProblem(program: ts.Program, file: ts.SourceFile): string | undefined {
  const [first, ...rest] = program.getSyntacticDiagnostics(file);
  if (first === undefined) return undefined;
  const at = new Lines(file.text).position(first.start ?? 0);
  const more = rest.length === 0 ? "" : ` (and ${rest.length} more)`;
  return `${at.line}:${at.col}: ${ts.flattenDiagnosticMessageText(first.messageText, "\n")}${more}`;
}
