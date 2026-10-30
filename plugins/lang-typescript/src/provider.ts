import fs from "node:fs";
import path from "node:path";
import type { Handler } from "./server.ts";
import { PROTOCOL_VERSION } from "./server.ts";
import { analyze } from "./analyze.ts";
import { isDeclarationFile, isIgnored, slashed } from "./paths.ts";
import { type Snippet, planSnippets } from "./regions.ts";
import type { EmbeddedFile, Fragment, IndexParams, IndexResult, InitializeResult, ProviderManifest } from "./wire.ts";

const LANGUAGE = "typescript";
const SOURCE_GLOB = "**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}";
const TEST_EXTENSIONS = "{ts,tsx,mts,cts,js,jsx,mjs,cjs}";

/** The language provider of TypeScript and JavaScript; it implements the server's [Handler]. */
export class Provider implements Handler {
  constructor(
    private readonly id: string,
    private readonly version: string,
  ) {}

  initialize(): InitializeResult {
    const manifest: ProviderManifest = {
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
    };
    return {
      protocolVersion: PROTOCOL_VERSION,
      serverInfo: { name: this.id, version: this.version },
      capabilities: { languages: [manifest] },
    };
  }

  /**
   * Analyzes every requested file, and the fences of host files, which go
   * through the program of their project with the files: N files cost one load.
   * Failures are reported as incomplete entries; a bad request never ends the process.
   */
  index(params: IndexParams): IndexResult {
    const result: Required<IndexResult> = { fragments: [], sources: [], notices: [], incomplete: [] };
    const unknown = unknownOption(params.context?.options?.[LANGUAGE]);
    if (unknown !== undefined) {
      result.incomplete.push({ reason: unknown });
      return result;
    }
    const root = fs.realpathSync(params.project.root);
    const overlays = new Map<string, string>();
    for (const overlay of params.context?.overlays ?? [])
      overlays.set(slashed(path.join(root, overlay.path)), overlay.text);

    const files: string[] = [];
    const hosts: EmbeddedFile[] = [];
    for (const input of params.inputs) {
      if (input.kind === "file") files.push(input.file.path);
      else hosts.push(input.regions);
    }
    const requested = this.admit(root, files, overlays, result);
    const snippets = planSnippets(hosts);
    const regionFragments = analyze({ root, overlays, requested, snippets, result });
    this.merge(result, hosts, snippets, regionFragments);
    result.fragments.sort((a, b) => (a.file.path < b.file.path ? -1 : a.file.path > b.file.path ? 1 : 0));
    return result;
  }

  /** The requested files that are analysed; the others get an empty fragment or are incomplete. */
  private admit(
    root: string,
    files: readonly string[],
    overlays: ReadonlyMap<string, string>,
    result: Required<IndexResult>,
  ): string[] {
    const admitted: string[] = [];
    for (const rel of new Set(files)) {
      const abs = slashed(path.join(root, rel));
      if (isIgnored(rel) || isDeclarationFile(rel)) {
        result.fragments.push(emptyFragment(rel));
      } else if (!overlays.has(abs) && !fs.existsSync(abs)) {
        result.incomplete.push({ path: rel, reason: "cannot read the file" });
      } else {
        admitted.push(rel);
      }
    }
    return admitted;
  }

  /** Adds the fragments of the fences to the fragment of their host file, one per host file. */
  private merge(
    result: Required<IndexResult>,
    hosts: readonly EmbeddedFile[],
    snippets: readonly Snippet[],
    regionFragments: ReadonlyMap<Snippet, Fragment>,
  ): void {
    for (const host of hosts) {
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
      result.fragments.push(merged);
    }
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
