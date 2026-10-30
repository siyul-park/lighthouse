import ts from "typescript";
import type { Decl, FunctionBody } from "./symbols.ts";
import type { Model } from "./model.ts";
import type { Reach } from "./visibility.ts";
import type { Signature, TypeRef } from "./wire.ts";

const PRINT = ts.TypeFormatFlags.NoTruncation | ts.TypeFormatFlags.UseAliasDefinedOutsideCurrentScope;
const NOTHING = ts.TypeFlags.Void | ts.TypeFlags.Undefined | ts.TypeFlags.Never;

/** The types a declaration states, as the protocol writes them. */
export class Types {
  constructor(
    private readonly model: Model,
    private readonly reach: Reach,
  ) {}

  /** The type as written, with the project type it names, if there is one. */
  ref(type: ts.Type): TypeRef {
    const checker = this.model.checker;
    const ref: TypeRef = { text: checker.typeToString(type, undefined, PRINT).replaceAll(`${this.model.root}/`, "") };
    const named = checker.getNonNullableType(type);
    const symbol = named.aliasSymbol ?? named.getSymbol();
    const decl = this.model.declsOfSymbol(symbol).find((d) => d.kind === "type" || d.kind === "interface");
    if (decl !== undefined) {
      ref.symbol = decl.id.slice(0, decl.id.lastIndexOf("#"));
      ref.exported = this.reach.exported(decl);
    }
    return ref;
  }

  /** The type of a field as written, and whether a caller may leave the field out. */
  field(node: ts.Node): { ref: TypeRef; optional: boolean } | undefined {
    const checker = this.model.checker;
    const written = (node as { type?: ts.TypeNode }).type;
    const type = written === undefined ? checker.getTypeAtLocation(node) : checker.getTypeFromTypeNode(written);
    if (type === undefined) return undefined;
    const question = (node as { questionToken?: ts.Node }).questionToken !== undefined;
    const nullable = type.isUnion()
      ? type.types.some((member) => (member.flags & (ts.TypeFlags.Undefined | ts.TypeFlags.Null)) !== 0)
      : (type.flags & (ts.TypeFlags.Undefined | ts.TypeFlags.Null)) !== 0;
    return { ref: this.ref(type), optional: question || nullable };
  }

  /** The parameter and result types of a function, receiver excluded; the results are none for a function that returns nothing. */
  signature(fn: FunctionBody): Signature | undefined {
    const checker = this.model.checker;
    const signature = checker.getSignatureFromDeclaration(fn);
    if (signature === undefined) return undefined;
    const params = fn.parameters.filter((p) => !isThis(p)).map((p) => this.parameterRef(p));
    const results = ts.isConstructorDeclaration(fn) ? [] : this.results(fn, signature);
    if (params.length + results.length === 0) return undefined;
    return { params, results };
  }

  /** The number of results, as the signature counts them. */
  returns(fn: FunctionBody): number {
    const signature = this.model.checker.getSignatureFromDeclaration(fn);
    if (signature === undefined || ts.isConstructorDeclaration(fn)) return 0;
    return this.results(fn, signature).length;
  }

  private parameterRef(parameter: ts.ParameterDeclaration): TypeRef {
    const checker = this.model.checker;
    const type =
      parameter.type === undefined ? checker.getTypeAtLocation(parameter) : checker.getTypeFromTypeNode(parameter.type);
    return this.ref(type);
  }

  private results(fn: FunctionBody, signature: ts.Signature): TypeRef[] {
    const checker = this.model.checker;
    let type = signature.getReturnType();
    if ((ts.getCombinedModifierFlags(fn) & ts.ModifierFlags.Async) !== 0) {
      type = checker.getAwaitedType(type) ?? type;
    }
    return (type.flags & NOTHING) !== 0 ? [] : [this.ref(type)];
  }

  /** Whether a function builds a value of a type its own file declares. */
  constructs(decl: Decl): boolean {
    const fn = decl.body;
    if (fn === undefined) return false;
    if (ts.isConstructorDeclaration(fn)) return true;
    if (decl.owner !== undefined && decl.kind === "method" && !isStatic(fn)) return false;
    const signature = this.model.checker.getSignatureFromDeclaration(fn);
    if (signature === undefined) return false;
    let type = signature.getReturnType();
    if ((ts.getCombinedModifierFlags(fn) & ts.ModifierFlags.Async) !== 0) {
      type = this.model.checker.getAwaitedType(type) ?? type;
    }
    const symbol = this.model.checker.getNonNullableType(type).getSymbol();
    const declared = this.model.declsOfSymbol(symbol).find((d) => d.kind === "type" || d.kind === "interface");
    return declared !== undefined && declared.module === decl.module;
  }
}

function isThis(parameter: ts.ParameterDeclaration): boolean {
  return ts.isIdentifier(parameter.name) && parameter.name.text === "this";
}

function isStatic(fn: FunctionBody): boolean {
  return (ts.getCombinedModifierFlags(fn) & ts.ModifierFlags.Static) !== 0;
}
