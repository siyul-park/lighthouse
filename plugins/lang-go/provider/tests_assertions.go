package provider

import (
	"go/ast"
	"go/token"
	"go/types"
)

var failMethods = map[string]bool{
	"Fatal": true, "Fatalf": true, "Error": true, "Errorf": true, "Fail": true, "FailNow": true,
}

// manualAssertions counts the checks written out by hand in body: an `if`
// with no else whose condition compares or negates and whose only effect is
// to fail the test through its testing handle.
func manualAssertions(body *ast.BlockStmt, info *types.Info) int {
	n := 0
	ast.Inspect(body, func(node ast.Node) bool {
		stmt, ok := node.(*ast.IfStmt)
		if ok && stmt.Else == nil && comparing(stmt.Cond) && failsOnly(stmt.Body, info) {
			n++
		}
		return true
	})
	return n
}

// comparing reports a comparison, a negation, or a boolean combination of
// them.
func comparing(e ast.Expr) bool {
	switch x := unparen(e).(type) {
	case *ast.BinaryExpr:
		switch x.Op {
		case token.EQL, token.NEQ, token.LSS, token.GTR, token.LEQ, token.GEQ:
			return true
		case token.LAND, token.LOR:
			return comparing(x.X) && comparing(x.Y)
		}
	case *ast.UnaryExpr:
		return x.Op == token.NOT
	}
	return false
}

// failsOnly reports a block whose one statement fails the test.
func failsOnly(block *ast.BlockStmt, info *types.Info) bool {
	if len(block.List) != 1 {
		return false
	}
	stmt, ok := block.List[0].(*ast.ExprStmt)
	if !ok {
		return false
	}
	call, ok := stmt.X.(*ast.CallExpr)
	if !ok {
		return false
	}
	sel, ok := call.Fun.(*ast.SelectorExpr)
	return ok && failMethods[sel.Sel.Name] && testingHandle(info.TypeOf(sel.X))
}
