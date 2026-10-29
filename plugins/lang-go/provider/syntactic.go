package provider

import (
	"go/ast"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

const resolutionSyntactic = "syntactic"

// unresolved states what the fence of a document uses by name where the type
// checker knows nothing of the name: prose defines names, and a fence that
// uses them is a fragment. Each is a `syntactic` edge to the bare name, which
// the project settles when one symbol of the project is called so.
func (u *unit) unresolved() {
	from := sdk.Node{Module: u.module}
	info := u.info()
	known := func(id *ast.Ident) bool {
		return id.Name == "_" || id.Obj != nil || info.Uses[id] != nil || info.Defs[id] != nil
	}
	called := map[ast.Expr]bool{}
	skipped := map[*ast.Ident]bool{}
	ast.Inspect(u.file, func(n ast.Node) bool {
		switch n := n.(type) {
		case *ast.CallExpr:
			called[n.Fun] = true
		case *ast.KeyValueExpr:
			if key, ok := n.Key.(*ast.Ident); ok {
				skipped[key] = true
			}
		case *ast.SelectorExpr:
			qualifier, ok := n.X.(*ast.Ident)
			if !ok || known(qualifier) {
				// The type checker settled what is selected from a value
				// or a package it knows.
				skipped[n.Sel] = true
				break
			}
			skipped[qualifier] = true
			if info.Uses[n.Sel] == nil && info.Selections[n] == nil {
				u.unresolvedUse(from, n.Sel, called[n])
			}
			skipped[n.Sel] = true
		}
		return true
	})
	ast.Inspect(u.file, func(n ast.Node) bool {
		if id, ok := n.(*ast.Ident); ok && !skipped[id] && !known(id) && id != u.file.Name {
			u.unresolvedUse(from, id, called[id])
		}
		return true
	})
}

// unresolvedUse emits the edge of one use of a name.
func (u *unit) unresolvedUse(from sdk.Node, id *ast.Ident, call bool) {
	kind := edgeReferences
	if call {
		kind = edgeCalls
	}
	site := span(u.fset, id.Pos(), id.End())
	u.edgeAs(kind, from, id.Name, &site, resolutionSyntactic)
}
