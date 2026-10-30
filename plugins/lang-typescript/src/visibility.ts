import fs from "node:fs";
import path from "node:path";
import ts from "typescript";
import type { Decl } from "./symbols.ts";
import { MODULE_BODY } from "./functions.ts";
import type { Model } from "./model.ts";
import { isTestFile, relative, slashed } from "./project/paths.ts";
import type { Visibility } from "./sdk/wire.ts";

const RANK: Record<Visibility, number> = { private: 0, internal: 1, public: 2 };

/** How far the names a file declares reach: exported names of a package that is not published reach the project only. */
export class Reach {
  private readonly packages = new Map<string, boolean>();

  constructor(
    private readonly model: Model,
    private readonly root: string,
  ) {}

  visibility(decl: Decl): Visibility {
    const owner = decl.owner;
    const file = decl.nodes[0]!.getSourceFile();
    if (owner === undefined) {
      if (decl.name === MODULE_BODY) return "private";
      if (this.isTest(file) || !this.exported(decl)) return "private";
      return this.published(file) ? "public" : "internal";
    }
    const outer = this.visibility(owner);
    if (inNamespace(decl) && !this.exported(decl)) return "private";
    const mine = own(decl);
    // A public member of a class its module keeps to itself is still reached through the instances the module hands out.
    if (mine === "public" && outer === "private" && !this.isTest(file)) return "internal";
    return least(outer, mine);
  }

  /** Whether the declaration is importable from outside its file. */
  exported(decl: Decl): boolean {
    const owner = decl.owner;
    if (owner !== undefined && !inNamespace(decl)) return this.exported(owner);
    if (decl.assignment && owner === undefined) return true;
    const node = decl.node;
    const flags = ts.getCombinedModifierFlags(node as ts.Declaration);
    if ((flags & (ts.ModifierFlags.Export | ts.ModifierFlags.Ambient)) !== 0) {
      return owner === undefined || this.exported(owner);
    }
    if (owner !== undefined) return false;
    const symbol = this.model.symbolOfDecl(decl);
    return symbol !== undefined && this.model.exportsOf(node.getSourceFile()).has(symbol);
  }

  private isTest(file: ts.SourceFile): boolean {
    const rel = relative(this.root, slashed(file.fileName));
    return rel !== undefined && isTestFile(rel);
  }

  /** Whether the nearest package.json describes a package that is published, or none is found. */
  private published(file: ts.SourceFile): boolean {
    let dir = path.dirname(file.fileName);
    const visited: string[] = [];
    let result = true;
    while (relative(this.root, dir) !== undefined || dir === this.root) {
      const known = this.packages.get(dir);
      if (known !== undefined) {
        result = known;
        break;
      }
      visited.push(dir);
      const manifest = readPackage(path.join(dir, "package.json"));
      if (manifest !== undefined) {
        result = manifest.private !== true;
        break;
      }
      if (dir === this.root) break;
      dir = path.dirname(dir);
    }
    for (const seen of visited) this.packages.set(seen, result);
    return result;
  }
}

/** Whether a declaration stands directly in a namespace, where only what is exported is visible outside it. */
function inNamespace(decl: Decl): boolean {
  const node = decl.node;
  const parent = ts.isVariableDeclaration(node) ? node.parent.parent.parent : node.parent;
  return ts.isModuleBlock(parent) || ts.isModuleDeclaration(parent);
}

/** The visibility a declaration's own modifiers give it. */
function own(decl: Decl): Visibility {
  const name = (decl.node as ts.NamedDeclaration).name;
  if (name !== undefined && ts.isPrivateIdentifier(name)) return "private";
  const flags = ts.getCombinedModifierFlags(decl.node as ts.Declaration);
  if ((flags & ts.ModifierFlags.Private) !== 0) return "private";
  if ((flags & ts.ModifierFlags.Protected) !== 0) return "internal";
  return "public";
}

function least(a: Visibility, b: Visibility): Visibility {
  return RANK[a] <= RANK[b] ? a : b;
}

function readPackage(file: string): { private?: boolean } | undefined {
  try {
    const parsed: unknown = JSON.parse(fs.readFileSync(file, "utf8"));
    return typeof parsed === "object" && parsed !== null ? (parsed as { private?: boolean }) : undefined;
  } catch {
    return undefined;
  }
}
