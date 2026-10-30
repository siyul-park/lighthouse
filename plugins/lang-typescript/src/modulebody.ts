import ts from "typescript";
import { assignsToExports } from "./commonjs.ts";
import type { FileDecls } from "./decls.ts";

/** The name of the function that stands for the statements a module runs at its top level, as CPython names the code of a module. */
export const MODULE_BODY = "<module>";

/** The statements that run when a module loads: those that declare nothing. */
const EXECUTABLE = [
  ts.SyntaxKind.ExpressionStatement,
  ts.SyntaxKind.IfStatement,
  ts.SyntaxKind.ForStatement,
  ts.SyntaxKind.ForInStatement,
  ts.SyntaxKind.ForOfStatement,
  ts.SyntaxKind.WhileStatement,
  ts.SyntaxKind.DoStatement,
  ts.SyntaxKind.TryStatement,
  ts.SyntaxKind.SwitchStatement,
  ts.SyntaxKind.Block,
  ts.SyntaxKind.ThrowStatement,
  ts.SyntaxKind.LabeledStatement,
];

/**
 * Declares the body of a module: the one function that owns the uses its top-level statements make, so that
 * a function called only when the module loads is not mistaken for dead code. A bodyless function, it has no metrics.
 */
export function declareModuleBody(decls: FileDecls): void {
  if (decls.fence) return;
  for (const statement of decls.file.statements) {
    if (!EXECUTABLE.includes(statement.kind) || decls.covers(statement)) continue;
    if (ts.isExpressionStatement(statement) && (isDirective(statement) || assignsToExports(statement))) continue;
    decls.add({ kind: "function", name: MODULE_BODY, node: statement });
  }
}

/** Whether a statement is a directive such as `"use strict"`, which runs nothing. */
function isDirective(statement: ts.ExpressionStatement): boolean {
  return ts.isStringLiteral(statement.expression);
}
