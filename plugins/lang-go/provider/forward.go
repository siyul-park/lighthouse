package provider

import (
	"go/ast"
	"go/types"
)

// forwards names the call a function body consists of when that call passes
// the receiver and every parameter on, in order: `return s.read(k)` in
// `func (s *S) load(k string)`, or `inner(a, rest...)`.
func (b *builder) forwards(d *ast.FuncDecl) string {
	call := soleCall(d.Body)
	if call == nil {
		return ""
	}
	params, ok := parameterNames(d.Type)
	if !ok || !sameNames(params, call.Args) {
		return ""
	}
	info := b.u.info()
	switch fun := call.Fun.(type) {
	case *ast.Ident:
		if d.Recv != nil {
			return ""
		}
		if obj, ok := info.Uses[fun].(*types.Func); ok {
			to, _ := b.u.res.function(obj)
			return to
		}
	case *ast.SelectorExpr:
		return b.forwardSelector(d, fun)
	}
	return ""
}

func (b *builder) forwardSelector(d *ast.FuncDecl, fun *ast.SelectorExpr) string {
	info := b.u.info()
	operand, ok := fun.X.(*ast.Ident)
	if !ok {
		return ""
	}
	if d.Recv == nil {
		if _, pkg := info.Uses[operand].(*types.PkgName); !pkg {
			return ""
		}
		if obj, ok := info.Uses[fun.Sel].(*types.Func); ok {
			to, _ := b.u.res.function(obj)
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
			to, _ := b.u.res.function(fn)
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

// parameterNames lists parameter names in order; false when one is unnamed
// or blank.
func parameterNames(t *ast.FuncType) ([]string, bool) {
	var names []string
	if t.Params == nil {
		return names, true
	}
	for _, f := range t.Params.List {
		if len(f.Names) == 0 {
			return nil, false
		}
		for _, n := range f.Names {
			if n.Name == "_" {
				return nil, false
			}
			names = append(names, n.Name)
		}
	}
	return names, true
}

func sameNames(params []string, args []ast.Expr) bool {
	if len(params) != len(args) {
		return false
	}
	for i, arg := range args {
		id, ok := arg.(*ast.Ident)
		if !ok || id.Name != params[i] {
			return false
		}
	}
	return true
}
