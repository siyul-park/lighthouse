import assert from "node:assert/strict";
import { describe, it } from "node:test";
import ts from "typescript";
import { manualAssertions } from "../src/assertions.ts";

function body(text: string): ts.Node {
  return ts.createSourceFile("check.ts", text, ts.ScriptTarget.ESNext, true);
}

describe("manualAssertions", () => {
  it("counts an if that compares and only throws", () => {
    const found = manualAssertions(body('if (got !== 1) { throw new Error("got"); }\nif (!ok) throw new Error("ok");'));

    assert.equal(found, 2);
  });

  it("counts a boolean combination of comparisons", () => {
    const found = manualAssertions(body('if ((a < 1 || b > 2) && !c) { throw new Error("range"); }'));

    assert.equal(found, 1);
  });

  it("ignores an if with an else, more than a throw, or a condition that is no comparison", () => {
    const found = manualAssertions(
      body(
        [
          'if (a === 1) { throw new Error("a"); } else { b(); }',
          'if (a === 1) { log(); throw new Error("a"); }',
          'if (call()) { throw new Error("a"); }',
          "if (a === 1) { b(); }",
        ].join("\n"),
      ),
    );

    assert.equal(found, 0);
  });
});
