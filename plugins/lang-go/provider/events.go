package provider

import (
	"go/ast"
	"go/constant"
	"go/token"
	"go/types"
	"slices"
	"strings"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

type eventWalker struct {
	*unit
	// inIs is set inside an `Is(error) bool` method, where comparing and
	// asserting on errors is the point.
	inIs  bool
	found []sdk.Event
}

// The kinds of body events, as the protocol names them.
const (
	eventPanic           = "panic"
	eventErrorCompare    = "error-compare"
	eventErrorAssert     = "error-assert"
	eventErrorfUnwrapped = "errorf-unwrapped"
)

// errorType is the predeclared `error`.
var errorType = types.Universe.Lookup("error").Type()

// allowedSentinels are the errors the standard library documents as returned
// unwrapped, by package path and name. go-errorlint lets one be compared only
// when it comes straight from the function that documents it; this check does
// not follow that flow and lets the comparison be.
var allowedSentinels = map[string]string{"io": "EOF", "database/sql": "ErrNoRows"}

// events are the places a body panics or handles an error in a way that
// stops working once the error is wrapped, as go-errorlint reads them: an
// error compared with == or != (or switched on), an assertion or type switch
// on an error, and a fmt.Errorf that formats an error without %w.
func (u *unit) events(d *ast.FuncDecl) []sdk.Event {
	if d.Body == nil {
		return nil
	}
	w := &eventWalker{unit: u, inIs: isErrorIs(d, u.info())}
	ast.Inspect(d.Body, w.visit)
	slices.SortStableFunc(w.found, func(a, b sdk.Event) int {
		if a.Span.Start.Line != b.Span.Start.Line {
			return a.Span.Start.Line - b.Span.Start.Line
		}
		return a.Span.Start.Col - b.Span.Start.Col
	})
	return w.found
}

func (w *eventWalker) visit(n ast.Node) bool {
	switch n := n.(type) {
	case *ast.CallExpr:
		w.call(n)
	case *ast.BinaryExpr:
		w.binary(n)
	case *ast.SwitchStmt:
		w.switchOn(n)
	case *ast.TypeAssertExpr:
		w.assert(n)
	case *ast.TypeSwitchStmt:
		w.typeSwitch(n)
	}
	return true
}

func (w *eventWalker) call(n *ast.CallExpr) {
	if id, ok := n.Fun.(*ast.Ident); ok {
		if b, ok := w.info().Uses[id].(*types.Builtin); ok && b.Name() == "panic" {
			w.add(eventPanic, n.Pos(), n.End(), "")
		}
		return
	}
	if isFmtErrorf(n, w.info()) {
		w.errorf(n)
	}
}

// errorf reports a fmt.Errorf whose format has no %w and whose arguments
// include an error; the detail is the verb that formats the first one.
func (w *eventWalker) errorf(n *ast.CallExpr) {
	lit, ok := n.Args[0].(*ast.BasicLit)
	if len(n.Args) < 2 || !ok {
		return
	}
	value := w.info().Types[lit].Value
	if value == nil || value.Kind() != constant.String {
		return
	}
	verbs, ok := formatVerbs(constant.StringVal(value))
	if !ok || slices.Contains(verbs, 'w') {
		return
	}
	for i, arg := range n.Args[1:] {
		if i < len(verbs) && hasErrorMethod(w.info().TypeOf(arg)) {
			w.add(eventErrorfUnwrapped, n.Pos(), n.End(), "%"+string(verbs[i]))
			return
		}
	}
}

// binary reports == and != between an error and anything but nil.
func (w *eventWalker) binary(n *ast.BinaryExpr) {
	if w.inIs || (n.Op != token.EQL && n.Op != token.NEQ) {
		return
	}
	info := w.info()
	x, y := info.Types[n.X], info.Types[n.Y]
	if x.IsNil() || y.IsNil() || !(isError(x.Type) || isError(y.Type)) {
		return
	}
	if allowedSentinel(n.X, info) || allowedSentinel(n.Y, info) {
		return
	}
	operand := n.X
	if !isError(x.Type) {
		operand = n.Y
	}
	w.add(eventErrorCompare, n.Pos(), n.End(), w.text(operand))
}

// switchOn reports a switch on an error that has a case other than nil.
func (w *eventWalker) switchOn(n *ast.SwitchStmt) {
	if w.inIs || n.Tag == nil || !isError(w.info().TypeOf(n.Tag)) {
		return
	}
	for _, stmt := range n.Body.List {
		clause, ok := stmt.(*ast.CaseClause)
		if !ok {
			continue
		}
		for _, expr := range clause.List {
			if !w.info().Types[expr].IsNil() && !allowedSentinel(expr, w.info()) {
				w.add(eventErrorCompare, n.Tag.Pos(), n.Tag.End(), w.text(n.Tag))
				return
			}
		}
	}
}

// assert reports x.(T) on an error x when T is an error type.
func (w *eventWalker) assert(n *ast.TypeAssertExpr) {
	if w.inIs || n.Type == nil || !isError(w.info().TypeOf(n.X)) {
		return
	}
	if hasErrorMethod(w.info().TypeOf(n.Type)) {
		w.add(eventErrorAssert, n.Pos(), n.End(), w.text(n.Type))
	}
}

// typeSwitch reports a type switch on an error.
func (w *eventWalker) typeSwitch(n *ast.TypeSwitchStmt) {
	var guard ast.Expr
	switch s := n.Assign.(type) {
	case *ast.ExprStmt:
		guard = s.X
	case *ast.AssignStmt:
		if len(s.Rhs) == 1 {
			guard = s.Rhs[0]
		}
	}
	assert, ok := guard.(*ast.TypeAssertExpr)
	if !ok || w.inIs || !isError(w.info().TypeOf(assert.X)) {
		return
	}
	w.add(eventErrorAssert, assert.Pos(), assert.End(), w.text(assert.X))
}

func (w *eventWalker) add(kind string, from, to token.Pos, detail string) {
	w.found = append(w.found, sdk.Event{Kind: kind, Span: span(w.fset, from, to), Detail: detail})
}

// text is the source of a node.
func (w *eventWalker) text(n ast.Node) string {
	start := w.fset.PositionFor(n.Pos(), false).Offset
	end := w.fset.PositionFor(n.End(), false).Offset
	if start < 0 || end > len(w.src) || start > end {
		return ""
	}
	return string(w.src[start:end])
}

// hasErrorMethod reports whether the type has the method `Error() string`.
func hasErrorMethod(t types.Type) bool {
	return t != nil && types.Implements(t, errorType.Underlying().(*types.Interface))
}

// isFmtErrorf reports a call of fmt.Errorf.
func isFmtErrorf(n *ast.CallExpr, info *types.Info) bool {
	sel, ok := n.Fun.(*ast.SelectorExpr)
	if !ok {
		return false
	}
	fn, ok := info.Uses[sel.Sel].(*types.Func)
	return ok && fn.Pkg() != nil && fn.Pkg().Path() == "fmt" && fn.Name() == "Errorf"
}

// allowedSentinel reports one of allowedSentinels.
func allowedSentinel(e ast.Expr, info *types.Info) bool {
	sel, ok := e.(*ast.SelectorExpr)
	if !ok {
		return false
	}
	v, ok := info.Uses[sel.Sel].(*types.Var)
	return ok && v.Pkg() != nil && allowedSentinels[v.Pkg().Path()] == v.Name()
}

// isErrorIs reports a method `Is(error) bool`.
func isErrorIs(d *ast.FuncDecl, info *types.Info) bool {
	if d.Recv == nil || d.Name.Name != "Is" {
		return false
	}
	fn, ok := info.Defs[d.Name].(*types.Func)
	if !ok {
		return false
	}
	sig := fn.Type().(*types.Signature)
	return sig.Params().Len() == 1 && isError(sig.Params().At(0).Type()) &&
		sig.Results().Len() == 1 && types.Identical(sig.Results().At(0).Type(), types.Typ[types.Bool])
}

// formatVerbs are the verbs of a printf format in argument order, `*` widths
// counted as arguments; false when the format indexes its arguments.
func formatVerbs(format string) ([]rune, bool) {
	var verbs []rune
	runes := []rune(format)
	for i := 0; i < len(runes); i++ {
		if runes[i] != '%' {
			continue
		}
		i++
		for i < len(runes) && strings.ContainsRune("+-# 0123456789.*[", runes[i]) {
			switch runes[i] {
			case '[':
				return nil, false
			case '*':
				verbs = append(verbs, '*')
			}
			i++
		}
		if i >= len(runes) {
			break
		}
		if runes[i] != '%' {
			verbs = append(verbs, runes[i])
		}
	}
	return verbs, true
}
