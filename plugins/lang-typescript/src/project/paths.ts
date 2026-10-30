import path from "node:path";

/** The extensions a source file of the provider may have, longest first. */
const SOURCE_EXTENSIONS = [".d.ts", ".d.mts", ".d.cts", ".tsx", ".jsx", ".mts", ".cts", ".mjs", ".cjs", ".ts", ".js"];

const TEST_MARKERS = /\.(test|spec)$/;

/** The directory a fence's units live in, beside their host file. */
export const SNIPPETS_DIR = ".lighthouse-snippets";

/** An absolute path with `/` separators, as the compiler spells them. */
export function slashed(file: string): string {
  return file.split(path.sep).join("/");
}

/** A path relative to the root with `/` separators; undefined when it lies outside the root. */
export function relative(root: string, file: string): string | undefined {
  const rel = slashed(path.relative(root, file));
  if (rel === "" || rel === ".." || rel.startsWith("../") || path.isAbsolute(rel)) return undefined;
  return rel;
}

export function isDeclarationFile(file: string): boolean {
  return /\.d\.[cm]?ts$/.test(file);
}

/** The path without its source extension. */
export function withoutExtension(file: string): string {
  const extension = SOURCE_EXTENSIONS.find((candidate) => file.endsWith(candidate));
  return extension === undefined ? file : file.slice(0, -extension.length);
}

/** The module a project-relative file declares: its path without the extension. */
export function modulePath(rel: string): string {
  return withoutExtension(rel);
}

/** The module path a test file tests by naming alone, `a/b.test` → `a/b`, `a/__tests__/b` → `a/b`. */
export function testedModule(module: string): string | undefined {
  const dir = path.posix.dirname(module);
  const base = path.posix.basename(module);
  const stripped = base.replace(TEST_MARKERS, "");
  const parent = path.posix.basename(dir) === "__tests__" ? path.posix.dirname(dir) : dir;
  const joined = parent === "." ? stripped : `${parent}/${stripped}`;
  return joined === module ? undefined : joined;
}

/** Whether a project-relative file is a test by the conventions the manifest declares. */
export function isTestFile(rel: string): boolean {
  return /\.(test|spec)\.[cm]?[jt]sx?$/.test(rel) || rel.split("/").includes("__tests__");
}

/** The paths the provider leaves with an empty fragment: dependencies, hidden directories, test data. */
export function isIgnored(rel: string): boolean {
  const parts = rel.split("/");
  if (parts.includes(SNIPPETS_DIR)) return parts.includes("node_modules");
  return rel.split("/").some((part, index, parts) => {
    const isDirectory = index < parts.length - 1;
    return isDirectory && (part === "node_modules" || part === "testdata" || part.startsWith("."));
  });
}

/** A part of a symbol id, with the characters the id format reserves escaped. */
export function idPart(name: string): string {
  return name.replaceAll("#", "%23").replaceAll("::", ":%3A");
}
