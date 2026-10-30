import fs from "node:fs";
import path from "node:path";
import ts from "typescript";
import { SNIPPETS_DIR } from "./paths.ts";

const EXTENSIONS: [string, ts.Extension][] = [
  [".ts", ts.Extension.Ts],
  [".tsx", ts.Extension.Tsx],
  [".mts", ts.Extension.Mts],
  [".cts", ts.Extension.Cts],
  [".d.ts", ts.Extension.Dts],
  [".js", ts.Extension.Js],
  [".jsx", ts.Extension.Jsx],
  [".mjs", ts.Extension.Mjs],
  [".cjs", ts.Extension.Cjs],
];

const BUILD_DIRS = /^(dist|build|lib|out)\//;

interface Package {
  dir: string;
  name: string;
  manifest: Record<string, unknown>;
}

/**
 * The module a fence imports when it names the package around its Markdown
 * file, as a consumer would: `import { f } from "pkg"` is the package's entry,
 * `"pkg/sub"` a file below it. Only fences (units under the snippets directory) get it.
 */
export function selfReference(
  containing: string,
  specifier: string,
  exists: (file: string) => boolean,
): ts.ResolvedModuleFull | undefined {
  const at = containing.indexOf(`/${SNIPPETS_DIR}/`);
  if (at < 0 || specifier.startsWith(".") || specifier.startsWith("/")) return undefined;
  const found = packageAbove(containing.slice(0, at));
  if (found === undefined) return undefined;
  const { dir, name, manifest } = found;
  if (specifier !== name && !specifier.startsWith(`${name}/`)) return undefined;
  const subpath = specifier.slice(name.length + 1);
  const bases = subpath === "" ? entryBases(dir, manifest) : [path.join(dir, subpath), path.join(dir, "src", subpath)];
  for (const base of bases) {
    const resolved = probe(base, exists);
    if (resolved !== undefined) return { ...resolved, isExternalLibraryImport: false };
  }
  return undefined;
}

function packageAbove(start: string): Package | undefined {
  let dir = start;
  for (;;) {
    try {
      const manifest: unknown = JSON.parse(fs.readFileSync(path.join(dir, "package.json"), "utf8"));
      if (
        typeof manifest === "object" &&
        manifest !== null &&
        typeof (manifest as { name?: unknown }).name === "string"
      ) {
        return { dir, name: (manifest as { name: string }).name, manifest: manifest as Record<string, unknown> };
      }
    } catch {
      // No readable manifest here: look in the parent directory.
    }
    const parent = path.dirname(dir);
    if (parent === dir) return undefined;
    dir = parent;
  }
}

/** The files the package's entry may be: what its manifest names, with build directories mapped to `src`, then the usual ones. */
function entryBases(dir: string, manifest: Record<string, unknown>): string[] {
  const named = ["types", "typings", "module", "main"]
    .map((key) => manifest[key])
    .filter((value): value is string => typeof value === "string")
    .map((value) => value.replace(/^\.\//, "").replace(/\.(d\.)?[cm]?[jt]sx?$/, ""));
  const mapped = named.map((value) => value.replace(BUILD_DIRS, "src/"));
  return [...named, ...mapped, "src/index", "index", "lib/index"].map((value) => path.join(dir, value));
}

function probe(
  base: string,
  exists: (file: string) => boolean,
): Pick<ts.ResolvedModuleFull, "resolvedFileName" | "extension"> | undefined {
  for (const stem of [base, path.join(base, "index")]) {
    for (const [suffix, extension] of EXTENSIONS) {
      if (exists(stem + suffix)) return { resolvedFileName: stem + suffix, extension };
    }
  }
  return undefined;
}
