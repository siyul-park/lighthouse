import ts from "typescript";
import { assignmentTarget, exportedNames, isRequireInit, unwrap } from "./commonjs.ts";
import { SNIPPETS_DIR, idPart } from "./paths.ts";
import { Lines } from "./positions.ts";
import type { SymbolKind } from "./wire.ts";

/** Anything with a body whose shape the provider measures. */
export type FunctionBody = ts.FunctionLikeDeclaration;

/** One declared name: all the declarations that merge into it (overloads, a class and its namespace). */
export interface Decl {
  id: string;
  kind: SymbolKind;
  name: string;
  module: string;
  owner?: Decl;
  /** Every declaration node of the name, in source order. */
  nodes: ts.Node[];
  /** The node whose range is the symbol's span. */
  node: ts.Node;
  /** Per declaration node, the node whose leading comments and trailing text belong to the symbol; undefined when it shares it. */
  extents: (ts.Node | undefined)[];
  /** The function the summary measures, when the name has a body. */
  body?: FunctionBody;
  /** The CommonJS or class member modifiers' source: a declaration written as an assignment. */
  assignment?: boolean;
}

/** The declarations of one source file, in source order. */
export class FileDecls {
  readonly list: Decl[] = [];
  private readonly ids = new Map<string, Decl>();
  private readonly nodes = new Map<ts.Node, Decl>();
  readonly lines: Lines;
  /** Whether the file is a fence of a host file: analysed as a unit of its own beside the host. */
  readonly fence: boolean;
  /** The names a CommonJS file hands out: a value it exports, not a binding anyone can assign. */
  readonly exported: ReadonlySet<string>;

  constructor(
    readonly file: ts.SourceFile,
    readonly module: string,
  ) {
    this.lines = new Lines(file.text);
    this.fence = file.fileName.includes(`/${SNIPPETS_DIR}/`);
    this.exported = new Set(isScript(file) ? file.statements.flatMap(exportedNames).map((id) => id.text) : []);
    new Collector(this).statements(file.statements, undefined);
  }

  /** The declaration a node declares, or that its range lies in. */
  of(node: ts.Node): Decl | undefined {
    const direct = this.nodes.get(node);
    if (direct !== undefined) return direct;
    if (ts.isVariableDeclaration(node) && !ts.isIdentifier(node.name)) return this.firstBound(node.name);
    if (!ts.isBinaryExpression(node)) return undefined;
    return this.nodes.get(node.left) ?? this.nodes.get(node.parent);
  }

  /** The declaration of the first name a destructuring pattern binds: the one that owns what its initializer uses. */
  private firstBound(pattern: ts.BindingName): Decl | undefined {
    if (ts.isIdentifier(pattern)) return undefined;
    for (const element of pattern.elements) {
      if (!ts.isBindingElement(element)) continue;
      const found = this.nodes.get(element) ?? this.firstBound(element.name);
      if (found !== undefined) return found;
    }
    return undefined;
  }

  /** Whether a statement is, or states, a declaration already. */
  covers(statement: ts.Statement): boolean {
    if (this.nodes.has(statement)) return true;
    return ts.isExpressionStatement(statement) && this.of(statement.expression) !== undefined;
  }

  /** The member of a declaration with a name, as the class's constructor. */
  memberOf(owner: Decl, name: string): Decl | undefined {
    return this.list.find((decl) => decl.owner === owner && decl.name === name);
  }

  /** Adds a declaration or merges the node into the one with the same id. */
  add(init: {
    kind: SymbolKind;
    name: string;
    owner?: Decl;
    node: ts.Node;
    extent?: ts.Node;
    body?: FunctionBody;
    assignment?: boolean;
  }): Decl {
    const parts = [...this.ownerNames(init.owner), init.name].map(idPart);
    const id = `${this.module}::${parts.join("::")}#${init.kind}`;
    const existing = this.ids.get(id);
    if (existing !== undefined) {
      existing.nodes.push(init.node);
      existing.extents.push(init.extent);
      if (init.body !== undefined && existing.body === undefined) {
        existing.body = init.body;
        existing.node = init.node;
      }
      this.nodes.set(init.node, existing);
      return existing;
    }
    const { extent, ...rest } = init;
    const decl: Decl = { ...rest, id, module: this.module, nodes: [init.node], extents: [extent] };
    this.list.push(decl);
    this.ids.set(id, decl);
    this.nodes.set(init.node, decl);
    return decl;
  }

  private ownerNames(owner: Decl | undefined): string[] {
    return owner === undefined ? [] : [...this.ownerNames(owner.owner), owner.name];
  }
}

/** Collects the declarations of a statement list into a [FileDecls]. */
class Collector {
  constructor(private readonly decls: FileDecls) {}

  statements(list: readonly ts.Statement[], owner: Decl | undefined): void {
    for (const statement of list) this.statement(statement, owner);
  }

  private statement(statement: ts.Statement, owner: Decl | undefined): void {
    if (ts.isFunctionDeclaration(statement)) return this.functionDeclaration(statement, owner);
    if (ts.isClassDeclaration(statement)) return this.classDeclaration(statement, owner);
    if (ts.isInterfaceDeclaration(statement)) return this.interfaceDeclaration(statement, owner);
    if (ts.isTypeAliasDeclaration(statement)) return this.typeAlias(statement, owner);
    if (ts.isEnumDeclaration(statement)) return this.enumDeclaration(statement, owner);
    if (ts.isModuleDeclaration(statement)) return this.moduleDeclaration(statement, owner);
    if (ts.isVariableStatement(statement)) return this.variableStatement(statement, owner);
    if (ts.isExportAssignment(statement)) return this.exportAssignment(statement, owner);
    if (ts.isExpressionStatement(statement)) this.assignment(statement, owner);
  }

  private functionDeclaration(node: ts.FunctionDeclaration, owner: Decl | undefined): void {
    const name = node.name?.text ?? (hasDefault(node) ? "default" : undefined);
    if (name === undefined) return;
    this.decls.add({
      kind: "function",
      name,
      owner,
      node,
      extent: node,
      body: node.body === undefined ? undefined : node,
    });
  }

  private classDeclaration(node: ts.ClassDeclaration, owner: Decl | undefined): void {
    const name = node.name?.text ?? (hasDefault(node) ? "default" : undefined);
    if (name === undefined) return;
    const decl = this.decls.add({ kind: "type", name, owner, node, extent: node });
    this.classMembers(node, decl);
  }

  private classMembers(node: ts.ClassLikeDeclaration, owner: Decl): void {
    for (const member of node.members) {
      if (ts.isConstructorDeclaration(member)) {
        this.method("constructor", member, owner);
        for (const parameter of member.parameters) this.parameterProperty(parameter, owner);
        this.assignedFields(member, owner);
      } else if (ts.isMethodDeclaration(member) || ts.isGetAccessor(member) || ts.isSetAccessor(member)) {
        const name = nameText(member.name);
        if (name !== undefined) this.method(name, member, owner);
      } else if (ts.isPropertyDeclaration(member)) {
        this.property(member, owner);
      }
    }
  }

  /** The fields a JavaScript constructor sets: `this.name = value` among its statements. */
  private assignedFields(constructor: ts.ConstructorDeclaration, owner: Decl): void {
    if (!isScript(constructor.getSourceFile())) return;
    for (const statement of constructor.body?.statements ?? []) {
      if (!ts.isExpressionStatement(statement)) continue;
      const expression = statement.expression;
      if (!ts.isBinaryExpression(expression) || expression.operatorToken.kind !== ts.SyntaxKind.EqualsToken) continue;
      const left = expression.left;
      if (!ts.isPropertyAccessExpression(left) || left.expression.kind !== ts.SyntaxKind.ThisKeyword) continue;
      if (this.decls.memberOf(owner, left.name.text) !== undefined) continue;
      this.decls.add({ kind: "field", name: left.name.text, owner, node: left, extent: statement, assignment: true });
    }
  }

  private method(name: string, node: FunctionBody, owner: Decl): void {
    this.decls.add({
      kind: "method",
      name,
      owner,
      node,
      extent: node,
      body: node.body === undefined ? undefined : node,
    });
  }

  private parameterProperty(parameter: ts.ParameterDeclaration, owner: Decl): void {
    const flags = ts.getCombinedModifierFlags(parameter);
    const marked =
      flags &
      (ts.ModifierFlags.Public | ts.ModifierFlags.Private | ts.ModifierFlags.Protected | ts.ModifierFlags.Readonly);
    if (marked === 0 || !ts.isIdentifier(parameter.name)) return;
    this.decls.add({ kind: "field", name: parameter.name.text, owner, node: parameter, extent: parameter });
  }

  private property(member: ts.PropertyDeclaration, owner: Decl): void {
    const name = nameText(member.name);
    if (name === undefined) return;
    const fn = functionInitializer(member.initializer);
    this.decls.add({
      kind: fn === undefined ? "field" : "method",
      name,
      owner,
      node: member,
      extent: member,
      body: fn,
    });
  }

  private interfaceDeclaration(node: ts.InterfaceDeclaration, owner: Decl | undefined): void {
    const decl = this.decls.add({ kind: "interface", name: node.name.text, owner, node, extent: node });
    this.signatures(node.members, decl);
  }

  private signatures(members: readonly ts.TypeElement[], owner: Decl): void {
    for (const member of members) {
      const name = nameText(member.name);
      if (name === undefined) continue;
      if (ts.isPropertySignature(member)) {
        this.decls.add({ kind: "field", name, owner, node: member, extent: member });
      } else if (
        ts.isMethodSignature(member) ||
        ts.isGetAccessorDeclaration(member) ||
        ts.isSetAccessorDeclaration(member)
      ) {
        this.decls.add({ kind: "method", name, owner, node: member, extent: member });
      }
    }
  }

  private typeAlias(node: ts.TypeAliasDeclaration, owner: Decl | undefined): void {
    const decl = this.decls.add({ kind: "type", name: node.name.text, owner, node, extent: node });
    if (ts.isTypeLiteralNode(node.type)) this.signatures(node.type.members, decl);
  }

  private enumDeclaration(node: ts.EnumDeclaration, owner: Decl | undefined): void {
    const decl = this.decls.add({ kind: "type", name: node.name.text, owner, node, extent: node });
    for (const member of node.members) {
      const name = nameText(member.name);
      if (name !== undefined) this.decls.add({ kind: "variant", name, owner: decl, node: member, extent: member });
    }
  }

  private moduleDeclaration(node: ts.ModuleDeclaration, owner: Decl | undefined): void {
    if (!ts.isIdentifier(node.name) || (node.flags & ts.NodeFlags.GlobalAugmentation) !== 0) return;
    const decl = this.decls.add({ kind: "type", name: node.name.text, owner, node, extent: node });
    const body = node.body;
    if (body === undefined) return;
    if (ts.isModuleBlock(body)) this.statements(body.statements, decl);
    else if (ts.isModuleDeclaration(body)) this.moduleDeclaration(body, decl);
  }

  private variableStatement(statement: ts.VariableStatement, owner: Decl | undefined): void {
    for (const declaration of statement.declarationList.declarations) {
      this.binding(declaration.name, declaration, owner);
    }
  }

  /** Declares the names a variable declaration binds: one, or those of a destructuring pattern. */
  private binding(name: ts.BindingName, declaration: ts.VariableDeclaration, owner: Decl | undefined): void {
    if (isRequireInit(declaration.initializer)) return;
    const element = ts.isBindingElement(name.parent) ? name.parent : undefined;
    const list = declaration.parent as ts.VariableDeclarationList;
    const isConst = (list.flags & ts.NodeFlags.Const) !== 0 || (list.flags & ts.NodeFlags.Using) !== 0;
    const single = name === declaration.name && list.declarations.length === 1;
    const extent = single && ts.isVariableStatement(list.parent) ? list.parent : undefined;
    if (ts.isIdentifier(name)) {
      const whole = element === undefined;
      const fn = isConst && whole ? functionInitializer(declaration.initializer) : undefined;
      const cls = isConst && whole ? classInitializer(declaration.initializer) : undefined;
      const value = isConst || this.decls.exported.has(name.text);
      const decl = this.decls.add({
        kind: fn !== undefined ? "function" : cls !== undefined ? "type" : value ? "const" : "var",
        name: name.text,
        owner,
        node: element ?? declaration,
        extent,
        body: fn,
      });
      if (cls !== undefined) this.classMembers(cls, decl);
      return;
    }
    for (const part of name.elements) {
      if (ts.isBindingElement(part)) this.binding(part.name, declaration, owner);
    }
  }

  private exportAssignment(node: ts.ExportAssignment, owner: Decl | undefined): void {
    const value = unwrap(node.expression);
    if (ts.isIdentifier(value)) return;
    const fn = functionInitializer(value);
    if (fn !== undefined) {
      this.decls.add({ kind: "function", name: "default", owner, node, extent: node, body: fn });
    } else if (ts.isClassExpression(value)) {
      const decl = this.decls.add({ kind: "type", name: "default", owner, node, extent: node });
      this.classMembers(value, decl);
    } else {
      this.decls.add({ kind: "const", name: "default", owner, node, extent: node });
    }
  }

  /** CommonJS and prototype assignments of JavaScript files: `exports.f = function`, `A.prototype.m = ...`. */
  private assignment(statement: ts.ExpressionStatement, owner: Decl | undefined): void {
    if (owner !== undefined || !isScript(statement.getSourceFile())) return;
    const expression = statement.expression;
    if (!ts.isBinaryExpression(expression) || expression.operatorToken.kind !== ts.SyntaxKind.EqualsToken) return;
    const target = assignmentTarget(expression.left);
    if (target === undefined) return;
    const value = unwrap(expression.right);
    if (target.base === "module.exports") {
      this.moduleExports(statement, value);
      return;
    }
    const base = target.base === "exports" ? undefined : this.byName(target.base);
    if (target.base !== "exports" && base === undefined) return;
    const fn = functionInitializer(value);
    const kind: SymbolKind =
      base === undefined
        ? fn !== undefined
          ? "function"
          : ts.isClassExpression(value)
            ? "type"
            : "const"
        : fn !== undefined
          ? "method"
          : "field";
    if (base === undefined && ts.isIdentifier(value)) return;
    const decl = this.decls.add({
      kind,
      name: target.name,
      owner: base,
      node: expression.left,
      extent: statement,
      body: fn,
      assignment: true,
    });
    if (ts.isClassExpression(value)) this.classMembers(value, decl);
  }

  private moduleExports(statement: ts.ExpressionStatement, value: ts.Expression): void {
    if (ts.isIdentifier(value)) return;
    const fn = functionInitializer(value);
    if (fn !== undefined) {
      const name = ts.isFunctionExpression(fn) ? (fn.name?.text ?? "default") : "default";
      this.decls.add({ kind: "function", name, node: statement, extent: statement, body: fn, assignment: true });
    } else if (ts.isClassExpression(value)) {
      const decl = this.decls.add({
        kind: "type",
        name: value.name?.text ?? "default",
        node: statement,
        extent: statement,
        assignment: true,
      });
      this.classMembers(value, decl);
    } else if (ts.isObjectLiteralExpression(value)) {
      this.exportedObject(value);
    }
  }

  private exportedObject(value: ts.ObjectLiteralExpression): void {
    for (const property of value.properties) {
      const name = nameText(property.name);
      if (name === undefined) continue;
      const fn = ts.isMethodDeclaration(property)
        ? property
        : ts.isPropertyAssignment(property)
          ? functionInitializer(property.initializer)
          : undefined;
      if (fn !== undefined) {
        this.decls.add({ kind: "function", name, node: property, extent: property, body: fn, assignment: true });
      } else if (ts.isPropertyAssignment(property) && !ts.isIdentifier(unwrap(property.initializer))) {
        this.decls.add({ kind: "const", name, node: property, extent: property, assignment: true });
      }
    }
  }

  private byName(name: string): Decl | undefined {
    return this.decls.list.find((decl) => decl.name === name && decl.owner === undefined);
  }
}

/** The text of the name of a member or declaration, or undefined when it is computed. */
export function nameText(name: ts.Node | undefined): string | undefined {
  if (name === undefined) return undefined;
  if (ts.isIdentifier(name) || ts.isPrivateIdentifier(name)) return name.text;
  if (ts.isStringLiteralLike(name) || ts.isNumericLiteral(name)) return name.text;
  if (ts.isComputedPropertyName(name)) return nameText(name.expression);
  return undefined;
}

/** The function an initializer is, through parentheses and assertions. */
export function functionInitializer(
  init: ts.Expression | undefined,
): ts.ArrowFunction | ts.FunctionExpression | undefined {
  const inner = init === undefined ? undefined : unwrap(init);
  return inner !== undefined && (ts.isArrowFunction(inner) || ts.isFunctionExpression(inner)) ? inner : undefined;
}

/** The class an initializer is, through parentheses and assertions. */
export function classInitializer(init: ts.Expression | undefined): ts.ClassExpression | undefined {
  const inner = init === undefined ? undefined : unwrap(init);
  return inner !== undefined && ts.isClassExpression(inner) ? inner : undefined;
}

/** Whether the file is JavaScript, where assignments declare members and exports. */
export function isScript(file: ts.SourceFile): boolean {
  return /\.[cm]?jsx?$/.test(file.fileName);
}

function hasDefault(node: ts.Node): boolean {
  return (ts.getCombinedModifierFlags(node as ts.Declaration) & ts.ModifierFlags.Default) !== 0;
}
