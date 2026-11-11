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
	}
	return true
}

// collect records the calls of a statement.
func (c *stepCounter) collect(n ast.Node) bool {
	if call, ok := n.(*ast.CallExpr); ok {
		c.call(call)
	}
	return true
}

// call records a call that is not a conversion or a builtin, named as
// written, or `importpath.Func` and `importpath.Type.Method` when it resolves.
func (c *stepCounter) call(n *ast.CallExpr) {
	if tv, ok := c.info.Types[n.Fun]; ok && (tv.IsType() || tv.IsBuiltin()) {
		return
	}
	call := sdk.StepCall{Site: span(c.fset, n.Lparen, n.Lparen+1)}
	switch f := callee(n.Fun).(type) {
	case *ast.Ident:
		call.Name, call.Site = f.Name, span(c.fset, f.Pos(), f.End())
	case *ast.SelectorExpr:
		call.Name, call.Site = c.written(f), span(c.fset, f.Sel.Pos(), f.Sel.End())
	}
	if fn, on := calledFunc(c.info, n); fn != nil {
		call.Name = qualified(fn, on)
	}
	c.calls = append(c.calls, call)
}

// written names the callee of a call that resolves to no function, as the
// package path and the name for a package-level value (`pkg.Var`).
func (c *stepCounter) written(f *ast.SelectorExpr) string {
	if x, ok := f.X.(*ast.Ident); ok {
		if pkg, ok := c.info.Uses[x].(*types.PkgName); ok {
			return pkg.Imported().Path() + "." + f.Sel.Name
		}
	}
	return f.Sel.Name
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
	ast.Inspect(s, c.collect)
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

// qualified names a function `importpath.Func`, and a method
// `importpath.Type.Method` on the type it is called on, the pointer stripped
// (`error.Error` for a universe type, a local type by its package). A method
// on a type parameter or an unnamed type is `.Method`, as Rust writes it.
func qualified(fn *types.Func, on types.Type) string {
	sig, _ := fn.Type().(*types.Signature)
	if sig == nil || sig.Recv() == nil {
		if fn.Pkg() == nil {
			return fn.Name()
		}
		return fn.Pkg().Path() + "." + fn.Name()
	}
	if on == nil {
		on = sig.Recv().Type()
	}
	if p, ok := types.Unalias(on).(*types.Pointer); ok {
		on = p.Elem()
	}
	named, ok := types.Unalias(on).(*types.Named)
	if !ok {
		return "." + fn.Name()
	}
	owner := named.Origin().Obj()
	if owner.Pkg() == nil {
		return owner.Name() + "." + fn.Name()
	}
	return owner.Pkg().Path() + "." + owner.Name() + "." + fn.Name()
}
