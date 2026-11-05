import assert from "node:assert/strict";
import { describe, it } from "node:test";
import ts from "typescript";
import { skeleton } from "../src/api.ts";

function of(text: string, name = "a.ts"): string {
  return skeleton(ts.createSourceFile(name, text, ts.ScriptTarget.Latest, true));
}

const base = "export interface S { a: number }\nexport function f(x: number): number {\n  return x + 1;\n}\n";

describe("the api of a file", () => {
  it("is not moved by a body, a comment, a doc comment or the layout", () => {
    assert.equal(of(base), of(base.replace("x + 1", "x + 2")));
    assert.equal(of(base), of(`// about\n${base.replace("a: number", "a:   number /* c */")}\n\n`));
    assert.equal(of(base), of(`/** The file. */\n/** The type. */\n${base}`));
  });

  it("is moved by a signature, a member or a name", () => {
    for (const edited of [
      base.replace("x: number", "x: string"),
      base.replace("a: number", "a: string"),
      base.replace("function f", "function g"),
    ]) {
      assert.notEqual(of(base), of(edited));
    }
  });

  it("ignores comments and layout of a JavaScript file, where the text stays whole", () => {
    const js = "exports.f = function (x) { return x; };\nconst r = /a b/;\n";
    assert.equal(of(js, "a.js"), of(`// about\n${js.replace("function (x)", "function(x)")}`, "a.js"));
    assert.notEqual(of(js, "a.js"), of(js.replace("/a b/", "/ab/"), "a.js"));
  });
});
