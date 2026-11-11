package provider

import (
	"go/ast"
	"go/token"
	"go/types"
	"sort"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// stepCounter counts the syntax of one statement.
type stepCounter struct {
	info      *types.Info
	fset      *token.FileSet
	operators int
	indexing  int
	literals  int
	calls     []sdk.StepCall
}

func (c *stepCounter) visit(n ast.Node) bool {
	switch n := n.(type) {
	case *ast.BinaryExpr:
		// ADD to AND_NOT: arithmetic, bitwise and shift operators.
		if n.Op >= token.ADD && n.Op <= token.AND_NOT {
			c.operators++
		}
	case *ast.UnaryExpr:
		if n.Op == token.SUB || n.Op == token.XOR {
			c.operators++
		}
	case *ast.IncDecStmt:
		c.operators++
	case *ast.AssignStmt:
		if n.Tok >= token.ADD_ASSIGN && n.Tok <= token.AND_NOT_ASSIGN {
			c.operators++
		}
	case *ast.IndexExpr:
		// A generic function is a value of signature type, not something to index.
		tv := c.info.Types[n.X]
		if _, generic := tv.Type.(*types.Signature); tv.IsValue() && !generic {
			c.indexing++
		}
	case *ast.SliceExpr:
		c.indexing++
	case *ast.BasicLit:
		c.literals++
	case *ast.CompositeLit:
		c.literals++
	case *ast.CallExpr:
		c.call(n)
	}
	return true
}

// call records a call that is not a conversion or a builtin.
func (c *stepCounter) call(n *ast.CallExpr) {
	if tv, ok := c.info.Types[n.Fun]; ok && (tv.IsType() || tv.IsBuiltin()) {
		return
	}
	call := sdk.StepCall{Site: span(c.fset, n.Lparen, n.Lparen+1)}
	switch f := callee(n.Fun).(type) {
	case *ast.Ident:
		call.Name = f.Name
		call.Site = span(c.fset, f.Pos(), f.End())
	case *ast.SelectorExpr:
		call.Name = f.Sel.Name
		x, _ := f.X.(*ast.Ident)
		if _, pkg := c.info.Uses[x].(*types.PkgName); pkg {
			call.Name = x.Name + "." + f.Sel.Name
		}
		call.Site = span(c.fset, f.Sel.Pos(), f.Sel.End())
	}
	c.calls = append(c.calls, call)
}

// steps reports the top-level statements of a body with the syntax counted
// inside each.
func (u *unit) steps(d *ast.FuncDecl) []sdk.Step {
	var out []sdk.Step
	for _, s := range d.Body.List {
		if _, empty := s.(*ast.EmptyStmt); empty {
			continue
		}
		out = append(out, u.step(s))
	}
	return out
}

func (u *unit) step(s ast.Stmt) sdk.Step {
	info := u.info()
	c := &stepCounter{info: info, fset: u.fset}
	ast.Inspect(s, c.visit)
	f := &flow{info: info, events: []sdk.Flow{}}
	f.walk(s)
	step := sdk.Step{
		Span:      span(u.fset, s.Pos(), s.End()),
		Kind:      stepKind(s),
		Operators: c.operators,
		Indexing:  c.indexing,
		Literals:  c.literals,
		Calls:     c.calls,
		Nested:    f.count,
	}
	if i, ok := s.(*ast.IfStmt); ok {
		step.Guard = i.Else == nil && f.onlyJumps(i.Body)
	}
	sort.SliceStable(step.Calls, func(a, b int) bool {
		x, y := step.Calls[a].Site.Start, step.Calls[b].Site.Start
		return x.Line < y.Line || x.Line == y.Line && x.Col < y.Col
	})
	return step
}

// stepKind is the normalized kind of a statement; a labeled statement has
// the kind of the statement it labels.
func stepKind(s ast.Stmt) string {
	switch s := s.(type) {
	case *ast.LabeledStmt:
		return stepKind(s.Stmt)
	case *ast.AssignStmt, *ast.IncDecStmt:
		return "assign"
	case *ast.DeclStmt:
		return "declare"
	case *ast.ReturnStmt:
		return "return"
	case *ast.BranchStmt:
		return "jump"
	case *ast.GoStmt, *ast.DeferStmt:
		return "defer"
	case *ast.IfStmt:
		return "if"
	case *ast.SwitchStmt, *ast.TypeSwitchStmt, *ast.SelectStmt:
		return "switch"
	case *ast.ForStmt, *ast.RangeStmt:
		return "loop"
	case *ast.BlockStmt:
		return "block"
	}
	return "call"
}
