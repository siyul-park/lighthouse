package provider

import (
	"go/ast"
	"go/scanner"
	"go/token"
	"go/types"
	"path"
	"slices"
	"strconv"
	"strings"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"golang.org/x/tools/go/packages"
)

// unit is one requested file together with the package that type-checked it
// and the fragment being built from it.
type unit struct {
	rel  string
	file *ast.File
	pkg  *packages.Package
	fset *token.FileSet
	src  []byte
	res  *resolver
	frag *sdk.Fragment

	module   string
	testOf   string
	main     bool
	internal bool
	test     bool
	edges    map[edgeKey]bool
	cmap     ast.CommentMap
}

// edgeKey identifies an emitted edge: the same relation at another site is
// another edge.
type edgeKey struct {
	kind, to string
	from     sdk.Node
	site     sdk.Span
	sited    bool
}

func newUnit(rel string, file *ast.File, pkg *packages.Package, src []byte, res *resolver) *unit {
	dir := path.Dir(rel)
	u := &unit{
		rel:      rel,
		file:     file,
		pkg:      pkg,
		fset:     pkg.Fset,
		src:      src,
		res:      res,
		frag:     emptyFragment(sdk.FileInfo{Path: rel, Generated: isGenerated(file)}),
		module:   dir,
		main:     file.Name.Name == "main",
		internal: hasComponent(dir, "internal"),
		test:     strings.HasSuffix(rel, "_test.go"),
		edges:    map[edgeKey]bool{},
		cmap:     ast.NewCommentMap(pkg.Fset, file, file.Comments),
	}
	if u.test && strings.HasSuffix(file.Name.Name, "_test") {
		u.module = dir + moduleSuffixTest
		u.testOf = dir
	}
	u.frag.Modules = []sdk.Module{{Path: u.module, Name: file.Name.Name, TestOf: u.testOf}}
	return u
}

// fragment collects the symbols, edges, function summaries and test cases of
// the file, plus its comment groups.
func (u *unit) fragment() *sdk.Fragment {
	u.imports()
	for _, decl := range u.file.Decls {
		switch d := decl.(type) {
		case *ast.FuncDecl:
			u.function(d)
		case *ast.GenDecl:
			u.general(d)
		}
	}
	u.comments()
	return u.frag
}

func (u *unit) imports() {
	from := sdk.Node{Module: u.module}
	for _, spec := range u.file.Imports {
		path, err := strconv.Unquote(spec.Path.Value)
		if err != nil || path == "C" {
			continue
		}
		to := u.res.importPath(path)
		if imported, ok := u.pkg.Imports[path]; ok {
			to = u.res.importPath(imported.PkgPath)
		}
		u.edge(edgeImports, from, to)
	}
}

func (u *unit) function(d *ast.FuncDecl) {
	name := d.Name.Name
	if name == "_" {
		return
	}
	doc := docText(d.Doc)
	if d.Recv == nil {
		kind := kindFunction
		if u.test && isTestEntry(name, testEntryPrefixes...) {
			kind = kindTest
		}
		idName := name
		if name == "init" {
			idName = "init:" + path.Base(u.rel) + ":" + strconv.Itoa(position(u.fset, d.Pos()).Line)
		}
		id := symbolID(kind, u.module, idName)
		u.symbol(sdk.Symbol{ID: id, Kind: kind, Name: name, Owner: "", Doc: doc}, d.Pos(), d.End(), u.extent(d))
		if u.test && kind == kindFunction {
			u.frag.Symbols[len(u.frag.Symbols)-1].Role = u.functionRole(d)
		}
		u.summarize(d, id, kind == kindTest && isTestEntry(name, "Test"))
		return
	}
	owner := u.receiverType(d)
	if owner == "" {
		return
	}
	id := symbolID(kindMethod, u.module, owner, name)
	u.symbol(sdk.Symbol{ID: id, Kind: kindMethod, Name: name, Owner: symbolID(kindType, u.module, owner), Doc: doc}, d.Pos(), d.End(), u.extent(d))
	u.summarize(d, id, false)
}

// summarize records the function summary, the body's call and reference
// edges and, for test entry points, the test case.
func (u *unit) summarize(d *ast.FuncDecl, id string, isTestCase bool) {
	if d.Body == nil {
		return
	}
	usages, occurrences := u.usages(d)
	u.siteEdges(id, occurrences)
	f := newFlow(d, u.info())
	ast.Walk(f, d.Body)
	params, returns := signature(d.Type)
	manual := 0
	if u.test {
		manual = manualAssertions(d.Body, u.info())
	}
	u.frag.Functions = append(u.frag.Functions, sdk.FunctionSummary{
		Symbol:           id,
		MaxNesting:       f.deepest,
		Statements:       f.count,
		TopLevel:         statementCount(d.Body.List),
		Params:           params,
		Returns:          returns,
		Tokens:           u.tokens(d.Body),
		Flow:             f.events,
		ForwardsTo:       forwardTarget(d, u.info(), u.res),
		Signature:        u.signature(d),
		Events:           u.events(d),
		Implementation:   u.implementation(d),
		Constructs:       u.constructs(d),
		ManualAssertions: manual,
	})
	if isTestCase {
		u.frag.Tests = append(u.frag.Tests, testCaseOf(d, id, usages, u.info()))
	}
}

// siteEdges emits an edge for every place a function uses something, each
// with the span of the identifier that names the target.
func (u *unit) siteEdges(from string, occurrences []occurrence) {
	for _, use := range occurrences {
		site := span(u.fset, use.at.Pos(), use.at.End())
		u.edgeAt(use.kind, sdk.Node{Symbol: from}, use.to, &site)
	}
}

// tokens counts the leaf tokens of a body, comments and the semicolons the
// scanner inserts excluded.
func (u *unit) tokens(body *ast.BlockStmt) int {
	start := u.fset.PositionFor(body.Lbrace, false)
	end := u.fset.PositionFor(body.Rbrace, false)
	if start.Offset < 0 || end.Offset >= len(u.src) || start.Offset > end.Offset {
		return 0
	}
	text := u.src[start.Offset : end.Offset+1]
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

// receiverType names the type a method belongs to: the declared type behind
// aliases, pointers and type arguments.
func (u *unit) receiverType(d *ast.FuncDecl) string {
	if fn, ok := u.info().Defs[d.Name].(*types.Func); ok {
		if sig, ok := fn.Type().(*types.Signature); ok && sig.Recv() != nil {
			if owner, ok := namedType(sig.Recv().Type()); ok {
				return owner.Name()
			}
		}
	}
	return baseName(d.Recv.List[0].Type)
}

func (u *unit) general(d *ast.GenDecl) {
	for _, spec := range d.Specs {
		switch s := spec.(type) {
		case *ast.TypeSpec:
			u.typeSpec(s, d)
		case *ast.ValueSpec:
			kind := kindVar
			if d.Tok == token.CONST {
				kind = kindConst
			}
			var extent *sdk.Span
			if len(s.Names) == 1 {
				extent = u.specExtent(d, s)
			}
			for i, name := range s.Names {
				if name.Name == "_" {
					continue
				}
				id := symbolID(kind, u.module, name.Name)
				u.symbol(sdk.Symbol{ID: id, Kind: kind, Name: name.Name, Owner: "", Doc: docText(s.Doc, d.Doc)}, name.Pos(), s.End(), extent)
				if kind == kindVar {
					u.initializer(id, initializersOf(s, i))
				}
			}
		}
	}
}

// initializer records what the initializer of a package variable uses, as
// edges of the variable: a function named in a table of handlers is used there.
func (u *unit) initializer(from string, values []ast.Expr) {
	w := &uses{
		res:     u.res,
		info:    u.info(),
		handled: map[ast.Node]bool{},
		seen:    map[usage]bool{},
	}
	for _, value := range values {
		ast.Walk(w, value)
	}
	u.siteEdges(from, w.all)
}

func (u *unit) typeSpec(s *ast.TypeSpec, d *ast.GenDecl) {
	name := s.Name.Name
	if name == "_" {
		return
	}
	kind := kindType
	if _, ok := s.Type.(*ast.InterfaceType); ok {
		kind = kindInterface
	}
	id := symbolID(kind, u.module, name)
	u.symbol(sdk.Symbol{ID: id, Kind: kind, Name: name, Owner: "", Doc: docText(s.Doc, d.Doc)}, s.Pos(), s.End(), u.specExtent(d, s))
	switch t := s.Type.(type) {
	case *ast.StructType:
		u.fields(t, name, id)
	case *ast.InterfaceType:
		u.interfaceMethods(t, name, id)
	}
}

func (u *unit) fields(t *ast.StructType, owner, ownerID string) {
	for _, field := range t.Fields.List {
		doc := docText(field.Doc)
		optional, ref := u.optional(field), u.fieldType(field)
		if len(field.Names) == 0 {
			name := baseName(field.Type)
			if name != "" && name != "_" {
				id := symbolID(kindField, u.module, owner, name)
				u.symbol(sdk.Symbol{ID: id, Kind: kindField, Name: name, Owner: ownerID, Doc: doc, Optional: optional, TypeRef: ref}, field.Pos(), field.End(), u.extent(field))
			}
			continue
		}
		for _, name := range field.Names {
			if name.Name == "_" {
				continue
			}
			id := symbolID(kindField, u.module, owner, name.Name)
			var extent *sdk.Span
			if len(field.Names) == 1 {
				extent = u.extent(field)
			}
			u.symbol(sdk.Symbol{ID: id, Kind: kindField, Name: name.Name, Owner: ownerID, Doc: doc, Optional: optional, TypeRef: ref}, name.Pos(), field.End(), extent)
		}
	}
}

// optional reports whether a caller may leave the field out: its zero value is
// nil (a pointer, slice, map, function, channel or interface). A field typed
// by a type parameter is not: its constraint says nothing about the value.
func (u *unit) optional(field *ast.Field) bool {
	t := u.info().TypeOf(field.Type)
	if t == nil {
		return false
	}
	if _, generic := types.Unalias(t).(*types.TypeParam); generic {
		return false
	}
	switch t.Underlying().(type) {
	case *types.Pointer, *types.Slice, *types.Map, *types.Signature, *types.Chan, *types.Interface:
		return true
	}
	return false
}

func (u *unit) interfaceMethods(t *ast.InterfaceType, owner, ownerID string) {
	for _, method := range t.Methods.List {
		for _, name := range method.Names {
			id := symbolID(kindMethod, u.module, owner, name.Name)
			var extent *sdk.Span
			if len(method.Names) == 1 {
				extent = u.extent(method)
			}
			u.symbol(sdk.Symbol{ID: id, Kind: kindMethod, Name: name.Name, Owner: ownerID, Doc: docText(method.Doc)}, name.Pos(), method.End(), extent)
		}
	}
}

// functionRole is the role of a function of a test file: a helper when it
// takes the testing handle, a fixture otherwise.
func (u *unit) functionRole(d *ast.FuncDecl) string {
	if fn, ok := u.info().Defs[d.Name].(*types.Func); ok {
		if sig, ok := fn.Type().(*types.Signature); ok && takesTestingHandle(sig) {
			return roleTestHelper
		}
	}
	return roleFixture
}

// symbol records a declaration: sym carries what the caller knows (identity,
// kind, name, owner, doc, optional); the rest is the unit's.
func (u *unit) symbol(sym sdk.Symbol, from, to token.Pos, extent *sdk.Span) {
	sym.Visibility = u.visibility(sym.Name)
	sym.File = u.rel
	sym.Span = span(u.fset, from, to)
	sym.Extent = extent
	sym.Role = u.declarationRole(sym.Kind, sym.Owner)
	u.frag.Symbols = append(u.frag.Symbols, sym)
	container := sdk.Node{Module: u.module}
	if sym.Owner != "" {
		container = sdk.Node{Symbol: sym.Owner}
	}
	u.edge(edgeContains, container, sym.ID)
}

// declarationRole is the role of a type, constant or variable of a test
// file, which is always a fixture.
func (u *unit) declarationRole(kind, owner string) string {
	switch {
	case !u.test || owner != "":
		return ""
	case kind == kindType || kind == kindInterface || kind == kindConst || kind == kindVar:
		return roleFixture
	default:
		return ""
	}
}

func (u *unit) edge(kind string, from sdk.Node, to string) { u.edgeAt(kind, from, to, nil) }

// edgeAt emits an edge once per distinct relation and site.
func (u *unit) edgeAt(kind string, from sdk.Node, to string, site *sdk.Span) {
	key := edgeKey{kind: kind, from: from, to: to}
	if site != nil {
		key.site, key.sited = *site, true
	}
	if u.edges[key] {
		return
	}
	u.edges[key] = true
	u.frag.Edges = append(u.frag.Edges, sdk.Edge{
		Kind: kind, From: from, To: to, Resolution: resolutionSemantic, Site: site,
	})
}

// visibility follows Go's capitalization, tightened by where the symbol
// lives: nothing of a main package is importable and symbols under an
// internal directory are exported only inside the project.
func (u *unit) visibility(name string) string {
	switch {
	case u.main || !ast.IsExported(name):
		return visibilityPrivate
	case u.internal:
		return visibilityInternal
	default:
		return visibilityPublic
	}
}

func (u *unit) info() *types.Info { return u.pkg.TypesInfo }

// baseName is the identifier a type expression names, behind pointers,
// parentheses, type arguments and package qualifiers.
func baseName(e ast.Expr) string {
	for {
		switch t := e.(type) {
		case *ast.Ident:
			return t.Name
		case *ast.StarExpr:
			e = t.X
		case *ast.ParenExpr:
			e = t.X
		case *ast.IndexExpr:
			e = t.X
		case *ast.IndexListExpr:
			e = t.X
		case *ast.SelectorExpr:
			return t.Sel.Name
		default:
			return ""
		}
	}
}

func hasComponent(dir, name string) bool {
	return slices.Contains(strings.Split(dir, "/"), name)
}

// initializersOf are the expressions that give the i-th name of a spec its
// value: its own, or all of them when one call initializes several names.
func initializersOf(s *ast.ValueSpec, i int) []ast.Expr {
	if len(s.Values) == len(s.Names) {
		return s.Values[i : i+1]
	}
	if i == 0 {
		return s.Values
	}
	return nil
}
