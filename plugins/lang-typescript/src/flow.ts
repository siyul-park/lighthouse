import ts from "typescript";
import type { Flow as FlowEvent } from "./wire.ts";

/** Decides whether a call calls the function being measured. */
export type SelfCall = (call: ts.CallExpression) => boolean;

/** Walks one function body and normalizes its control flow. */
export class FlowWalker {
  readonly events: FlowEvent[] = [];
  deepest = 0;
  count = 0;
  private nesting = 0;

  constructor(private readonly isSelfCall: SelfCall) {}

  /** Measures a body: a block, or the expression of an arrow function. */
  run(body: ts.ConciseBody): void {
    if (ts.isBlock(body)) {
      this.walk(body);
    } else {
      this.count += 1;
      this.walk(body);
    }
  }

  private walk(node: ts.Node | undefined): void {
    if (node === undefined) return;
    if (ts.isBlock(node)) {
      this.count += statementCount(node.statements);
      ts.forEachChild(node, (child) => this.walk(child));
    } else if (ts.isIfStatement(node)) {
      this.ifStatement(node, false);
    } else if (isLoop(node)) {
      this.loop(node);
    } else if (ts.isSwitchStatement(node)) {
      this.switchStatement(node);
    } else if (ts.isTryStatement(node)) {
      this.tryStatement(node);
    } else if (ts.isConditionalExpression(node)) {
      this.conditional(node);
    } else if (isFunctionLike(node)) {
      this.nested(() => this.functionBody(node));
    } else if (ts.isBinaryExpression(node) && isLogical(node.operatorToken.kind)) {
      this.logic(node);
    } else if (isLabeledJump(node)) {
      this.event("jump");
    } else {
      if (ts.isCallExpression(node) && this.isSelfCall(node)) this.event("recursion");
      ts.forEachChild(node, (child) => this.walk(child));
    }
  }

  private functionBody(node: ts.Node): void {
    const body = (node as ts.FunctionLikeDeclaration).body;
    if (body === undefined) return;
    if (!ts.isBlock(body)) this.count += 1;
    this.walk(body);
  }

  private ifStatement(node: ts.IfStatement, chained: boolean): void {
    this.event(chained ? "else-if" : "if");
    this.walk(node.expression);
    this.nested(() => this.walk(node.thenStatement));
    const other = node.elseStatement;
    if (other === undefined) return;
    if (ts.isIfStatement(other)) {
      this.ifStatement(other, true);
    } else {
      this.event("else");
      this.nested(() => this.walk(other));
    }
  }

  private loop(node: ts.IterationStatement): void {
    this.event("loop");
    ts.forEachChild(node, (child) => {
      if (child !== node.statement) this.walk(child);
    });
    this.nested(() => this.walk(node.statement));
  }

  private switchStatement(node: ts.SwitchStatement): void {
    const clauses = node.caseBlock.clauses;
    const arms = clauses.filter((clause) => ts.isCaseClause(clause)).length;
    const returning =
      clauses.length > 0 &&
      clauses.every((clause) => clause.statements.length === 1 && ts.isReturnStatement(clause.statements[0]!));
    this.events.push({ kind: "switch", nesting: this.nesting, arms, ...(returning ? { returning } : {}) });
    this.walk(node.expression);
    this.nested(() => {
      for (const clause of clauses) {
        this.count += 1;
        if (ts.isCaseClause(clause)) this.walk(clause.expression);
        this.count += statementCount(clause.statements);
        for (const statement of clause.statements) this.walk(statement);
      }
    });
  }

  private tryStatement(node: ts.TryStatement): void {
    this.walk(node.tryBlock);
    const handler = node.catchClause;
    if (handler !== undefined) {
      this.event("catch");
      this.walk(handler.variableDeclaration);
      this.nested(() => this.walk(handler.block));
    }
    this.walk(node.finallyBlock);
  }

  private conditional(node: ts.ConditionalExpression): void {
    this.event("if");
    this.walk(node.condition);
    this.nested(() => {
      this.walk(node.whenTrue);
      this.walk(node.whenFalse);
    });
  }

  /** One event per run of like operators in a chain of `&&`, `||` and `??`, looking through parentheses. */
  private logic(node: ts.BinaryExpression): void {
    const operators: ts.SyntaxKind[] = [];
    const leaves: ts.Expression[] = [];
    chain(node, operators, leaves);
    for (let i = 0; i < operators.length;) {
      let j = i;
      while (j < operators.length && operators[j] === operators[i]) j++;
      this.events.push({ kind: "logic", nesting: this.nesting, operators: j - i });
      i = j;
    }
    for (const leaf of leaves) this.walk(leaf);
  }

  private nested(run: () => void): void {
    this.nesting++;
    this.deepest = Math.max(this.deepest, this.nesting);
    run();
    this.nesting--;
  }

  private event(kind: FlowEvent["kind"]): void {
    this.events.push({ kind, nesting: this.nesting });
  }
}

/** A node whose body is a function of its own: a closure raises the nesting of what it contains. */
export function isFunctionLike(node: ts.Node): boolean {
  return (
    ts.isFunctionExpression(node) ||
    ts.isArrowFunction(node) ||
    ts.isFunctionDeclaration(node) ||
    ts.isMethodDeclaration(node) ||
    ts.isConstructorDeclaration(node) ||
    ts.isGetAccessorDeclaration(node) ||
    ts.isSetAccessorDeclaration(node)
  );
}

/** The statements of a list, empty ones excluded. */
export function statementCount(list: readonly ts.Statement[]): number {
  return list.filter((statement) => !ts.isEmptyStatement(statement)).length;
}

/** The leaf tokens of a node, comments excluded. */
export function countTokens(node: ts.Node, file: ts.SourceFile): number {
  if (node.kind >= ts.SyntaxKind.FirstJSDocNode && node.kind <= ts.SyntaxKind.LastJSDocNode) return 0;
  if (node.kind === ts.SyntaxKind.EndOfFileToken || node.end === node.pos) return 0;
  const children = node.getChildren(file);
  if (children.length === 0) return 1;
  let total = 0;
  for (const child of children) total += countTokens(child, file);
  return total;
}

function chain(node: ts.Expression, operators: ts.SyntaxKind[], leaves: ts.Expression[]): void {
  let current = node;
  while (ts.isParenthesizedExpression(current)) current = current.expression;
  if (!ts.isBinaryExpression(current) || !isLogical(current.operatorToken.kind)) {
    leaves.push(current);
    return;
  }
  chain(current.left, operators, leaves);
  operators.push(current.operatorToken.kind);
  chain(current.right, operators, leaves);
}

function isLogical(kind: ts.SyntaxKind): boolean {
  return (
    kind === ts.SyntaxKind.AmpersandAmpersandToken ||
    kind === ts.SyntaxKind.BarBarToken ||
    kind === ts.SyntaxKind.QuestionQuestionToken
  );
}

function isLoop(node: ts.Node): node is ts.IterationStatement {
  return (
    ts.isForStatement(node) ||
    ts.isForInStatement(node) ||
    ts.isForOfStatement(node) ||
    ts.isWhileStatement(node) ||
    ts.isDoStatement(node)
  );
}

function isLabeledJump(node: ts.Node): boolean {
  return (ts.isBreakStatement(node) || ts.isContinueStatement(node)) && node.label !== undefined;
}
