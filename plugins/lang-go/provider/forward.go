package provider

import (
	"go/ast"
	"go/types"
)

// forwardTarget names the call a function body consists of when that call
// passes the receiver and every parameter on, in order: `return s.read(k)` in
// `func (s *S) load(k string)`, or `inner(a, rest...)`.
func forwardTarget(d *ast.FuncDecl, info *types.Info, res *resolver) string {
	call := soleCall(d.Body)
	if call == nil || !passesOn(d.Type, call.Args) {
		return ""
	}
	switch fun := call.Fun.(type) {
	case *ast.Ident:
		if d.Recv != nil {
			return ""
		}
		if fn, ok := info.Uses[fun].(*types.Func); ok {
			to, _ := res.function(fn)
			return to
		}
	case *ast.SelectorExpr:
		return forwardSelector(d, fun, info, res)
	}
	return ""
}

func forwardSelector(d *ast.FuncDecl, fun *ast.SelectorExpr, info *types.Info, res *resolver) string {
	operand, ok := fun.X.(*ast.Ident)
	if !ok {
		return ""
	}
	if d.Recv == nil {
		if _, pkg := info.Uses[operand].(*types.PkgName); !pkg {
			return ""
		}
		if fn, ok := info.Uses[fun.Sel].(*types.Func); ok {
			to, _ := res.function(fn)
			return to
		}
		return ""
	}
	recv := d.Recv.List[0]
	if len(recv.Names) != 1 || recv.Names[0].Name != operand.Name || recv.Names[0].Name == "_" {
		return ""
	}
	if s := info.Selections[fun]; s != nil && s.Kind() == types.MethodVal {
		if fn, ok := s.Obj().(*types.Func); ok {
			to, _ := res.function(fn)
			return to
		}
	}
	return ""
}

func soleCall(body *ast.BlockStmt) *ast.CallExpr {
	var only ast.Stmt
	for _, s := range body.List {
		if _, empty := s.(*ast.EmptyStmt); empty {
			continue
		}
		if only != nil {
			return nil
		}
		only = s
	}
	var expr ast.Expr
	switch s := only.(type) {
	case *ast.ExprStmt:
		expr = s.X
	case *ast.ReturnStmt:
		if len(s.Results) != 1 {
			return nil
		}
		expr = s.Results[0]
	default:
		return nil
	}
	call, _ := expr.(*ast.CallExpr)
	return call
}

// passesOn reports whether args are exactly the parameters of t, named and
// not blank, in order.
func passesOn(t *ast.FuncType, args []ast.Expr) bool {
	var names []string
	if t.Params != nil {
		for _, f := range t.Params.List {
			if len(f.Names) == 0 {
				return false
			}
			for _, n := range f.Names {
				if n.Name == "_" {
					return false
				}
				names = append(names, n.Name)
			}
		}
	}
	if len(names) != len(args) {
		return false
	}
	for i, arg := range args {
		id, ok := arg.(*ast.Ident)
		if !ok || id.Name != names[i] {
			return false
		}
	}
	return true
}
