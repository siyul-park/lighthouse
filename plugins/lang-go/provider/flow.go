package provider

import (
	"go/ast"
	"go/token"
	"go/types"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// flow walks one function body and normalizes its control flow.
type flow struct {
	info    *types.Info
	self    *types.Func
	nesting int
	deepest int
	count   int
	events  []sdk.Flow
}

const (
	flowIf        = "if"
	flowElseIf    = "else-if"
	flowElse      = "else"
	flowSwitch    = "switch"
	flowLoop      = "loop"
	flowJump      = "jump"
	flowLogic     = "logic"
	flowRecursion = "recursion"
)

func (f *flow) Visit(n ast.Node) ast.Visitor {
	switch n := n.(type) {
	case *ast.BlockStmt:
		f.count += statementCount(n.List)
	case *ast.IfStmt:
		f.ifStmt(n, false)
		return nil
	case *ast.ForStmt:
		f.loop(n.Body, n.Init, n.Cond, n.Post)
		return nil
	case *ast.RangeStmt:
		f.loop(n.Body, n.Key, n.Value, n.X)
		return nil
	case *ast.SwitchStmt:
		f.switchStmt(n.Body, false, n.Init, n.Tag)
		return nil
	case *ast.TypeSwitchStmt:
		f.switchStmt(n.Body, false, n.Init, n.Assign)
		return nil
	case *ast.SelectStmt:
		f.switchStmt(n.Body, true)
		return nil
	case *ast.FuncLit:
		f.nested(func() { f.walk(n.Body) })
		return nil
	case *ast.BinaryExpr:
		if isLogical(n) {
			f.logic(n)
			return nil
		}
	case *ast.BranchStmt:
		if isJump(n) {
			f.event(flowJump)
		}
	case *ast.CallExpr:
		if f.recursive(n) {
			f.event(flowRecursion)
		}
	}
	return f
}

func (f *flow) ifStmt(n *ast.IfStmt, chained bool) {
	if chained {
		f.event(flowElseIf)
	} else {
		f.event(flowIf)
	}
	f.walk(n.Init, n.Cond)
	f.nested(func() { f.walk(n.Body) })
	switch next := n.Else.(type) {
	case *ast.IfStmt:
		f.ifStmt(next, true)
	case *ast.BlockStmt:
		f.event(flowElse)
		f.nested(func() { f.walk(next) })
	}
}

func (f *flow) loop(body *ast.BlockStmt, header ...ast.Node) {
	f.event(flowLoop)
	f.walk(header...)
	f.nested(func() { f.walk(body) })
}

func (f *flow) switchStmt(body *ast.BlockStmt, comm bool, header ...ast.Node) {
	arms, returning := switchArms(body, comm)
	f.events = append(f.events, sdk.Flow{
		Kind:      flowSwitch,
		Nesting:   f.nesting,
		Arms:      arms,
		Returning: returning,
	})
	f.walk(header...)
	f.nested(func() {
		for _, clause := range body.List {
			f.count++
			switch c := clause.(type) {
			case *ast.CaseClause:
				for _, e := range c.List {
					f.walk(e)
				}
				f.clauseBody(c.Body)
			case *ast.CommClause:
				f.walk(c.Comm)
				f.clauseBody(c.Body)
			}
		}
	})
}

func (f *flow) clauseBody(list []ast.Stmt) {
	f.count += statementCount(list)
	for _, s := range list {
		f.walk(s)
	}
}

// logic emits one event per run of like operators in a chain of && and ||,
// looking through parentheses, then walks the operands.
func (f *flow) logic(n *ast.BinaryExpr) {
	ops, leaves := chain(n)
	for i := 0; i < len(ops); {
		j := i
		for j < len(ops) && ops[j] == ops[i] {
			j++
		}
		f.events = append(f.events, sdk.Flow{Kind: flowLogic, Nesting: f.nesting, Operators: j - i})
		i = j
	}
	for _, leaf := range leaves {
		f.walk(leaf)
	}
}

// recursive reports a call of the function being analyzed.
func (f *flow) recursive(n *ast.CallExpr) bool {
	if f.self == nil {
		return false
	}
	var fn *types.Func
	switch c := callee(n.Fun).(type) {
	case *ast.Ident:
		fn, _ = f.info.Uses[c].(*types.Func)
	case *ast.SelectorExpr:
		if s := f.info.Selections[c]; s != nil {
			fn, _ = s.Obj().(*types.Func)
		} else {
			fn, _ = f.info.Uses[c.Sel].(*types.Func)
		}
	}
	return fn != nil && fn.Origin() == f.self
}

func (f *flow) nested(fn func()) {
	f.nesting++
	f.deepest = max(f.deepest, f.nesting)
	fn()
	f.nesting--
}

func (f *flow) walk(nodes ...ast.Node) {
	for _, n := range nodes {
		if n != nil {
			ast.Walk(f, n)
		}
	}
}

func (f *flow) event(kind string) {
	f.events = append(f.events, sdk.Flow{Kind: kind, Nesting: f.nesting})
}

func newFlow(d *ast.FuncDecl, info *types.Info) *flow {
	f := &flow{info: info, events: []sdk.Flow{}}
	if fn, ok := info.Defs[d.Name].(*types.Func); ok {
		f.self = fn.Origin()
	}
	return f
}

// switchArms counts the non-default arms of a switch or select; a switch
// whose every arm is a lone return is returning.
func switchArms(body *ast.BlockStmt, comm bool) (arms int, returning bool) {
	returning = len(body.List) > 0 && !comm
	for _, clause := range body.List {
		switch c := clause.(type) {
		case *ast.CaseClause:
			if c.List != nil {
				arms++
			}
			returning = returning && len(c.Body) == 1 && isReturn(c.Body[0])
		case *ast.CommClause:
			if c.Comm != nil {
				arms++
			}
		}
	}
	return arms, returning
}

func isReturn(s ast.Stmt) bool {
	_, ok := s.(*ast.ReturnStmt)
	return ok
}

// chain flattens a tree of && and || into its operators and operands in
// source order, looking through parentheses.
func chain(e ast.Expr) ([]token.Token, []ast.Expr) {
	e = unparen(e)
	b, ok := e.(*ast.BinaryExpr)
	if !ok || !isLogical(b) {
		return nil, []ast.Expr{e}
	}
	leftOps, leftLeaves := chain(b.X)
	rightOps, rightLeaves := chain(b.Y)
	return append(append(leftOps, b.Op), rightOps...), append(leftLeaves, rightLeaves...)
}

// isJump reports a goto or a labeled break or continue.
func isJump(n *ast.BranchStmt) bool {
	return n.Tok == token.GOTO || (n.Label != nil && (n.Tok == token.BREAK || n.Tok == token.CONTINUE))
}

func isLogical(n *ast.BinaryExpr) bool { return n.Op == token.LAND || n.Op == token.LOR }

func unparen(e ast.Expr) ast.Expr {
	for {
		p, ok := e.(*ast.ParenExpr)
		if !ok {
			return e
		}
		e = p.X
	}
}

// statementCount counts the statements of a list, empty ones excluded.
func statementCount(list []ast.Stmt) int {
	n := 0
	for _, s := range list {
		if _, empty := s.(*ast.EmptyStmt); !empty {
			n++
		}
	}
	return n
}

// signature counts parameters and results, each name once, unnamed ones as
// one; a method's receiver is not a parameter.
func signature(t *ast.FuncType) (params, returns int) {
	count := func(list *ast.FieldList) int {
		n := 0
		if list != nil {
			for _, f := range list.List {
				n += max(len(f.Names), 1)
			}
		}
		return n
	}
	return count(t.Params), count(t.Results)
}
