import path from "node:path";
import { Model } from "./model.ts";
import { slashed } from "./paths.ts";
import { Workspace } from "./projects.ts";
import { syntaxProblem } from "./diagnostics.ts";
import { Remap, type Snippet } from "./regions.ts";
import { Types } from "./types.ts";
import { Unit } from "./unit.ts";
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

/**
 * Loads one program per project and indexes the requested files and fences
 * through it. The fragments of files go into the result; those of fences
 * come back in the host's coordinates, for the caller to merge by host file.
 */
export function analyze(request: Request): Map<Snippet, Fragment> {
  const { root, requested, snippets, result } = request;
  const overlays = new Map(request.overlays);
  for (const snippet of snippets) overlays.set(slashed(path.join(root, snippet.rel)), snippet.text.text);
  const workspace = new Workspace(root, overlays);
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
  for (const rel of requested) {
    const abs = slashed(path.join(root, rel));
    group(workspace.ownerOf(abs)).files.set(abs, rel);
  }
  for (const snippet of snippets) {
    group(workspace.nearestConfig(slashed(path.join(root, snippet.rel)))).snippets.push(snippet);
  }

  const fenced = new Map<Snippet, Fragment>();
  let unresolved = 0;
  let untyped = 0;
  for (const members of groups.values()) {
    const abs = [...members.files.keys(), ...members.snippets.map((s) => slashed(path.join(root, s.rel)))];
    let built;
    try {
      built = workspace.build(members.config, abs);
    } catch (error) {
      const reason = `cannot build the program: ${error instanceof Error ? error.message : String(error)}`;
      for (const rel of members.files.values()) result.incomplete.push({ path: rel, reason });
      continue;
    }
    result.notices.push(...built.notices);
    const model = new Model(built.program, root, built.host);
    const reach = new Reach(model, root);
    const types = new Types(model, reach);
    for (const [file, rel] of members.files) {
      const source = built.program.getSourceFile(file);
      if (source === undefined || model.declsOf(source) === undefined) {
        result.incomplete.push({ path: rel, reason: "the program does not contain the file" });
        continue;
      }
      const problem = syntaxProblem(built.program, source);
      if (problem !== undefined) result.incomplete.push({ path: rel, reason: problem });
      const indexed = new Unit(model, reach, types, source, rel, false).index();
      unresolved += indexed.unresolved;
      result.fragments.push(indexed.fragment);
    }
    for (const snippet of members.snippets) {
      const source = built.program.getSourceFile(slashed(path.join(root, snippet.rel)));
      const where = `${snippet.host.path}:${hostLine(snippet)}`;
      if (source === undefined || model.declsOf(source) === undefined) continue;
      if (syntaxProblem(built.program, source) !== undefined) {
        result.notices.push(`${where}: the ${language(snippet)} fence is not valid ${language(snippet)} and is not analysed`);
        continue;
      }
      if (built.program.getSemanticDiagnostics(source).length > 0) untyped++;
      const indexed = new Unit(model, reach, types, source, snippet.rel, true).index();
      const module = indexed.fragment.modules?.[0]?.path ?? "";
      fenced.set(snippet, new Remap(snippet, module).fragment(indexed.fragment));
    }
  }
  if (unresolved > 0) {
    result.notices.push(
      `${unresolved} import(s) resolve to no file (dependencies not installed, or paths the configuration does not map); their edges are syntactic`,
    );
  }
  if (untyped > 0) {
    result.notices.push(
      `${untyped} TypeScript/JavaScript fence(s) do not type-check (names the prose defines, or code that is not meant to compile); what resolves is reported`,
    );
  }
  return fenced;
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
