package provider

import (
	"go/ast"
	"go/types"
)

type usage struct {
	kind string
	to   string
}

// uses collects what one function body calls and references, resolved
// through go/types, in order of first use.
type uses struct {
	res     *resolver
	info    *types.Info
	handled map[ast.Node]bool
	seen    map[usage]bool
	list    []usage
}

func (w *uses) Visit(n ast.Node) ast.Visitor {
	switch n := n.(type) {
	case *ast.CallExpr:
		w.call(n)
	case *ast.SelectorExpr:
		if w.handled[n] {
			return w
		}
		w.selector(n, edgeReferences)
		ast.Walk(w, n.X)
		return nil
	case *ast.Ident:
		if !w.handled[n] {
			w.ident(n, edgeReferences)
		}
	case *ast.CompositeLit:
		w.literal(n)
	case *ast.FuncLit:
		ast.Walk(w, n.Body)
		return nil
	}
	return w
}

// call records the callee of a call, or of a conversion, and marks it so the
// walk does not count it a second time as a plain reference.
func (w *uses) call(n *ast.CallExpr) {
	switch f := callee(n.Fun).(type) {
	case *ast.Ident:
		w.handled[f] = true
		w.ident(f, edgeCalls)
	case *ast.SelectorExpr:
		w.handled[f] = true
		w.handled[f.Sel] = true
		w.selector(f, edgeCalls)
	}
}

func (w *uses) ident(id *ast.Ident, kind string) {
	if obj := w.info.Uses[id]; obj != nil {
		w.object(obj, kind)
	}
}

func (w *uses) selector(n *ast.SelectorExpr, kind string) {
	s := w.info.Selections[n]
	if s == nil {
		if obj := w.info.Uses[n.Sel]; obj != nil {
			w.object(obj, kind)
		}
		return
	}
	switch s.Kind() {
	case types.FieldVal:
		if owner, ok := holder(s); ok {
			w.field(owner, s.Obj())
		}
	default:
		fn, ok := s.Obj().(*types.Func)
		if !ok {
			return
		}
		to, ok := w.res.function(fn)
		if !ok {
			return
		}
		w.add(kind, to)
	}
}

// literal records the fields a keyed struct literal sets.
func (w *uses) literal(n *ast.CompositeLit) {
	tv, ok := w.info.Types[n]
	if !ok {
		return
	}
	owner, ok := namedType(tv.Type)
	if !ok {
		return
	}
	for _, element := range n.Elts {
		kv, ok := element.(*ast.KeyValueExpr)
		if !ok {
			continue
		}
		key, ok := kv.Key.(*ast.Ident)
		if !ok {
			continue
		}
		if field, ok := w.info.Uses[key].(*types.Var); ok && field.IsField() {
			w.handled[key] = true
			w.field(owner, field)
		}
	}
}

// object records a use of a package-level object or method: a call stays a
// call only for functions and methods.
func (w *uses) object(obj types.Object, kind string) {
	to, ok := w.res.object(obj)
	if !ok {
		return
	}
	if _, fn := obj.(*types.Func); !fn {
		kind = edgeReferences
	}
	w.add(kind, to)
}

func (w *uses) field(owner *types.TypeName, field types.Object) {
	if to, ok := w.res.field(owner, field); ok {
		w.add(edgeReferences, to)
	}
}

func (w *uses) add(kind, to string) {
	u := usage{kind, to}
	if !w.seen[u] {
		w.seen[u] = true
		w.list = append(w.list, u)
	}
}

func collectUses(u *unit, d *ast.FuncDecl) []usage {
	w := &uses{
		res:     u.res,
		info:    u.info(),
		handled: map[ast.Node]bool{},
		seen:    map[usage]bool{},
	}
	ast.Walk(w, d.Body)
	return w.list
}

// callee strips parentheses and explicit type arguments from a call's function.
func callee(e ast.Expr) ast.Expr {
	for {
		switch t := e.(type) {
		case *ast.ParenExpr:
			e = t.X
		case *ast.IndexExpr:
			e = t.X
		case *ast.IndexListExpr:
			e = t.X
		default:
			return e
		}
	}
}
