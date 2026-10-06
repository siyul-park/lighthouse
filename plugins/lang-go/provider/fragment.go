package provider

import (
	"go/ast"
	"go/token"
	"go/types"
	"path"
	"strconv"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// builder accumulates the fragment of one file.
type builder struct {
	u     *unit
	frag  sdk.Fragment
	edges map[sdk.Edge]bool
}

func buildFragment(u *unit) sdk.Fragment {
	b := &builder{
		u: u,
		frag: sdk.Fragment{
			File: sdk.FileInfo{Path: u.rel, Generated: ast.IsGenerated(u.file)},
			Modules: []sdk.Module{{
				Path:   u.module,
				Name:   u.file.Name.Name,
				TestOf: u.testOf,
			}},
			Symbols:   []sdk.Symbol{},
			Edges:     []sdk.Edge{},
			Functions: []sdk.FunctionSummary{},
			Tests:     []sdk.TestCase{},
		},
		edges: map[sdk.Edge]bool{},
	}
	b.imports()
	for _, decl := range u.file.Decls {
		switch d := decl.(type) {
		case *ast.FuncDecl:
			b.function(d)
		case *ast.GenDecl:
			b.general(d)
		}
	}
	return b.frag
}

func (b *builder) edge(kind string, from sdk.Node, to string) {
	e := sdk.Edge{Kind: kind, From: from, To: to, Resolution: resolutionSemantic}
	if !b.edges[e] {
		b.edges[e] = true
		b.frag.Edges = append(b.frag.Edges, e)
	}
}

func (b *builder) imports() {
	from := sdk.Node{Module: b.u.module}
	for _, spec := range b.u.file.Imports {
		path, err := strconv.Unquote(spec.Path.Value)
		if err != nil || path == "C" {
			continue
		}
		to := b.u.res.importPath(path)
		if imported, ok := b.u.pkg.Imports[path]; ok {
			to = b.u.res.importPath(imported.PkgPath)
		}
		b.edge(edgeImports, from, to)
	}
}

func (b *builder) symbol(kind, name, owner string, id string, from, to token.Pos, doc string) {
	b.frag.Symbols = append(b.frag.Symbols, sdk.Symbol{
		ID:         id,
		Kind:       kind,
		Visibility: b.u.visibility(name),
		Owner:      owner,
		File:       b.u.rel,
		Span:       span(b.u.fset, from, to),
		Doc:        doc,
		Name:       name,
	})
	container := sdk.Node{Module: b.u.module}
	if owner != "" {
		container = sdk.Node{Symbol: owner}
	}
	b.edge(edgeContains, container, id)
}

func (b *builder) function(d *ast.FuncDecl) {
	name := d.Name.Name
	if name == "_" {
		return
	}
	doc := docText(d.Doc)
	if d.Recv == nil {
		kind := kindFunction
		if b.u.test && isTestName(name) {
			kind = kindTest
		}
		idName := name
		if name == "init" {
			idName = "init:" + path.Base(b.u.rel) + ":" + strconv.Itoa(position(b.u.fset, d.Pos()).Line)
		}
		id := symbolID(kind, b.u.module, idName)
		b.symbol(kind, name, "", id, d.Pos(), d.End(), doc)
		b.summarize(d, id, kind == kindTest && isTestCaseName(name))
		return
	}
	owner := b.receiverType(d)
	if owner == "" {
		return
	}
	id := symbolID(kindMethod, b.u.module, owner, name)
	b.symbol(kindMethod, name, symbolID(kindType, b.u.module, owner), id, d.Pos(), d.End(), doc)
	b.summarize(d, id, false)
}

// receiverType names the type a method belongs to: the declared type behind
// aliases, pointers and type arguments.
func (b *builder) receiverType(d *ast.FuncDecl) string {
	if fn, ok := b.u.info().Defs[d.Name].(*types.Func); ok {
		if sig, ok := fn.Type().(*types.Signature); ok && sig.Recv() != nil {
			if owner, ok := namedType(sig.Recv().Type()); ok {
				return owner.Name()
			}
		}
	}
	return baseName(d.Recv.List[0].Type)
}

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

func (b *builder) general(d *ast.GenDecl) {
	for _, spec := range d.Specs {
		switch s := spec.(type) {
		case *ast.TypeSpec:
			b.typeSpec(s, d)
		case *ast.ValueSpec:
			kind := kindVar
			if d.Tok == token.CONST {
				kind = kindConst
			}
			for _, name := range s.Names {
				if name.Name == "_" {
					continue
				}
				id := symbolID(kind, b.u.module, name.Name)
				b.symbol(kind, name.Name, "", id, name.Pos(), s.End(), docText(s.Doc, d.Doc))
			}
		}
	}
}

func (b *builder) typeSpec(s *ast.TypeSpec, d *ast.GenDecl) {
	name := s.Name.Name
	if name == "_" {
		return
	}
	kind := kindType
	if _, ok := s.Type.(*ast.InterfaceType); ok {
		kind = kindInterface
	}
	id := symbolID(kind, b.u.module, name)
	b.symbol(kind, name, "", id, s.Pos(), s.End(), docText(s.Doc, d.Doc))
	switch t := s.Type.(type) {
	case *ast.StructType:
		b.fields(t, name, id)
	case *ast.InterfaceType:
		b.interfaceMethods(t, name, id)
	}
}

func (b *builder) fields(t *ast.StructType, owner, ownerID string) {
	for _, field := range t.Fields.List {
		doc := docText(field.Doc)
		if len(field.Names) == 0 {
			name := baseName(field.Type)
			if name != "" && name != "_" {
				id := symbolID(kindField, b.u.module, owner, name)
				b.symbol(kindField, name, ownerID, id, field.Pos(), field.End(), doc)
			}
			continue
		}
		for _, name := range field.Names {
			if name.Name == "_" {
				continue
			}
			id := symbolID(kindField, b.u.module, owner, name.Name)
			b.symbol(kindField, name.Name, ownerID, id, name.Pos(), field.End(), doc)
		}
	}
}

func (b *builder) interfaceMethods(t *ast.InterfaceType, owner, ownerID string) {
	for _, method := range t.Methods.List {
		for _, name := range method.Names {
			id := symbolID(kindMethod, b.u.module, owner, name.Name)
			b.symbol(kindMethod, name.Name, ownerID, id, name.Pos(), method.End(), docText(method.Doc))
		}
	}
}
