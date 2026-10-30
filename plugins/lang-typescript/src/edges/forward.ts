import ts from "typescript";
import type { Decl } from "../symbols.ts";
import type { Model } from "../model.ts";

/**
 * The call a function body consists of when it passes the receiver and every
 * parameter on, in order: `return this.read(k)` in `load(k)`, or `inner(a, ...rest)`.
 */
export function forwardTarget(model: Model, decl: Decl): string | undefined {
  const fn = decl.body;
  if (fn?.body === undefined) return undefined;
  const call = soleCall(fn.body);
  if (call === undefined || !passesOn(fn.parameters, call.arguments)) return undefined;
  const callee = call.expression;
  const method = decl.kind === "method";
  let name: ts.Identifier | ts.PrivateIdentifier | undefined;
  if (ts.isIdentifier(callee) && !method) name = callee;
  else if (ts.isPropertyAccessExpression(callee) && method && callee.expression.kind === ts.SyntaxKind.ThisKeyword) {
    name = callee.name;
  }
  if (name === undefined) return undefined;
  const target = model.declsOfSymbol(model.checker.getSymbolAtLocation(name))[0];
  return target === undefined ? undefined : target.id;
}

/** The only call a body does, as a statement or as the returned value. */
function soleCall(body: ts.ConciseBody): ts.CallExpression | undefined {
  let expression: ts.Expression | undefined;
  if (ts.isBlock(body)) {
    const statements = body.statements.filter((s) => !ts.isEmptyStatement(s));
    const only = statements[0];
    if (statements.length !== 1 || only === undefined) return undefined;
    if (ts.isExpressionStatement(only)) expression = only.expression;
    else if (ts.isReturnStatement(only)) expression = only.expression;
  } else {
    expression = body;
  }
  while (expression !== undefined && (ts.isParenthesizedExpression(expression) || ts.isAwaitExpression(expression))) {
    expression = expression.expression;
  }
  return expression !== undefined && ts.isCallExpression(expression) ? expression : undefined;
}

/** Whether the arguments are exactly the parameters, named and in order. */
function passesOn(parameters: readonly ts.ParameterDeclaration[], args: readonly ts.Expression[]): boolean {
  const named = parameters.filter((p) => !(ts.isIdentifier(p.name) && p.name.text === "this"));
  if (named.length !== args.length) return false;
  return named.every((parameter, index) => {
    const arg = args[index]!;
    if (!ts.isIdentifier(parameter.name) || parameter.name.text === "_") return false;
    const value = parameter.dotDotDotToken !== undefined && ts.isSpreadElement(arg) ? arg.expression : arg;
    return ts.isIdentifier(value) && value.text === parameter.name.text;
  });
}
