import ts from "typescript";
import { commentsOf, isGenerated } from "./comments.ts";
import type { Decl } from "./decls.ts";
import { docOf, extentOf } from "./extent.ts";
import { FlowWalker, countTokens, statementCount } from "./flow.ts";
import { forwardTarget } from "./forward.ts";
import type { Model } from "./model.ts";
import { isTestFile, testedModule } from "./paths.ts";
import { Lines } from "./positions.ts";
import { isTable, nestingOf, testCallOf } from "./tests.ts";
import { Types } from "./types.ts";
import { type EdgeSink, Uses } from "./uses.ts";
import type { Reach } from "./visibility.ts";
import type {
  Edge,
  EdgeKind,
  Fragment,
  FunctionSummary,
  Node,
  Resolution,
  Span,
  Symbol as Sym,
  TestCase,
} from "./wire.ts";

/** What indexing a file gave: its fragment and how many of its imports resolve nowhere. */
export interface Indexed {
  fragment: Fragment;
  unresolved: number;
}

/** Builds the fragment of one source file of the project. */
export class Unit implements EdgeSink {
  private readonly lines: Lines;
  private readonly rel: string;
  private readonly test: boolean;
  private readonly edges: Edge[] = [];
  private readonly seen = new Set<string>();
  private readonly targets = new Map<string, Set<string>>();
  private unresolved = 0;

  constructor(
    private readonly model: Model,
    private readonly reach: Reach,
    private readonly types: Types,
    private readonly file: ts.SourceFile,
    rel: string,
    private readonly fence: boolean,
  ) {
    this.lines = new Lines(file.text);
    this.rel = rel;
    this.test = isTestFile(rel);
  }

  edge(kind: EdgeKind, from: Node, to: string, site: Span | undefined, resolution: Resolution): void {
    const owner = "symbol" in from ? from.symbol : from.module;
    const key = `${kind}\0${owner}\0${to}\0${site === undefined ? "" : JSON.stringify(site)}`;
    if (this.seen.has(key)) return;
    this.seen.add(key);
    this.edges.push({ kind, from, to, resolution, ...(site === undefined ? {} : { site }) });
    if (kind === "imports" && resolution === "syntactic" && !to.startsWith("node:")) this.unresolved++;
    if (kind === "calls" || kind === "references") {
      const set = this.targets.get(owner) ?? new Set<string>();
      set.add(to);
      this.targets.set(owner, set);
    }
  }

  index(): Indexed {
    const decls = this.model.declsOf(this.file)!;
    const symbols = decls.list.map((decl) => this.symbol(decl));
    for (const decl of decls.list) {
      const container: Node = decl.owner === undefined ? { module: decls.module } : { symbol: decl.owner.id };
      this.edge("contains", container, decl.id, undefined, "semantic");
    }
    new Uses(this.model, decls, this.lines, this, this.fence).run(this.file);
    const functions: FunctionSummary[] = [];
    const tests: TestCase[] = [];
    for (const decl of decls.list) {
      if (decl.body === undefined) continue;
      functions.push(this.summary(decl));
      if (decl.kind === "test") tests.push(this.testCase(decl));
    }
    const fragment: Fragment = {
      file: { path: this.rel, ...(isGenerated(this.file) ? { generated: true } : {}) },
      modules: [this.moduleRecord(decls.module)],
      symbols,
      edges: this.edges,
      functions,
      tests,
      comments: commentsOf(this.file, this.lines, symbols),
    };
    return { fragment, unresolved: this.unresolved };
  }

  private moduleRecord(module: string): { path: string; test_of?: string } {
    if (!this.test) return { path: module };
    const named = testedModule(module);
    const testOf = named !== undefined && this.model.hasModule(named) ? named : directoryOf(module);
    return { path: module, test_of: testOf };
  }

  private symbol(decl: Decl): Sym {
    const node = decl.assignment ? (decl.extents[0] ?? decl.node) : decl.node;
    const doc = docOf(decl, this.file);
    const extent = extentOf(decl, this.file, this.lines);
    const symbol: Sym = {
      id: decl.id,
      kind: decl.kind,
      visibility: this.reach.visibility(decl),
      file: this.rel,
      span: this.lines.span(node.getStart(this.file), node.end),
      name: decl.name,
    };
    if (decl.owner !== undefined) symbol.owner = decl.owner.id;
    if (extent !== undefined) symbol.extent = extent;
    if (doc !== undefined) symbol.doc = doc;
    const role = this.role(decl);
    if (role !== undefined) symbol.role = role;
    if (decl.kind === "field") {
      const field = this.types.field(decl.node);
      if (field !== undefined) {
        symbol.type_ref = field.ref;
        if (field.optional) symbol.optional = true;
      }
    }
    return symbol;
  }

  /** The role of a declaration of a test file: fixtures are data, helpers check on behalf of tests. */
  private role(decl: Decl): "test-helper" | "fixture" | undefined {
    if (!this.test || decl.owner !== undefined) return undefined;
    if (decl.kind === "function") return callsAssertion(decl.body) ? "test-helper" : "fixture";
    return decl.kind === "type" || decl.kind === "interface" || decl.kind === "const" || decl.kind === "var"
      ? "fixture"
      : undefined;
  }

  private summary(decl: Decl): FunctionSummary {
    const fn = decl.body!;
    const body = fn.body!;
    const selves = new Set<ts.Symbol>();
    const own = this.model.symbolOfDecl(decl);
    if (own !== undefined) selves.add(own);
    const walker = new FlowWalker((call) => this.callsSelf(call, selves));
    walker.run(body);
    const params = fn.parameters.filter((p) => !(ts.isIdentifier(p.name) && p.name.text === "this")).length;
    const signature = this.types.signature(fn);
    const summary: FunctionSummary = {
      symbol: decl.id,
      max_nesting: walker.deepest,
      statements: walker.count,
      top_level: ts.isBlock(body) ? statementCount(body.statements) : 1,
      params,
      returns: this.types.returns(fn),
      tokens: countTokens(body, this.file),
      flow: walker.events,
    };
    const forwards = forwardTarget(this.model, decl);
    if (forwards !== undefined) summary.forwards_to = forwards;
    if (signature !== undefined) summary.signature = signature;
    if (this.implementation(decl)) summary.implementation = true;
    if (this.types.constructs(decl)) summary.constructs = true;
    return summary;
  }

  private callsSelf(call: ts.CallExpression, selves: Set<ts.Symbol>): boolean {
    if (selves.size === 0) return false;
    const callee = call.expression;
    const name = ts.isIdentifier(callee) ? callee : ts.isPropertyAccessExpression(callee) ? callee.name : undefined;
    if (name === undefined) return false;
    const symbol = this.model.checker.getSymbolAtLocation(name);
    return symbol !== undefined && selves.has(this.model.resolve(symbol));
  }

  /** Whether a method of a class implements a member its base class or an interface it names declares. */
  private implementation(decl: Decl): boolean {
    if (decl.kind !== "method" || decl.owner === undefined) return false;
    if ((ts.getCombinedModifierFlags(decl.node as ts.Declaration) & ts.ModifierFlags.Override) !== 0) return true;
    const cls = decl.owner.nodes.find(ts.isClassLike);
    if (cls === undefined || decl.name === "constructor") return false;
    for (const clause of cls.heritageClauses ?? []) {
      for (const type of clause.types) {
        const resolved = this.model.checker.getTypeAtLocation(type);
        if (resolved.getProperty(decl.name) !== undefined) return true;
      }
    }
    return false;
  }

  private testCase(decl: Decl): TestCase {
    const statement = decl.node as ts.ExpressionStatement;
    const call = testCallOf(statement.expression as ts.CallExpression)!;
    return {
      symbol: decl.id,
      nesting: nestingOf(call.callback),
      style: isTable(call, this.file) ? "table" : "scenario",
      targets: [...(this.targets.get(decl.id) ?? [])],
    };
  }
}

/** The directory a test file tests as a whole: its own, or the one above `__tests__`. */
function directoryOf(module: string): string {
  const parts = module.split("/").slice(0, -1);
  if (parts[parts.length - 1] === "__tests__") parts.pop();
  return parts.length === 0 ? "." : parts.join("/");
}

/** Whether a function calls `expect` or an `assert` function: it checks on behalf of tests. */
function callsAssertion(fn: ts.FunctionLikeDeclaration | undefined): boolean {
  if (fn?.body === undefined) return false;
  let found = false;
  const visit = (node: ts.Node): void => {
    if (found) return;
    if (ts.isCallExpression(node)) {
      let callee: ts.Expression = node.expression;
      while (ts.isPropertyAccessExpression(callee)) callee = callee.expression;
      if (ts.isIdentifier(callee) && (callee.text === "expect" || callee.text.startsWith("assert"))) found = true;
    }
    ts.forEachChild(node, visit);
  };
  visit(fn.body);
  return found;
}
