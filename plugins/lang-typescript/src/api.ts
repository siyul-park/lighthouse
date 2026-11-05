import { createHash } from "node:crypto";
import ts from "typescript";
import type { Model } from "./model.ts";

/** What an importer of a file sees of it, and which files of the project it imports. */
export interface Shape {
  /** A hash of the file's declarations, and of those of every file it imports, directly or not. */
  api: string;
  /** The project-relative files it imports, sorted. */
  imports: string[];
}

/** SHA-256 over the parts, which are kept apart by their lengths. */
export function digest(...parts: readonly string[]): string {
  const hash = createHash("sha256");
  for (const part of parts) hash.update(`${Buffer.byteLength(part)}:`).update(part);
  return hash.digest("hex");
}

/**
 * The text of a file without what an importer cannot see of it: the bodies of
 * functions whose return type is written. A body whose type the compiler infers
 * stays, as it decides the type, and so does the whole of a JavaScript file,
 * where members are made by assignments. An edit inside a removed body leaves
 * the text as it was, which is the early cutoff.
 */
export function skeleton(file: ts.SourceFile): string {
  if (!/\.[cm]?tsx?$/.test(file.fileName)) return file.text;
  const bodies: [number, number][] = [];
  const visit = (node: ts.Node): void => {
    const body = bodyOf(node);
    if (body !== undefined) {
      bodies.push([body.getStart(file), body.end]);
      return;
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  let text = "";
  let at = 0;
  for (const [start, end] of bodies.sort((a, b) => a[0] - b[0])) {
    text += file.text.slice(at, start);
    at = end;
  }
  return text + file.text.slice(at);
}

/** The block of a function-like node that states its return type, or of a constructor. */
function bodyOf(node: ts.Node): ts.Block | undefined {
  const functionLike =
    ts.isFunctionDeclaration(node) ||
    ts.isMethodDeclaration(node) ||
    ts.isFunctionExpression(node) ||
    ts.isArrowFunction(node) ||
    ts.isGetAccessorDeclaration(node);
  if (functionLike && node.type !== undefined && node.body !== undefined && ts.isBlock(node.body)) return node.body;
  if (ts.isConstructorDeclaration(node) && node.body !== undefined) return node.body;
  return undefined;
}

/** The project files a file imports, as the program resolved them. */
export function importsOf(model: Model, file: ts.SourceFile): ts.SourceFile[] {
  const specifiers = ts.preProcessFile(file.text, true, true).importedFiles.map((imported) => imported.fileName);
  const found = new Map<string, ts.SourceFile>();
  for (const specifier of specifiers) {
    const resolved = model.resolveFile(specifier, file);
    const source = resolved === undefined ? undefined : model.program.getSourceFile(resolved);
    if (source !== undefined && model.relOf(source) !== undefined) found.set(source.fileName, source);
  }
  return [...found.values()];
}

/** The shapes of files of one program, each computed once. */
export class Shapes {
  private readonly skeletons = new Map<string, string>();

  constructor(private readonly model: Model) {}

  of(file: ts.SourceFile): Shape {
    const seen = new Map<string, ts.SourceFile>([[file.fileName, file]]);
    const direct = importsOf(this.model, file);
    const stack = [...direct];
    while (stack.length > 0) {
      const next = stack.pop()!;
      if (seen.has(next.fileName)) continue;
      seen.set(next.fileName, next);
      stack.push(...importsOf(this.model, next));
    }
    const names = [...seen.keys()].sort();
    const parts = names.flatMap((name) => [name, this.skeletonOf(seen.get(name)!)]);
    const imports = direct
      .map((source) => this.model.relOf(source)!)
      .filter((rel) => rel !== this.model.relOf(file))
      .sort();
    return { api: digest(...parts), imports: [...new Set(imports)] };
  }

  private skeletonOf(file: ts.SourceFile): string {
    let found = this.skeletons.get(file.fileName);
    if (found === undefined) {
      found = skeleton(file);
      this.skeletons.set(file.fileName, found);
    }
    return found;
  }
}
