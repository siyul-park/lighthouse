import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, describe, it } from "node:test";
import { Provider } from "../src/provider.ts";

const roots: string[] = [];

after(() => {
  for (const root of roots) fs.rmSync(root, { recursive: true, force: true });
});

/** The `from -> to` pairs of the uses of one file that are evaluated as the module loads, by name. */
function loads(text: string): string[] {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "lang-typescript-load-")));
  roots.push(root);
  fs.writeFileSync(path.join(root, "package.json"), '{ "name": "demo" }\n');
  fs.writeFileSync(path.join(root, "a.test.ts"), text);
  const result = new Provider("lang-typescript", "0").index({
    project: { root },
    language: "typescript",
    units: [{ id: "a.test.ts", key: "", files: [{ path: "a.test.ts", hash: "x" }], imports: {} }],
    files: ["a.test.ts"],
    context: { options: {} },
  });
  const name = (id: string) => id.split("::").slice(1).join("::").replace(/#.*/, "");
  return (result.units[0]!.fragments[0]!.edges ?? [])
    .filter((edge) => edge.when === "load")
    .map((edge) => `${"symbol" in edge.from ? name(edge.from.symbol) : "<module>"} -> ${name(edge.to)}`)
    .sort();
}

describe("the uses evaluated as the module loads", () => {
  it("are initializers, the callback of a suite and top-level code, not function bodies or tests", () => {
    const found = loads(
      [
        "const base = 1;",
        "const next = base + 1;",
        "const later = () => base;",
        "const v = later();",
        "describe('x', () => {",
        "  const w = later();",
        "  it('a', () => { later(); });",
        "});",
        "later();",
      ].join("\n"),
    );
    assert.deepEqual(found, ["<module> -> later", "next -> base", "v -> later", "x -> later"]);
  });

  it("are class bases, decorators and static initializers, not instance initializers or methods", () => {
    const found = loads(
      [
        "const K = 1;",
        "function deco(_: unknown): ClassDecorator { return () => {}; }",
        "class Base {}",
        "@deco(K)",
        "class A extends Base {",
        "  static s = K;",
        "  field = K;",
        "  m() { return K; }",
        "}",
        "const iife = (() => K)();",
      ].join("\n"),
    );
    assert.deepEqual(found, ["A -> Base", "A -> K", "A -> deco", "A::s -> K", "iife -> K"]);
  });
});
