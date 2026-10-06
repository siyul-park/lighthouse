package provider

import (
	"go/ast"
	"go/scanner"
	"go/token"
	"go/types"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

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

// summarize records the function summary, the body's call and reference
// edges and, for test entry points, the test case.
func (b *builder) summarize(d *ast.FuncDecl, id string, testCase bool) {
	if d.Body == nil {
		return
	}
	from := sdk.Node{Symbol: id}
	list := collectUses(b.u, d)
	for _, use := range list {
		b.edge(use.kind, from, use.to)
	}
	f := &flow{info: b.u.info(), self: b.selfFunc(d)}
	ast.Walk(f, d.Body)
	top := 0
	for _, s := range d.Body.List {
		if _, empty := s.(*ast.EmptyStmt); !empty {
			top++
		}
	}
	params, returns := signature(d.Type)
	summary := sdk.FunctionSummary{
		Symbol:     id,
		MaxNesting: f.deepest,
		Statements: f.count,
		TopLevel:   top,
		Params:     params,
		Returns:    returns,
		Tokens:     b.tokens(d.Body),
		Flow:       f.events,
	}
	if summary.Flow == nil {
		summary.Flow = []sdk.Flow{}
	}
	summary.ForwardsTo = b.forwards(d)
	b.frag.Functions = append(b.frag.Functions, summary)
	if testCase {
		b.testCase(d, id, list)
	}
}

func (b *builder) selfFunc(d *ast.FuncDecl) *types.Func {
	if fn, ok := b.u.info().Defs[d.Name].(*types.Func); ok {
		return fn.Origin()
	}
	return nil
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

// tokens counts the leaf tokens of a body, comments and the semicolons the
// scanner inserts excluded.
func (b *builder) tokens(body *ast.BlockStmt) int {
	start := b.u.fset.PositionFor(body.Lbrace, false)
	end := b.u.fset.PositionFor(body.Rbrace, false)
	if start.Offset < 0 || end.Offset >= len(b.u.src) || start.Offset > end.Offset {
		return 0
	}
	text := b.u.src[start.Offset : end.Offset+1]
	fset := token.NewFileSet()
	var s scanner.Scanner
	s.Init(fset.AddFile("", fset.Base(), len(text)), text, nil, 0)
	n := 0
	for {
		_, tok, lit := s.Scan()
		if tok == token.EOF {
			return n
		}
		if tok == token.SEMICOLON && lit == "\n" {
			continue
		}
		n++
	}
}

// flow walks one function body and normalizes its control flow.
type flow struct {
	info    *types.Info
	self    *types.Func
	nesting int
	deepest int
	count   int
	events  []sdk.Flow
}

func (f *flow) event(kind string) {
	f.events = append(f.events, sdk.Flow{Kind: kind, Nesting: f.nesting})
}

func (f *flow) nested(fn func()) {
	f.nesting++
	f.deepest = max(f.deepest, f.nesting)
	fn()
	f.nesting--
}

func (f *flow) statements(list []ast.Stmt) {
	for _, s := range list {
		if _, empty := s.(*ast.EmptyStmt); !empty {
			f.count++
		}
	}
}

func (f *flow) Visit(n ast.Node) ast.Visitor {
	switch n := n.(type) {
	case *ast.BlockStmt:
		f.statements(n.List)
	case *ast.IfStmt:
		f.ifStmt(n, false)
		return nil
	case *ast.ForStmt:
		f.event(flowLoop)
		f.walk(n.Init, n.Cond, n.Post)
		f.nested(func() { f.walk(n.Body) })
		return nil
	case *ast.RangeStmt:
		f.event(flowLoop)
		f.walk(n.Key, n.Value, n.X)
		f.nested(func() { f.walk(n.Body) })
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
		if n.Tok == token.GOTO || (n.Label != nil && (n.Tok == token.BREAK || n.Tok == token.CONTINUE)) {
			f.event(flowJump)
		}
	case *ast.CallExpr:
		if f.recursive(n) {
			f.event(flowRecursion)
		}
	}
	return f
}

func (f *flow) walk(nodes ...ast.Node) {
	for _, n := range nodes {
		if n != nil {
			ast.Walk(f, n)
		}
	}
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

func (f *flow) switchStmt(body *ast.BlockStmt, comm bool, header ...ast.Node) {
	arms, returning := 0, len(body.List) > 0 && !comm
	for _, clause := range body.List {
		switch c := clause.(type) {
		case *ast.CaseClause:
			if c.List != nil {
				arms++
			}
			returning = returning && returnsOnly(c.Body)
		case *ast.CommClause:
			if c.Comm != nil {
				arms++
			}
		}
	}
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
				f.statements(c.Body)
				for _, s := range c.Body {
					f.walk(s)
				}
			case *ast.CommClause:
				f.walk(c.Comm)
				f.statements(c.Body)
				for _, s := range c.Body {
					f.walk(s)
				}
			}
		}
	})
}

func returnsOnly(list []ast.Stmt) bool {
	if len(list) != 1 {
		return false
	}
	_, ok := list[0].(*ast.ReturnStmt)
	return ok
}

func isLogical(n *ast.BinaryExpr) bool { return n.Op == token.LAND || n.Op == token.LOR }

// logic emits one event per run of like operators in a chain of && and ||,
// looking through parentheses, then walks the operands.
func (f *flow) logic(n *ast.BinaryExpr) {
	var ops []token.Token
	var leaves []ast.Expr
	var flatten func(e ast.Expr)
	flatten = func(e ast.Expr) {
		e = unparen(e)
		b, ok := e.(*ast.BinaryExpr)
		if !ok || !isLogical(b) {
			leaves = append(leaves, e)
			return
		}
		flatten(b.X)
		ops = append(ops, b.Op)
		flatten(b.Y)
	}
	flatten(n)
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

func unparen(e ast.Expr) ast.Expr {
	for {
		p, ok := e.(*ast.ParenExpr)
		if !ok {
			return e
		}
		e = p.X
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
