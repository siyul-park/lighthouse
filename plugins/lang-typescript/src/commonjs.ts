import ts from "typescript";

/** The identifiers a statement hands out by name: `export default x`, `module.exports = x`, `module.exports = { x }`. */
export function exportedNames(statement: ts.Statement): ts.Identifier[] {
  if (ts.isExportAssignment(statement)) {
    return ts.isIdentifier(statement.expression) ? [statement.expression] : [];
  }
  if (ts.isVariableStatement(statement)) {
    return statement.declarationList.declarations.flatMap((declaration) =>
      ts.isIdentifier(declaration.name) && assignsExports(declaration.initializer) ? [declaration.name] : [],
    );
  }
  if (!ts.isExpressionStatement(statement) || !ts.isBinaryExpression(statement.expression)) return [];
  return assignedNames(statement.expression);
}

/** Whether a statement hands names out: `module.exports = x`, `exports.f = x`. Those are exports, not work the module does. */
export function assignsToExports(statement: ts.ExpressionStatement): boolean {
  const expression = statement.expression;
  if (!ts.isBinaryExpression(expression) || expression.operatorToken.kind !== ts.SyntaxKind.EqualsToken) return false;
  const base = assignmentTarget(expression.left)?.base;
  return base === "module.exports" || base === "exports" || assignsExports(expression);
}

/** Whether an initializer is what `require` returns, or a part of it: the declaration is an import, which declares no symbol. */
export function isRequireInit(init: ts.Expression | undefined): boolean {
  let current = init === undefined ? undefined : unwrap(init);
  while (current !== undefined && ts.isPropertyAccessExpression(current)) current = unwrap(current.expression);
  return (
    current !== undefined &&
    ts.isCallExpression(current) &&
    ts.isIdentifier(current.expression) &&
    current.expression.text === "require" &&
    current.arguments.length === 1 &&
    ts.isStringLiteralLike(current.arguments[0]!)
  );
}

/** The base and name of `exports.f`, `module.exports.f`, `A.f` and `A.prototype.f`; `module.exports` alone has the name "". */
export function assignmentTarget(left: ts.Expression): { base: string; name: string } | undefined {
  if (!ts.isPropertyAccessExpression(left)) return undefined;
  const object = left.expression;
  if (ts.isIdentifier(object) && object.text === "module" && left.name.text === "exports") {
    return { base: "module.exports", name: "" };
  }
  if (ts.isIdentifier(object)) return { base: object.text, name: left.name.text };
  if (ts.isPropertyAccessExpression(object) && ts.isIdentifier(object.expression)) {
    const root = object.expression.text;
    if (root === "module" && object.name.text === "exports") return { base: "exports", name: left.name.text };
    if (object.name.text === "prototype") return { base: root, name: left.name.text };
  }
  return undefined;
}

export function unwrap(expression: ts.Expression): ts.Expression {
  let current = expression;
  while (
    ts.isParenthesizedExpression(current) ||
    ts.isAsExpression(current) ||
    ts.isSatisfiesExpression(current) ||
    ts.isNonNullExpression(current) ||
    ts.isTypeAssertionExpression(current)
  ) {
    current = current.expression;
  }
  return current;
}

/** The names an assignment hands out: `module.exports = name`, `module.exports = { a, b }`, `exports = module.exports = name`. */
function assignedNames(assignment: ts.BinaryExpression): ts.Identifier[] {
  if (assignsExports(assignment) && ts.isBinaryExpression(unwrap(assignment.right))) {
    // `exports = module.exports = name`: the chain hands out the value it ends in.
    const end = chainValue(assignment);
    return ts.isIdentifier(end) ? [end] : [];
  }
  const { left, operatorToken, right } = assignment;
  if (operatorToken.kind !== ts.SyntaxKind.EqualsToken) return [];
  const target = assignmentTarget(left);
  if (target === undefined || (target.base !== "module.exports" && target.base !== "exports")) return [];
  if (ts.isIdentifier(right)) return [right];
  return ts.isObjectLiteralExpression(right) && target.base === "module.exports" ? propertyNames(right) : [];
}

/** The identifiers an object literal names as its values. */
function propertyNames(literal: ts.ObjectLiteralExpression): ts.Identifier[] {
  return literal.properties.flatMap((property) => {
    if (ts.isShorthandPropertyAssignment(property)) return [property.name];
    return ts.isPropertyAssignment(property) && ts.isIdentifier(property.initializer) ? [property.initializer] : [];
  });
}

/** Whether an initializer is a chain of assignments that ends in the exports: `var app = exports = module.exports = {}`. */
function assignsExports(init: ts.Expression | undefined): boolean {
  let current = init === undefined ? undefined : unwrap(init);
  while (
    current !== undefined &&
    ts.isBinaryExpression(current) &&
    current.operatorToken.kind === ts.SyntaxKind.EqualsToken
  ) {
    const target = assignmentTarget(current.left);
    if ((ts.isIdentifier(current.left) && current.left.text === "exports") || target?.base === "module.exports") {
      return true;
    }
    current = unwrap(current.right);
  }
  return false;
}

/** The value a chain of assignments ends in. */
function chainValue(expression: ts.Expression): ts.Expression {
  let current = unwrap(expression);
  while (ts.isBinaryExpression(current) && current.operatorToken.kind === ts.SyntaxKind.EqualsToken) {
    current = unwrap(current.right);
  }
  return current;
}
