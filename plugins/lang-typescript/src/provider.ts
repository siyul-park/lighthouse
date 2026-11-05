import fs from "node:fs";
import path from "node:path";
import type * as sdk from "./sdk/server.ts";
import { digest } from "./api.ts";
import { type Found, index } from "./index.ts";
import { isDeclarationFile, slashed } from "./project/paths.ts";
import { type Snippet, planSnippets } from "./regions.ts";
import type {
  EmbeddedFile,
  Fragment,
  Incomplete,
  IndexParams,
  IndexResult,
  LanguageManifest,
  UnitRef,
  UnitResult,
} from "./sdk/wire.ts";

const LANGUAGE = "typescript";
const SOURCE_GLOB = "**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}";
const TEST_EXTENSIONS = "{ts,tsx,mts,cts,js,jsx,mjs,cjs}";

/** The language provider of TypeScript and JavaScript; it implements the SDK's [sdk.Provider]. */
export class Provider implements sdk.Provider {
  constructor(
    readonly name: string,
    readonly version: string,
  ) {}

  manifest(): LanguageManifest[] {
    return [
      {
        id: LANGUAGE,
        aliases: ["typescript", "ts", "tsx", "typescriptreact", "javascript", "js", "node"],
        globs: [SOURCE_GLOB],
        priority: 0,
        conventions: {
          test_globs: [`**/*.test.${TEST_EXTENSIONS}`, `**/*.spec.${TEST_EXTENSIONS}`, "**/__tests__/**"],
          constructor_prefixes: ["new", "create"],
          test_naming: { prefix: "", snake_case: false, variants: false, ancestors: false },
        },
        capabilities: ["semantic-edges", "extent", "reference-sites", "overlays"],
        ordered_declarations: true,
        units: { by: "file", settings: ["tsconfig.json", "jsconfig.json", "package.json"] },
      },
    ];
  }

  /**
   * Analyzes the files of the units asked for, and the fences of host files,
   * which go through the program of their project with the files: N files cost
   * one load. Failures are reported as incomplete entries; a bad request never
   * ends the process.
   */
  index(params: IndexParams): IndexResult {
    const unknown = unknownOption(params.context?.options?.[LANGUAGE]);
    if (unknown !== undefined) return { units: [], notices: [], incomplete: [{ reason: unknown }] };
    const run = new Indexing(params);
    const files = params.units.flatMap((unit) => (unit.files ?? []).map((file) => file.path));
    const hosts: EmbeddedFile[] = params.units.flatMap((unit) => unit.regions ?? []);
    const requested = run.admit(files);
    const snippets = planSnippets(hosts);
    const { root, overlays, found } = run;
    const regionFragments = index({
      root,
      overlays,
      requested,
      listed: params.files ?? files,
      snippets,
      result: found,
    });
    const fenced = run.merge(hosts, snippets, regionFragments);
    return { units: params.units.map((unit) => run.unit(unit, fenced)), notices: [], incomplete: [] };
  }
}

/** One index request in progress: where it reads from and the result it fills. */
class Indexing {
  readonly root: string;
  readonly overlays = new Map<string, string>();
  readonly found: Found = { fragments: [], notices: [], incomplete: [], shapes: new Map(), fenceImports: new Set() };
  /** Files with no analysis to give: declarations, and files that cannot be read. */
  private readonly bare: Fragment[] = [];
  private readonly gaps: Incomplete[] = [];

  constructor(params: IndexParams) {
    this.root = fs.realpathSync(params.project.root);
    for (const overlay of params.context?.overlays ?? [])
      this.overlays.set(slashed(path.join(this.root, overlay.path)), overlay.text);
  }

  /** The requested files that are analysed; the others get an empty fragment or are incomplete. */
  admit(files: readonly string[]): string[] {
    const admitted: string[] = [];
    for (const rel of new Set(files)) {
      const abs = slashed(path.join(this.root, rel));
      if (isDeclarationFile(rel)) {
        this.bare.push(emptyFragment(rel));
      } else if (!this.overlays.has(abs) && !fs.existsSync(abs)) {
        this.gaps.push({ path: rel, reason: "cannot read the file" });
      } else {
        admitted.push(rel);
      }
    }
    return admitted;
  }

  /** The fragments of the fences, merged into one per host file. */
  merge(
    hosts: readonly EmbeddedFile[],
    snippets: readonly Snippet[],
    regionFragments: ReadonlyMap<Snippet, Fragment>,
  ): Fragment[] {
    return hosts.map((host) => {
      const merged = emptyFragment(host.path);
      for (const snippet of snippets) {
        const fragment = snippet.host === host ? regionFragments.get(snippet) : undefined;
        if (fragment === undefined) continue;
        merged.modules!.push(...(fragment.modules ?? []));
        merged.symbols!.push(...(fragment.symbols ?? []));
        merged.edges!.push(...(fragment.edges ?? []));
        merged.functions!.push(...(fragment.functions ?? []));
        merged.tests!.push(...(fragment.tests ?? []));
        merged.comments!.push(...(fragment.comments ?? []));
      }
      return merged;
    });
  }

  /** The answer for one unit: its files, or the fences of its host files. */
  unit(unit: UnitRef, fenced: readonly Fragment[]): UnitResult {
    const files = unit.files ?? [];
    const own = new Set(files.map((file) => file.path));
    const paths = [...own];
    const byPath = (a: { file: { path: string } }, b: { file: { path: string } }) =>
      a.file.path < b.file.path ? -1 : a.file.path > b.file.path ? 1 : 0;
    const regions = (unit.regions ?? []).map((host) => host.path);
    const fragments = [
      ...this.found.fragments.filter((fragment) => own.has(fragment.file.path)),
      ...this.bare.filter((fragment) => own.has(fragment.file.path)),
      ...fenced.filter((fragment) => regions.includes(fragment.file.path)),
    ].sort(byPath);
    const incomplete = [...this.found.incomplete, ...this.gaps].filter(
      (gap) => gap.path != null && (own.has(gap.path) || regions.includes(gap.path)),
    );
    const shapes = paths.flatMap((path) => this.found.shapes.get(path) ?? []);
    const notices = [...new Set(shapes.flatMap((shape) => shape.notices))];
    const hashes = [...files.map((file) => file.hash), ...(unit.regions ?? []).map((host) => host.hash)];
    const body = digest(...hashes);
    const result: UnitResult = { id: unit.id, fragments, body, api: body, imports: [], notices, incomplete };
    if (regions.length > 0) {
      result.notices = [...notices, ...this.found.notices];
      result.imports = [...this.found.fenceImports].sort();
    } else if (shapes.length > 0) {
      result.api = digest(...shapes.map((shape) => shape.api));
      result.imports = [...new Set(shapes.flatMap((shape) => shape.imports))].filter((id) => id !== unit.id).sort();
    }
    return result;
  }
}

export function emptyFragment(rel: string): Fragment {
  return { file: { path: rel }, modules: [], symbols: [], edges: [], functions: [], tests: [], comments: [] };
}

/** The reason the provider's options are refused, if they are: it has none yet, so any key is unknown. */
function unknownOption(options: unknown): string | undefined {
  if (options === undefined || options === null) return undefined;
  if (typeof options !== "object" || Array.isArray(options)) return `[languages.${LANGUAGE}] must be a table`;
  const [key] = Object.keys(options);
  return key === undefined ? undefined : `unknown key \`${key}\` in [languages.${LANGUAGE}]`;
}
