import ts from "typescript";
import { type FileDecls, type FunctionBody, functionInitializer, unwrap } from "./decls.ts";

const TEST_NAMES = new Set(["describe", "it", "test", "suite", "context", "specify"]);
const RUNNERS = new Set(["vitest", "@jest/globals", "mocha", "node:test", "bun:test", "@playwright/test", "ava"]);

/** A call of a test framework that declares a test, a suite or a table of them. */
export interface TestCall {
  title: string;
  callback: FunctionBody | undefined;
  each: boolean;
}

/** The test a call declares: `describe("x", fn)`, `it.skip("x", fn)`, `test.each(rows)("x", fn)`. */
export function testCallOf(call: ts.CallExpression): TestCall | undefined {
  let each = false;
  let current: ts.Expression = call.expression;
  for (;;) {
    current = unwrap(current);
    if (ts.isIdentifier(current)) {
      if (!TEST_NAMES.has(current.text)) return undefined;
      break;
    }
    if (ts.isPropertyAccessExpression(current)) {
      each ||= current.name.text === "each";
      current = current.expression;
    } else if (ts.isCallExpression(current)) {
      current = current.expression;
    } else {
      return undefined;
    }
  }
  const [first] = call.arguments;
  if (first === undefined || !ts.isStringLiteralLike(first)) return undefined;
  const callback = [...call.arguments].reverse().map((a) => functionInitializer(a)).find((f) => f !== undefined);
  return { title: first.text, callback, each };
}

/** Whether a file is written for a test framework: it imports a runner. */
export function importsRunner(file: ts.SourceFile): boolean {
  return file.statements.some(
    (statement) =>
      ts.isImportDeclaration(statement) &&
      ts.isStringLiteralLike(statement.moduleSpecifier) &&
      RUNNERS.has(statement.moduleSpecifier.text),
  );
}

/** Declares the top-level tests of a file. */
export function declareTests(decls: FileDecls): void {
  for (const statement of decls.file.statements) {
    if (!ts.isExpressionStatement(statement) || !ts.isCallExpression(statement.expression)) continue;
    const test = testCallOf(statement.expression);
    if (test === undefined) continue;
    decls.add({
      kind: "test",
      name: test.title,
      node: statement,
      extent: statement,
      body: test.callback,
    });
  }
}

/** The deepest chain of tests inside tests that a body declares. */
export function nestingOf(node: ts.Node | undefined): number {
  let deepest = 0;
  const visit = (child: ts.Node): void => {
    const test = ts.isCallExpression(child) ? testCallOf(child) : undefined;
    if (test !== undefined) {
      deepest = Math.max(deepest, 1 + nestingOf(test.callback?.body));
      return;
    }
    ts.forEachChild(child, visit);
  };
  if (node !== undefined) ts.forEachChild(node, visit);
  return deepest;
}

/** Whether a test runs over rows written in the file: `.each`, or a loop over a literal array of rows. */
export function isTable(call: TestCall, file: ts.SourceFile): boolean {
  if (call.each) return true;
  const body = call.callback?.body;
  if (body === undefined) return false;
  const arrays = new Map<string, ts.ArrayLiteralExpression>();
  const bind = (node: ts.Node): void => {
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.initializer !== undefined) {
      const value = unwrap(node.initializer);
      if (ts.isArrayLiteralExpression(value)) arrays.set(node.name.text, value);
    }
    ts.forEachChild(node, bind);
  };
  bind(file);
  const rows = (expression: ts.Expression): boolean => {
    const value = unwrap(expression);
    const array = ts.isArrayLiteralExpression(value) ? value : ts.isIdentifier(value) ? arrays.get(value.text) : undefined;
    return array !== undefined && array.elements.length > 0 && array.elements.every(isRow);
  };
  let table = false;
  const find = (node: ts.Node): void => {
    if (table) return;
    if (ts.isForOfStatement(node) && rows(node.expression)) table = true;
    if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression)) {
      const method = node.expression.name.text;
      if ((method === "forEach" || method === "map") && rows(node.expression.expression)) table = true;
    }
    ts.forEachChild(node, find);
  };
  find(body);
  return table;
}

function isRow(element: ts.Expression): boolean {
  return ts.isObjectLiteralExpression(element) || ts.isArrayLiteralExpression(element);
}
