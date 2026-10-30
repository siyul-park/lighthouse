import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, describe, it } from "node:test";
import { Provider } from "../src/provider.ts";
import type { Fragment, Visibility } from "../src/sdk/wire.ts";

const roots: string[] = [];

after(() => {
  for (const root of roots) fs.rmSync(root, { recursive: true, force: true });
});

/** The visibility of every symbol of one file of a project made of the given files. */
function visibilities(files: Record<string, string>, target: string): Record<string, Visibility> {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "lang-typescript-exports-")));
  roots.push(root);
  fs.writeFileSync(path.join(root, "package.json"), '{ "name": "demo" }\n');
  for (const [name, text] of Object.entries(files)) fs.writeFileSync(path.join(root, name), text);
  const result = new Provider("lang-typescript", "0").index({
    project: { root },
    language: "typescript",
    inputs: [{ kind: "file", file: { path: target, hash: "x" } }],
    context: { options: {} },
  });
  const fragment: Fragment = result.fragments[0]!;
  return Object.fromEntries((fragment.symbols ?? []).map((symbol) => [symbol.name, symbol.visibility]));
}

describe("what a file exports", () => {
  it("is what the checker reports for an ES module", () => {
    const seen = visibilities(
      { "lib.ts": "export const shown = 1;\nconst hidden = 2;\nexport { hidden as renamed };\n" },
      "lib.ts",
    );

    assert.deepEqual(seen, { shown: "public", hidden: "public" });
  });

  it("is nothing for a script, whose names are not importable", () => {
    const seen = visibilities({ "script.ts": "const shown = 1;\nfunction run(): void {}\n" }, "script.ts");

    assert.deepEqual(seen, { shown: "private", run: "private" });
  });

  it("is read off the assignments of a CommonJS file, which the checker does not treat as a module", () => {
    const seen = visibilities(
      { "cjs.js": "function used() {}\nfunction unused() {}\nclass Box {}\nmodule.exports = { used, Box };\n" },
      "cjs.js",
    );

    assert.deepEqual(seen, { used: "public", unused: "private", Box: "public" });
  });

  it("includes the class a CommonJS file assigns whole", () => {
    const seen = visibilities({ "whole.js": "class Whole {}\nmodule.exports = Whole;\n" }, "whole.js");

    assert.deepEqual(seen, { Whole: "public" });
  });

  it("includes a variable that a chain of assignments ends in the exports", () => {
    const seen = visibilities(
      { "app.js": "var app = exports = module.exports = {};\nvar local = {};\napp.run = function run() {};\n" },
      "app.js",
    );

    assert.deepEqual(seen, { app: "public", local: "private", run: "public" });
  });

  it("declares no name for a module a CommonJS file requires, as an import declares none", () => {
    const seen = visibilities(
      {
        "app.js":
          "var parser = require('body-parser')\nvar mixin = require('merge-descriptors');\nexports = module.exports = create;\nfunction create() { mixin({}, {}, false); }\nexports.json = parser.json\n",
      },
      "app.js",
    );

    assert.deepEqual(seen, { create: "public", json: "public" });
  });

  it("reaches the public members of a class the module keeps to itself", () => {
    const seen = visibilities(
      {
        "box.ts":
          "class Box {\n  get(): number { return 1; }\n  private hidden(): number { return 2; }\n}\nexport function make(): { get(): number } { return new Box(); }\n",
      },
      "box.ts",
    );

    assert.deepEqual(seen, { Box: "private", get: "internal", hidden: "private", make: "public" });
  });

  it("includes the default export named by an identifier", () => {
    const seen = visibilities({ "main.ts": "function main(): void {}\nexport default main;\n" }, "main.ts");

    assert.deepEqual(seen, { main: "public" });
  });
});
