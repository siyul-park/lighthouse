package provider

import (
	"go/ast"
	"go/types"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// subtests finds the deepest chain of `t.Run(name, func...)` calls inside one
// another.
type subtests struct {
	info    *types.Info
	depth   int
	deepest int
}

func (s *subtests) Visit(n ast.Node) ast.Visitor {
	call, ok := n.(*ast.CallExpr)
	if !ok || !isSubtest(call, s.info) {
		return s
	}
	s.depth++
	s.deepest = max(s.deepest, s.depth)
	ast.Walk(s, call.Fun)
	for _, arg := range call.Args {
		ast.Walk(s, arg)
	}
	s.depth--
	return nil
}

// testCaseOf describes the test entry point d, whose body references usages.
func testCaseOf(d *ast.FuncDecl, id string, usages []usage, info *types.Info) sdk.TestCase {
	style := styleScenario
	if isTable(d.Body, info) {
		style = styleTable
	}
	targets := []string{}
	seen := map[string]bool{}
	for _, use := range usages {
		if (use.kind == edgeCalls || use.kind == edgeReferences) && !seen[use.to] {
			seen[use.to] = true
			targets = append(targets, use.to)
		}
	}
	walker := &subtests{info: info}
	ast.Walk(walker, d.Body)
	return sdk.TestCase{Symbol: id, Nesting: walker.deepest, Style: style, Targets: targets}
}

// isSubtest reports a call of testing's Run with a function literal last.
func isSubtest(call *ast.CallExpr, info *types.Info) bool {
	sel, ok := call.Fun.(*ast.SelectorExpr)
	if !ok || len(call.Args) < 2 {
		return false
	}
	if _, lit := call.Args[len(call.Args)-1].(*ast.FuncLit); !lit {
		return false
	}
	s := info.Selections[sel]
	if s == nil {
		return false
	}
	fn, ok := s.Obj().(*types.Func)
	return ok && fn.Name() == "Run" && fn.Pkg() != nil && fn.Pkg().Path() == "testing"
}

// isTable reports a range over a collection of structs written in the test
// itself, directly or through a variable bound to the literal.
func isTable(body *ast.BlockStmt, info *types.Info) bool {
	literals := map[types.Object]*ast.CompositeLit{}
	ast.Inspect(body, func(n ast.Node) bool {
		switch s := n.(type) {
		case *ast.AssignStmt:
			if len(s.Lhs) == len(s.Rhs) {
				for i, lhs := range s.Lhs {
					bind(literals, info, lhs, s.Rhs[i])
				}
			}
		case *ast.ValueSpec:
			if len(s.Names) == len(s.Values) {
				for i, name := range s.Names {
					bind(literals, info, name, s.Values[i])
				}
			}
		}
		return true
	})
	table := false
	ast.Inspect(body, func(n ast.Node) bool {
		r, ok := n.(*ast.RangeStmt)
		if !ok || table {
			return !table
		}
		lit, _ := unparen(r.X).(*ast.CompositeLit)
		if id, ok := unparen(r.X).(*ast.Ident); ok {
			lit = literals[info.Uses[id]]
		}
		table = lit != nil && structCollection(info.TypeOf(lit))
		return !table
	})
	return table
}

func bind(literals map[types.Object]*ast.CompositeLit, info *types.Info, name, value ast.Expr) {
	id, ok := name.(*ast.Ident)
	lit, isLit := unparen(value).(*ast.CompositeLit)
	if !ok || !isLit {
		return
	}
	if obj := info.ObjectOf(id); obj != nil {
		literals[obj] = lit
	}
}

func structCollection(t types.Type) bool {
	if t == nil {
		return false
	}
	var element types.Type
	switch c := t.Underlying().(type) {
	case *types.Slice:
		element = c.Elem()
	case *types.Array:
		element = c.Elem()
	case *types.Map:
		element = c.Elem()
	default:
		return false
	}
	_, isStruct := element.Underlying().(*types.Struct)
	return isStruct
}
