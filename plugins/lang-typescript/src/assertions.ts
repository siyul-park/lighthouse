import ts from "typescript";

const COMPARISONS = new Set<ts.SyntaxKind>([
  ts.SyntaxKind.EqualsEqualsToken,
  ts.SyntaxKind.EqualsEqualsEqualsToken,
  ts.SyntaxKind.ExclamationEqualsToken,
  ts.SyntaxKind.ExclamationEqualsEqualsToken,
  ts.SyntaxKind.LessThanToken,
  ts.SyntaxKind.GreaterThanToken,
  ts.SyntaxKind.LessThanEqualsToken,
  ts.SyntaxKind.GreaterThanEqualsToken,
]);

/**
 * Counts the checks written out by hand in a body: an `if` with no else whose
 * condition compares or negates and whose only effect is to throw.
 */
export function manualAssertions(body: ts.Node): number {
  let count = 0;
  const visit = (node: ts.Node): void => {
    if (
      ts.isIfStatement(node) &&
      node.elseStatement === undefined &&
      comparing(node.expression) &&
      throwsOnly(node.thenStatement)
    ) {
      count += 1;
    }
    ts.forEachChild(node, visit);
  };
  visit(body);
  return count;
}

/** A comparison, a negation, or a boolean combination of them. */
function comparing(expression: ts.Expression): boolean {
  if (ts.isParenthesizedExpression(expression)) return comparing(expression.expression);
  if (ts.isPrefixUnaryExpression(expression)) return expression.operator === ts.SyntaxKind.ExclamationToken;
  if (!ts.isBinaryExpression(expression)) return false;
  const operator = expression.operatorToken.kind;
  if (operator === ts.SyntaxKind.AmpersandAmpersandToken || operator === ts.SyntaxKind.BarBarToken) {
    return comparing(expression.left) && comparing(expression.right);
  }
  return COMPARISONS.has(operator);
}

/** A statement, or a block of one statement, that throws. */
function throwsOnly(statement: ts.Statement): boolean {
  if (ts.isBlock(statement)) return statement.statements.length === 1 && ts.isThrowStatement(statement.statements[0]!);
  return ts.isThrowStatement(statement);
}
