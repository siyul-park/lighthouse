import { builtinModules } from "node:module";
import ts from "typescript";
import { type Decl, type FileDecls, assignmentTarget, isScript } from "./decls.ts";
import type { Lines } from "./positions.ts";
import type { Model } from "./model.ts";
import { isIgnored, modulePath, relative, slashed } from "./paths.ts";
import type { EdgeKind, Node, Resolution, Span } from "./wire.ts";

/** Takes the edges a file states. */
export interface EdgeSink {
  edge(kind: EdgeKind, from: Node, to: string, site: Span | undefined, resolution: Resolution): void;
}

const SOURCE_FILE = /\.[cm]?[jt]sx?$/;
const BUILTINS = new Set(builtinModules);

/** Walks a file once and states what each declaration calls, references, imports and implements. */
export class Uses {
  constructor(
    private readonly model: Model,
    private readonly decls: FileDecls,
    private readonly lines: Lines,
    private readonly sink: EdgeSink,
    /** Whether the file is a fence: names no declaration binds are the prose's, and stated by name. */
    private readonly fence: boolean,
  ) {}

  run(file: ts.SourceFile): void {
    this.visit(file, { module: this.decls.module });
  }

  private visit(node: ts.Node, from: Node): void {
    const declared = this.declaredAt(node);
    const here: Node = declared === undefined ? from : { symbol: declared.id };
    if (ts.isImportDeclaration(node)) return this.importEdge(node.moduleSpecifier, node);
    if (ts.isExportDeclaration(node)) return this.exportDeclaration(node, here);
    if (ts.isImportEqualsDeclaration(node)) return this.importEquals(node, here);
    if (ts.isBinaryExpression(node) && this.declaresByAssignment(node)) return this.visit(node.right, here);
    if (ts.isCallExpression(node)) return this.call(node, here);
    if (ts.isNewExpression(node)) return this.construct(node, here);
    if (ts.isTaggedTemplateExpression(node)) return this.tagged(node, here);
    if (ts.isDecorator(node)) return this.decorator(node, here);
    if (ts.isJsxOpeningLikeElement(node)) return this.jsx(node, here);
    if (ts.isPropertyAccessExpression(node)) return this.access(node, here, false);
    if (ts.isElementAccessExpression(node)) return this.elementAccess(node, here, false);
    if (ts.isQualifiedName(node)) return this.qualified(node, here);
    if (ts.isHeritageClause(node)) return this.heritage(node, here);
    if (ts.isShorthandPropertyAssignment(node)) return this.shorthand(node, here);
    if (ts.isBindingElement(node)) return this.bindingElement(node, here);
    if (ts.isImportTypeNode(node)) return this.importType(node, here);
    if (ts.isIdentifier(node)) {
      if (isBareUse(node)) this.use(node, node, here, false);
      return;
    }
    ts.forEachChild(node, (child) => this.visit(child, here));
  }

  /** Whether an assignment of a JavaScript file declares what it assigns to, so its left side is no use of the name. */
  private declaresByAssignment(node: ts.BinaryExpression): boolean {
    if (node.operatorToken.kind !== ts.SyntaxKind.EqualsToken || !isScript(this.decls.file)) return false;
    const target = assignmentTarget(node.left);
    if (target === undefined) return false;
    return target.base === "exports" || target.base === "module.exports" || this.decls.of(node) !== undefined;
  }

  /** The declaration a node is the declaring node of, when it starts a new owner of what its body uses. */
  private declaredAt(node: ts.Node): Decl | undefined {
    if (ts.isSourceFile(node) || ts.isBlock(node)) return undefined;
    return this.decls.of(node);
  }

  private visitAll(nodes: readonly ts.Node[] | undefined, from: Node): void {
    for (const node of nodes ?? []) this.visit(node, from);
  }

  private exportDeclaration(node: ts.ExportDeclaration, from: Node): void {
    if (node.moduleSpecifier !== undefined) {
      this.importEdge(node.moduleSpecifier, node);
      return;
    }
    const clause = node.exportClause;
    if (clause === undefined || !ts.isNamedExports(clause)) return;
    for (const specifier of clause.elements) {
      const target = this.model.checker.getExportSpecifierLocalTargetSymbol(specifier);
      this.add("references", this.model.declsOfSymbol(target), specifier.propertyName ?? specifier.name, from);
    }
  }

  private importEquals(node: ts.ImportEqualsDeclaration, from: Node): void {
    const reference = node.moduleReference;
    if (ts.isExternalModuleReference(reference)) this.importEdge(reference.expression, node);
    else this.visit(reference, from);
  }

  private importType(node: ts.ImportTypeNode, from: Node): void {
    if (ts.isLiteralTypeNode(node.argument)) this.importEdge(node.argument.literal, node);
    if (node.qualifier !== undefined) this.visit(node.qualifier, from);
    this.visitAll(node.typeArguments, from);
  }

  /** An `imports` edge to the module a specifier names, with the import as written for its site. */
  private importEdge(specifier: ts.Node, site: ts.Node): void {
    if (!ts.isStringLiteralLike(specifier)) return;
    const [to, resolution] = this.importTarget(specifier);
    this.sink.edge("imports", { module: this.decls.module }, to, this.spanOf(site), resolution);
  }

  private importTarget(specifier: ts.StringLiteralLike): [string, Resolution] {
    const symbol = this.model.checker.getSymbolAtLocation(specifier);
    const file =
      symbol?.declarations?.find(ts.isSourceFile)?.fileName ?? this.model.resolveFile(specifier.text, this.decls.file);
    if (file !== undefined) {
      const rel = relative(this.model.root, file);
      const inside = rel !== undefined && !isIgnored(rel) && SOURCE_FILE.test(file);
      return [inside ? modulePath(rel) : specifier.text, "semantic"];
    }
    if (symbol !== undefined || isBuiltin(specifier.text)) return [specifier.text, "semantic"];
    if (specifier.text.startsWith(".")) return [this.relativeTarget(specifier.text), "syntactic"];
    return [specifier.text, "syntactic"];
  }

  /** The project path a relative specifier that resolves nowhere would name. */
  private relativeTarget(text: string): string {
    const file = this.decls.file.fileName;
    const dir = slashed(file.slice(0, file.lastIndexOf("/")));
    const joined = slashed(new URL(text, `file://${dir}/`).pathname);
    const rel = relative(this.model.root, joined);
    return rel === undefined ? text : modulePath(rel);
  }

  private call(node: ts.CallExpression, from: Node): void {
    const callee = strip(node.expression);
    if (callee.kind === ts.SyntaxKind.ImportKeyword || isRequire(node)) {
      const [argument] = node.arguments;
      if (argument !== undefined && ts.isStringLiteralLike(argument)) this.importEdge(argument, node);
      this.visitAll(node.arguments, from);
      return;
    }
    this.callee(callee, from);
    this.visitAll(node.typeArguments, from);
    this.visitAll(node.arguments, from);
  }

  private callee(callee: ts.Expression, from: Node): void {
    if (ts.isIdentifier(callee)) this.use(callee, callee, from, true);
    else if (ts.isPropertyAccessExpression(callee)) this.access(callee, from, true);
    else if (ts.isElementAccessExpression(callee)) this.elementAccess(callee, from, true);
    else this.visit(callee, from);
  }

  private construct(node: ts.NewExpression, from: Node): void {
    const callee = strip(node.expression);
    const name = ts.isIdentifier(callee) ? callee : ts.isPropertyAccessExpression(callee) ? callee.name : undefined;
    if (name === undefined) this.visit(callee, from);
    else {
      if (ts.isPropertyAccessExpression(callee)) this.visit(callee.expression, from);
      this.newTarget(name, from);
    }
    this.visitAll(node.typeArguments, from);
    this.visitAll(node.arguments, from);
  }

  /** `new C()` calls the constructor `C` declares, else uses `C`. */
  private newTarget(name: ts.Identifier | ts.PrivateIdentifier, from: Node): void {
    const found = this.model.declsOfSymbol(this.model.checker.getSymbolAtLocation(name));
    const cls = found.find((decl) => decl.kind === "type");
    const constructor =
      cls === undefined ? undefined : this.model.declsOf(cls.nodes[0]!.getSourceFile())?.memberOf(cls, "constructor");
    if (constructor !== undefined) this.add("calls", [constructor], name, from);
    else this.use(name, name, from, true);
  }

  private tagged(node: ts.TaggedTemplateExpression, from: Node): void {
    this.callee(strip(node.tag), from);
    this.visit(node.template, from);
  }

  private decorator(node: ts.Decorator, from: Node): void {
    const expression = strip(node.expression);
    if (ts.isCallExpression(expression)) this.call(expression, from);
    else this.callee(expression, from);
  }

  private jsx(node: ts.JsxOpeningLikeElement, from: Node): void {
    const tag = node.tagName;
    if (ts.isIdentifier(tag)) this.use(tag, tag, from, false);
    else if (ts.isPropertyAccessExpression(tag)) this.access(tag, from, false);
    this.visitAll(node.typeArguments, from);
    this.visit(node.attributes, from);
  }

  private access(node: ts.PropertyAccessExpression, from: Node, call: boolean): void {
    this.visit(node.expression, from);
    this.use(node.name, node.name, from, call, node.expression);
  }

  private elementAccess(node: ts.ElementAccessExpression, from: Node, call: boolean): void {
    this.visit(node.expression, from);
    const argument = node.argumentExpression;
    if (ts.isStringLiteralLike(argument)) this.use(argument, argument, from, call, node.expression);
    else this.visit(argument, from);
  }

  private qualified(node: ts.QualifiedName, from: Node): void {
    this.visit(node.left, from);
    this.use(node.right, node.right, from, false);
  }

  private heritage(node: ts.HeritageClause, from: Node): void {
    for (const type of node.types) {
      const name = nameOf(type.expression);
      if (node.token === ts.SyntaxKind.ImplementsKeyword && name !== undefined) {
        const symbol = this.model.checker.getSymbolAtLocation(name);
        const decl = this.model.declsOfSymbol(symbol)[0];
        if (decl !== undefined) {
          this.sink.edge("implements", from, decl.id, this.spanOf(name), "semantic");
        } else this.use(name, name, from, false);
      } else this.visit(type.expression, from);
      this.visitAll(type.typeArguments, from);
    }
  }

  private shorthand(node: ts.ShorthandPropertyAssignment, from: Node): void {
    const symbol = this.model.checker.getShorthandAssignmentValueSymbol(node);
    this.add("references", this.model.declsOfSymbol(symbol), node.name, from);
    if (node.objectAssignmentInitializer !== undefined) this.visit(node.objectAssignmentInitializer, from);
  }

  /** A destructured property is a use of the property of the destructured value. */
  private bindingElement(node: ts.BindingElement, from: Node): void {
    const pattern = node.parent;
    if (ts.isObjectBindingPattern(pattern)) {
      const key = node.propertyName ?? (ts.isIdentifier(node.name) ? node.name : undefined);
      const text = key === undefined ? undefined : ts.isIdentifier(key) || ts.isStringLiteralLike(key) ? key.text : undefined;
      if (key !== undefined && text !== undefined) {
        const type = this.model.checker.getTypeAtLocation(pattern);
        this.add("references", this.model.declsOfSymbol(type.getProperty(text)), key, from);
      }
      if (node.propertyName !== undefined && !ts.isIdentifier(node.propertyName)) this.visit(node.propertyName, from);
    }
    if (!ts.isIdentifier(node.name)) this.visit(node.name, from);
    if (node.initializer !== undefined) this.visit(node.initializer, from);
  }

  /** A use of a name: a call when it is called and names a function, else a reference. */
  private use(
    name: ts.Identifier | ts.PrivateIdentifier | ts.StringLiteralLike,
    site: ts.Node,
    from: Node,
    call: boolean,
    receiver?: ts.Expression,
  ): void {
    const symbol = this.model.checker.getSymbolAtLocation(name);
    const found = this.model.declsOfSymbol(symbol);
    if (found.length > 0) {
      this.add(call ? "calls" : "references", found, site, from);
    } else if (this.model.declaresName(name.text) && this.unknownTarget(symbol, receiver, call)) {
      const kind = call ? "calls" : "references";
      this.sink.edge(kind, from, name.text, this.spanOf(site), "syntactic");
    }
  }

  /** Whether a name resolves nowhere because of an unresolved import or a receiver of type `any`. */
  private unknownTarget(symbol: ts.Symbol | undefined, receiver: ts.Expression | undefined, call: boolean): boolean {
    if (symbol === undefined) {
      if (receiver === undefined) return this.fence;
      const type = this.model.checker.getTypeAtLocation(receiver);
      return call && (type.flags & ts.TypeFlags.Any) !== 0;
    }
    if ((symbol.flags & ts.SymbolFlags.Alias) === 0) return false;
    if (this.model.checker.getAliasedSymbol(symbol).name !== "unknown") return false;
    return importedFromProject(symbol);
  }

  private add(kind: "calls" | "references", found: readonly Decl[], site: ts.Node, from: Node): void {
    const decl = found[0];
    if (decl === undefined) return;
    const callable = decl.kind === "function" || decl.kind === "method";
    this.sink.edge(kind === "calls" && callable ? "calls" : "references", from, decl.id, this.spanOf(site), "semantic");
  }

  private spanOf(node: ts.Node): Span {
    return this.lines.span(node.getStart(this.decls.file), node.end);
  }
}

/** Whether an identifier names something, rather than declaring a name or labelling a statement. */
function isBareUse(id: ts.Identifier): boolean {
  const parent = id.parent;
  if (parent === undefined) return false;
  if (ts.isShorthandPropertyAssignment(parent) || ts.isExportSpecifier(parent) || ts.isImportSpecifier(parent)) return false;
  if (ts.isBindingElement(parent) && parent.propertyName === id) return false;
  if (ts.isLabeledStatement(parent) || ts.isBreakOrContinueStatement(parent)) return false;
  if (ts.isJsxAttribute(parent) || ts.isJsxNamespacedName(parent)) return false;
  if (ts.isPropertyAccessExpression(parent)) return parent.expression === id;
  if (ts.isQualifiedName(parent)) return parent.left === id;
  if (ts.isTypePredicateNode(parent) && parent.parameterName === id) return false;
  if (ts.isNamespaceExportDeclaration(parent)) return false;
  return (parent as ts.NamedDeclaration).name !== id;
}

/** Whether an unresolved import names a file by a relative path: what it names is the project's, not a dependency's. */
function importedFromProject(alias: ts.Symbol): boolean {
  const declaration = alias.declarations?.[0];
  const statement = declaration === undefined ? undefined : ts.findAncestor(declaration, ts.isImportDeclaration);
  const specifier = statement?.moduleSpecifier;
  return specifier !== undefined && ts.isStringLiteralLike(specifier) && specifier.text.startsWith(".");
}

function strip(expression: ts.Expression): ts.Expression {
  let current = expression;
  while (
    ts.isParenthesizedExpression(current) ||
    ts.isNonNullExpression(current) ||
    ts.isAsExpression(current) ||
    ts.isTypeAssertionExpression(current) ||
    ts.isSatisfiesExpression(current)
  ) {
    current = current.expression;
  }
  return current;
}

function nameOf(expression: ts.Expression): ts.Identifier | undefined {
  if (ts.isIdentifier(expression)) return expression;
  return ts.isPropertyAccessExpression(expression) && ts.isIdentifier(expression.name) ? expression.name : undefined;
}

function isRequire(call: ts.CallExpression): boolean {
  return ts.isIdentifier(call.expression) && call.expression.text === "require" && call.arguments.length === 1;
}

function isBuiltin(specifier: string): boolean {
  return specifier.startsWith("node:") || BUILTINS.has(specifier);
}
