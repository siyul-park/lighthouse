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
	edges    map[sdk.Edge]bool
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
		frag:     emptyFragment(sdk.FileInfo{Path: rel, Generated: ast.IsGenerated(file)}),
		module:   dir,
		main:     file.Name.Name == "main",
		internal: hasComponent(dir, "internal"),
		test:     strings.HasSuffix(rel, "_test.go"),
		edges:    map[sdk.Edge]bool{},
	}
	if u.test && strings.HasSuffix(file.Name.Name, "_test") {
		u.module = dir + moduleSuffixTest
		u.testOf = dir
	}
	u.frag.Modules = []sdk.Module{{Path: u.module, Name: file.Name.Name, TestOf: u.testOf}}
	return u
}

// fragment collects the symbols, edges, function summaries and test cases of
// the file.
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
		u.symbol(kind, name, "", id, d.Pos(), d.End(), doc)
		u.summarize(d, id, kind == kindTest && isTestEntry(name, "Test"))
		return
	}
	owner := u.receiverType(d)
	if owner == "" {
		return
	}
	id := symbolID(kindMethod, u.module, owner, name)
	u.symbol(kindMethod, name, symbolID(kindType, u.module, owner), id, d.Pos(), d.End(), doc)
	u.summarize(d, id, false)
}

// summarize records the function summary, the body's call and reference
// edges and, for test entry points, the test case.
func (u *unit) summarize(d *ast.FuncDecl, id string, isTestCase bool) {
	if d.Body == nil {
		return
	}
	usages := collectUses(u, d)
	for _, use := range usages {
		u.edge(use.kind, sdk.Node{Symbol: id}, use.to)
	}
	f := newFlow(d, u.info())
	ast.Walk(f, d.Body)
	params, returns := signature(d.Type)
	u.frag.Functions = append(u.frag.Functions, sdk.FunctionSummary{
		Symbol:     id,
		MaxNesting: f.deepest,
		Statements: f.count,
		TopLevel:   statementCount(d.Body.List),
		Params:     params,
		Returns:    returns,
		Tokens:     u.tokens(d.Body),
		Flow:       f.events,
		ForwardsTo: forwardTarget(d, u.info(), u.res),
	})
	if isTestCase {
		u.frag.Tests = append(u.frag.Tests, testCaseOf(d, id, usages, u.info()))
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
			for _, name := range s.Names {
				if name.Name == "_" {
					continue
				}
				id := symbolID(kind, u.module, name.Name)
				u.symbol(kind, name.Name, "", id, name.Pos(), s.End(), docText(s.Doc, d.Doc))
			}
		}
	}
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
	u.symbol(kind, name, "", id, s.Pos(), s.End(), docText(s.Doc, d.Doc))
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
		if len(field.Names) == 0 {
			name := baseName(field.Type)
			if name != "" && name != "_" {
				id := symbolID(kindField, u.module, owner, name)
				u.symbol(kindField, name, ownerID, id, field.Pos(), field.End(), doc)
			}
			continue
		}
		for _, name := range field.Names {
			if name.Name == "_" {
				continue
			}
			id := symbolID(kindField, u.module, owner, name.Name)
			u.symbol(kindField, name.Name, ownerID, id, name.Pos(), field.End(), doc)
		}
	}
}

func (u *unit) interfaceMethods(t *ast.InterfaceType, owner, ownerID string) {
	for _, method := range t.Methods.List {
		for _, name := range method.Names {
			id := symbolID(kindMethod, u.module, owner, name.Name)
			u.symbol(kindMethod, name.Name, ownerID, id, name.Pos(), method.End(), docText(method.Doc))
		}
	}
}

func (u *unit) symbol(kind, name, owner, id string, from, to token.Pos, doc string) {
	u.frag.Symbols = append(u.frag.Symbols, sdk.Symbol{
		ID:         id,
		Kind:       kind,
		Visibility: u.visibility(name),
		Owner:      owner,
		File:       u.rel,
		Span:       span(u.fset, from, to),
		Doc:        doc,
		Name:       name,
	})
	container := sdk.Node{Module: u.module}
	if owner != "" {
		container = sdk.Node{Symbol: owner}
	}
	u.edge(edgeContains, container, id)
}

func (u *unit) edge(kind string, from sdk.Node, to string) {
	e := sdk.Edge{Kind: kind, From: from, To: to, Resolution: resolutionSemantic}
	if !u.edges[e] {
		u.edges[e] = true
		u.frag.Edges = append(u.frag.Edges, e)
	}
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
