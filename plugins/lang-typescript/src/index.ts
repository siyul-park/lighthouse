import path from "node:path";
import type ts from "typescript";
import { Model } from "./model.ts";
import { slashed } from "./paths.ts";
import { Workspace } from "./projects.ts";
import { syntaxProblem } from "./diagnostics.ts";
import { Remap, type Snippet } from "./regions.ts";
import { Types } from "./types.ts";
import { type Services, Unit } from "./unit.ts";
import { Reach } from "./visibility.ts";
import type { Fragment, IndexResult } from "./wire.ts";

/** What one index request asks to analyze. */
export interface Request {
  root: string;
  overlays: Map<string, string>;
  /** Project-relative paths of the files to analyze. */
  requested: readonly string[];
  snippets: readonly Snippet[];
  result: Required<IndexResult>;
}

/** The files of one program: those the program is made of and the fences that join it. */
interface Group {
  config: string | undefined;
  files: Map<string, string>;
  snippets: Snippet[];
}

/** One index request in progress: its workspace, the fragments of its fences and what it could not resolve. */
class Analysis {
  private readonly workspace: Workspace;
  private readonly fenced = new Map<Snippet, Fragment>();
  private unresolved = 0;
  private untyped = 0;

  constructor(private readonly request: Request) {
    const overlays = new Map(request.overlays);
    for (const snippet of request.snippets) overlays.set(this.absolute(snippet.rel), snippet.text.text);
    this.workspace = new Workspace(request.root, overlays);
  }

  index(): Map<Snippet, Fragment> {
    for (const members of this.groups()) this.indexGroup(members);
    this.summarize();
    return this.fenced;
  }

  private absolute(rel: string): string {
    return slashed(path.join(this.request.root, rel));
  }

  /** The requested files and fences, by the program of their project. */
  private groups(): Group[] {
    const groups = new Map<string, Group>();
    const group = (config: string | undefined): Group => {
      const key = config ?? "";
      let found = groups.get(key);
      if (found === undefined) {
        found = { config, files: new Map(), snippets: [] };
        groups.set(key, found);
      }
      return found;
    };
    for (const rel of this.request.requested) {
      const abs = this.absolute(rel);
      group(this.workspace.ownerOf(abs)).files.set(abs, rel);
    }
    for (const snippet of this.request.snippets) {
      group(this.workspace.nearestConfig(this.absolute(snippet.rel))).snippets.push(snippet);
    }
    return [...groups.values()];
  }

  private indexGroup(members: Group): void {
    const { result } = this.request;
    const abs = [...members.files.keys(), ...members.snippets.map((s) => this.absolute(s.rel))];
    let built;
    try {
      built = this.workspace.build(members.config, abs);
    } catch (error) {
      const reason = `cannot build the program: ${error instanceof Error ? error.message : String(error)}`;
      for (const rel of members.files.values()) result.incomplete.push({ path: rel, reason });
      return;
    }
    result.notices.push(...built.notices);
    const model = new Model(built.program, this.request.root, built.host);
    const reach = new Reach(model, this.request.root);
    const services: Services = { model, reach, types: new Types(model, reach) };
    for (const [file, rel] of members.files) this.indexFile(built.program, this.unitOf(services, file, rel), rel);
    for (const snippet of members.snippets) {
      const unit = this.unitOf(services, this.absolute(snippet.rel), snippet.rel);
      this.indexSnippet(built.program, unit, snippet);
    }
  }

  private unitOf(services: Services, file: string, rel: string): Unit | undefined {
    const source = services.model.program.getSourceFile(file);
    return source === undefined || services.model.declsOf(source) === undefined
      ? undefined
      : new Unit(services, source, rel);
  }

  private indexFile(program: ts.Program, unit: Unit | undefined, rel: string): void {
    const { result } = this.request;
    if (unit === undefined) {
      result.incomplete.push({ path: rel, reason: "the program does not contain the file" });
      return;
    }
    const problem = syntaxProblem(program, unit.file);
    if (problem !== undefined) result.incomplete.push({ path: rel, reason: problem });
    const indexed = unit.index();
    this.unresolved += indexed.unresolved;
    result.fragments.push(indexed.fragment);
  }

  private indexSnippet(program: ts.Program, unit: Unit | undefined, snippet: Snippet): void {
    if (unit === undefined) return;
    if (syntaxProblem(program, unit.file) !== undefined) {
      const where = `${snippet.host.path}:${hostLine(snippet)}`;
      this.request.result.notices.push(
        `${where}: the ${language(snippet)} fence is not valid ${language(snippet)} and is not analysed`,
      );
      return;
    }
    if (program.getSemanticDiagnostics(unit.file).length > 0) this.untyped++;
    const indexed = unit.index();
    const module = indexed.fragment.modules?.[0]?.path ?? "";
    this.fenced.set(snippet, new Remap(snippet, module).fragment(indexed.fragment));
  }

  private summarize(): void {
    const { notices } = this.request.result;
    if (this.unresolved > 0) {
      notices.push(
        `${this.unresolved} import(s) resolve to no file (dependencies not installed, or paths the configuration does not map); their edges are syntactic`,
      );
    }
    if (this.untyped > 0) {
      notices.push(
        `${this.untyped} TypeScript/JavaScript fence(s) do not type-check (names the prose defines, or code that is not meant to compile); what resolves is reported`,
      );
    }
  }
}

/**
 * Loads one program per project and indexes the requested files and fences
 * through it. The fragments of files go into the result; those of fences
 * come back in the host's coordinates, for the caller to merge by host file.
 */
export function index(request: Request): Map<Snippet, Fragment> {
  return new Analysis(request).index();
}

function language(snippet: Snippet): string {
  return snippet.rel.endsWith(".js") ? "JavaScript" : "TypeScript";
}

/** The line of the host file a fence starts on. */
function hostLine(snippet: Snippet): number {
  const upTo = Buffer.from(snippet.host.text).subarray(0, snippet.source.source_range[0]);
  let line = 1;
  for (const byte of upTo) if (byte === 10) line++;
  return line;
}
