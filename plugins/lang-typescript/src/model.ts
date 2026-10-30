import ts from "typescript";
import { type Decl, FileDecls } from "./decls.ts";
import { isDeclarationFile, isIgnored, isTestFile, modulePath, relative, slashed } from "./paths.ts";
import { selfReference } from "./selfref.ts";
import { declareTests, importsRunner } from "./tests.ts";

/** What one program knows of the project: its declarations, by file, and how symbols reach them. */
export class Model {
  readonly checker: ts.TypeChecker;
  private readonly files = new Map<ts.SourceFile, FileDecls | null>();
  private readonly exports = new Map<ts.SourceFile, Set<ts.Symbol>>();
  private modules: Set<string> | undefined;
  private names: Set<string> | undefined;

  constructor(
    readonly program: ts.Program,
    readonly root: string,
    private readonly host: ts.ModuleResolutionHost,
  ) {
    this.checker = program.getTypeChecker();
  }

  /** The project-relative path of a source file, when it is a subject of the project. */
  relOf(file: ts.SourceFile): string | undefined {
    if (isDeclarationFile(file.fileName) || this.program.isSourceFileFromExternalLibrary(file)) return undefined;
    if (this.program.isSourceFileDefaultLibrary(file)) return undefined;
    const rel = relative(this.root, slashed(file.fileName));
    return rel === undefined || isIgnored(rel) ? undefined : rel;
  }

  /** The declarations of a source file, or undefined for a file that is not the project's. */
  declsOf(file: ts.SourceFile): FileDecls | undefined {
    let decls = this.files.get(file);
    if (decls === undefined) {
      const rel = this.relOf(file);
      decls = rel === undefined ? null : this.collect(file, rel);
      this.files.set(file, decls);
    }
    return decls ?? undefined;
  }

  private collect(file: ts.SourceFile, rel: string): FileDecls {
    const decls = new FileDecls(file, modulePath(rel));
    if (isTestFile(rel) || importsRunner(file)) declareTests(decls);
    return decls;
  }

  /** The module path of a source file inside the project. */
  moduleOf(file: ts.SourceFile): string | undefined {
    const rel = this.relOf(file);
    return rel === undefined ? undefined : modulePath(rel);
  }

  /** The file a module specifier names from a file, the way the program resolved it. */
  resolveFile(specifier: string, from: ts.SourceFile, mode?: ts.ResolutionMode): string | undefined {
    const options = this.program.getCompilerOptions();
    const found = ts.resolveModuleName(specifier, from.fileName, options, this.host, undefined, undefined, mode);
    const resolved = found.resolvedModule;
    if (resolved !== undefined) return resolved.isExternalLibraryImport ? undefined : resolved.resolvedFileName;
    return selfReference(from.fileName, specifier, (file) => this.host.fileExists(file))?.resolvedFileName;
  }

  /** Whether a file of the project declares a name; a use of a name no declaration of the project bears can name nothing of it. */
  declaresName(name: string): boolean {
    if (this.names === undefined) {
      this.names = new Set();
      for (const file of this.program.getSourceFiles()) {
        for (const decl of this.declsOf(file)?.list ?? []) this.names.add(decl.name);
      }
    }
    return this.names.has(name);
  }

  /** Whether a source file of the project declares the module. */
  hasModule(module: string): boolean {
    if (this.modules === undefined) {
      this.modules = new Set();
      for (const file of this.program.getSourceFiles()) {
        const path = this.moduleOf(file);
        if (path !== undefined) this.modules.add(path);
      }
    }
    return this.modules.has(module);
  }

  /** The declaration a node declares. */
  declOf(node: ts.Node): Decl | undefined {
    return this.declsOf(node.getSourceFile())?.of(node);
  }

  /** The declarations of the project a symbol resolves to, through aliases. */
  declsOfSymbol(symbol: ts.Symbol | undefined): Decl[] {
    if (symbol === undefined) return [];
    const target = this.resolve(symbol);
    const found: Decl[] = [];
    for (const node of target.declarations ?? []) {
      const decl = this.declOf(node);
      if (decl !== undefined && !found.includes(decl)) found.push(decl);
    }
    return found;
  }

  /** The symbol an alias (an import, a re-export) stands for. */
  resolve(symbol: ts.Symbol): ts.Symbol {
    if ((symbol.flags & ts.SymbolFlags.Alias) === 0) return symbol;
    const target = this.checker.getAliasedSymbol(symbol);
    return target.declarations === undefined && target.flags === ts.SymbolFlags.None ? symbol : target;
  }

  /** The symbols a file exports, aliases resolved. */
  exportsOf(file: ts.SourceFile): Set<ts.Symbol> {
    let found = this.exports.get(file);
    if (found === undefined) {
      found = new Set();
      // The module symbol of a file is not reachable through the public API; the binder keeps it here.
      const module = (file as ts.SourceFile & { symbol?: ts.Symbol }).symbol;
      if (module !== undefined) {
        for (const exported of this.checker.getExportsOfModule(module)) found.add(this.resolve(exported));
      }
      for (const statement of file.statements) {
        if (!ts.isExportAssignment(statement) || !ts.isIdentifier(statement.expression)) continue;
        const target = this.checker.getSymbolAtLocation(statement.expression);
        if (target !== undefined) found.add(this.resolve(target));
      }
      this.exports.set(file, found);
    }
    return found;
  }

  /** The symbol a declaration's name introduces. */
  symbolOfDecl(decl: Decl): ts.Symbol | undefined {
    for (const node of decl.nodes) {
      const named = nameNodeOf(node);
      const symbol = named === undefined ? undefined : this.checker.getSymbolAtLocation(named);
      if (symbol !== undefined) return this.resolve(symbol);
    }
    return undefined;
  }
}

/** The node that carries the name of a declaration. */
export function nameNodeOf(node: ts.Node): ts.Node | undefined {
  if (ts.isBinaryExpression(node)) return nameNodeOf(node.left);
  if (ts.isExpressionStatement(node)) return nameNodeOf(node.expression);
  if (ts.isPropertyAccessExpression(node)) return node.name;
  if (ts.isExportAssignment(node)) return undefined;
  const named = (node as ts.NamedDeclaration).name;
  return named;
}
