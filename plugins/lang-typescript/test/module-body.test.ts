import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, describe, it } from "node:test";
import { MODULE_BODY } from "../src/modulebody.ts";
import { Provider } from "../src/provider.ts";
import type { Fragment } from "../src/wire.ts";

const roots: string[] = [];

after(() => {
  for (const root of roots) fs.rmSync(root, { recursive: true, force: true });
});

/** The fragment of one file of a project made of the given files. */
function fragmentOf(files: Record<string, string>, target: string): Fragment {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "lang-typescript-module-body-")));
  roots.push(root);
  fs.writeFileSync(path.join(root, "package.json"), '{ "name": "demo" }\n');
  for (const [name, text] of Object.entries(files)) fs.writeFileSync(path.join(root, name), text);
  const result = new Provider("lang-typescript", "0").index({
    project: { root },
    language: "typescript",
    inputs: [{ kind: "file", file: { path: target, hash: "x" } }],
    context: { options: {} },
  });
  return result.fragments[0]!;
}

const bodies = (fragment: Fragment) => (fragment.symbols ?? []).filter((symbol) => symbol.name === MODULE_BODY);

describe("the body of a module", () => {
  it("owns the calls its top-level statements make", () => {
    const fragment = fragmentOf({ "run.js": "function helper() {}\nhelper();\n" }, "run.js");

    const [body] = bodies(fragment);
    assert.equal(body?.id, "run::<module>#function");
    assert.equal(body?.visibility, "private");
    const calls = (fragment.edges ?? []).filter((edge) => edge.kind === "calls");
    assert.deepEqual(
      calls.map((edge) => [edge.from, edge.to]),
      [[{ symbol: "run::<module>#function" }, "run::helper#function"]],
    );
  });

  it("is one symbol however many statements run", () => {
    const fragment = fragmentOf({ "run.js": "function a() {}\nfunction b() {}\na();\nb();\n" }, "run.js");

    assert.equal(bodies(fragment).length, 1);
  });

  it("is declared by no directive and no export", () => {
    const fragment = fragmentOf(
      { "lib.js": "'use strict'\nfunction used() {}\nmodule.exports = { used };\nexports.same = used;\n" },
      "lib.js",
    );

    assert.equal(bodies(fragment).length, 0);
  });

  it("is declared by no declaration", () => {
    const fragment = fragmentOf({ "decl.ts": "export const x = 1;\nexport function f(): void {}\n" }, "decl.ts");

    assert.equal(bodies(fragment).length, 0);
  });
});
