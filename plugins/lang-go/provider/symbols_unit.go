package provider

import (
	"encoding/json"
	"go/ast"
	"go/token"
	"go/types"
	"slices"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// declBlock is one top-level `const` or `var` declaration: a block when it is
// parenthesized, else the one spec it holds.
type declBlock struct {
	Keyword string     `json:"keyword"`
	Line    int        `json:"line"`
	Specs   []declSpec `json:"specs"`
}

// declSpec is one spec of a block. Keys are the named types that tie it to its
// siblings: the type it declares or its value has, and, for an interface
// assertion (`var _ I = T`), both the interface and the concrete type.
type declSpec struct {
	Names  []string `json:"names"`
	Line   int      `json:"line"`
	Public bool     `json:"public"`
	Keys   []string `json:"keys"`
}

// inlineBody is a function whose body is written between braces of one line:
// where the braces are and the statements the body holds, as written.
type inlineBody struct {
	// Line is the line of the declaration, where the function symbol is.
	Line int `json:"line"`
	// Open and Close are the positions just after `{` and of `}`: the text
	// between them is the body.
	Open       sdk.Position `json:"open"`
	Close      sdk.Position `json:"close"`
	Statements []string     `json:"statements"`
	// Plain is false when a comment sits in the body, which an expansion
	// would have to carry along.
	Plain bool `json:"plain"`
}

// blocksFact is the name of the file fact that lists the top-level `const` and
// `var` declarations of a file with the types that relate them.
const blocksFact = "blocks"

// inlineBodiesFact is the name of the file fact that lists the function
// declarations whose non-empty body sits on the line of its braces.
const inlineBodiesFact = "inline_bodies"

// block records a top-level `const` or `var` declaration for the file fact.
func (u *unit) block(d *ast.GenDecl) {
	if d.Tok != token.CONST && d.Tok != token.VAR {
		return
	}
	block := declBlock{Keyword: d.Tok.String(), Line: position(u.fset, d.Pos()).Line, Specs: []declSpec{}}
	for _, spec := range d.Specs {
		if s, ok := spec.(*ast.ValueSpec); ok {
			block.Specs = append(block.Specs, u.declSpec(s))
		}
	}
	u.blocks = append(u.blocks, block)
}

func (u *unit) declSpec(s *ast.ValueSpec) declSpec {
	spec := declSpec{Line: position(u.fset, s.Pos()).Line, Keys: []string{}}
	for _, name := range s.Names {
		spec.Names = append(spec.Names, name.Name)
	}
	spec.Public = ast.IsExported(spec.Names[0])
	exprs := s.Values
	if s.Type != nil {
		exprs = append([]ast.Expr{s.Type}, exprs...)
	}
	for _, expr := range exprs {
		key := typeKey(u.info().TypeOf(expr))
		if key != "" && !slices.Contains(spec.Keys, key) {
			spec.Keys = append(spec.Keys, key)
		}
	}
	return spec
}

// publishBlocks sets the file fact once every declaration is recorded.
func (u *unit) publishBlocks() {
	if len(u.blocks) == 0 {
		return
	}
	data, err := json.Marshal(u.blocks)
	if err != nil {
		return
	}
	if u.frag.File.Facts == nil {
		u.frag.File.Facts = map[string]string{}
	}
	u.frag.File.Facts[blocksFact] = string(data)
}

// inline records the function declaration when its body is on one line.
func (u *unit) inline(d *ast.FuncDecl) {
	body := d.Body
	if body == nil || len(body.List) == 0 {
		return
	}
	open, close := u.fset.Position(body.Lbrace), u.fset.Position(body.Rbrace)
	if open.Line != close.Line {
		return
	}
	found := inlineBody{
		Line:  u.fset.Position(d.Pos()).Line,
		Open:  position(u.fset, body.Lbrace+1),
		Close: position(u.fset, body.Rbrace),
		Plain: true,
	}
	for _, stmt := range body.List {
		from, to := u.fset.Position(stmt.Pos()).Offset, u.fset.Position(stmt.End()).Offset
		if from < 0 || to > len(u.src) || from >= to {
			return
		}
		found.Statements = append(found.Statements, string(u.src[from:to]))
	}
	for _, group := range u.file.Comments {
		if group.Pos() > body.Lbrace && group.End() < body.Rbrace {
			found.Plain = false
		}
	}
	u.inlined = append(u.inlined, found)
}

// publishInline sets the file fact once every function is recorded.
func (u *unit) publishInline() {
	if len(u.inlined) == 0 {
		return
	}
	data, err := json.Marshal(u.inlined)
	if err != nil {
		return
	}
	if u.frag.File.Facts == nil {
		u.frag.File.Facts = map[string]string{}
	}
	u.frag.File.Facts[inlineBodiesFact] = string(data)
}

// signature is the parameter and result types of a function, receiver
// excluded, one entry per parameter or result; nil when it has none.
func (u *unit) signature(d *ast.FuncDecl) *sdk.Signature {
	fn, ok := u.info().Defs[d.Name].(*types.Func)
	if !ok {
		return nil
	}
	sig, ok := fn.Type().(*types.Signature)
	if !ok || sig.Params().Len()+sig.Results().Len() == 0 {
		return nil
	}
	return &sdk.Signature{Params: u.typeRefs(sig.Params()), Results: u.typeRefs(sig.Results())}
}

// typeRefs describes every variable of a parameter or result list. The slice
// is never nil, so that it encodes as `[]`.
func (u *unit) typeRefs(vars *types.Tuple) []sdk.TypeRef {
	refs := make([]sdk.TypeRef, 0, vars.Len())
	for v := range vars.Variables() {
		refs = append(refs, u.typeRef(v.Type()))
	}
	return refs
}

// fieldType is the type of a struct field as written.
func (u *unit) fieldType(field *ast.Field) *sdk.TypeRef {
	t := u.info().TypeOf(field.Type)
	if t == nil {
		return nil
	}
	ref := u.typeRef(t)
	return &ref
}

// typeRef writes a type with the full path of every package as qualifier and
// names the project type behind pointers and aliases, if there is one.
func (u *unit) typeRef(t types.Type) sdk.TypeRef {
	ref := sdk.TypeRef{Text: types.TypeString(t, fullPath)}
	if owner, ok := namedType(t); ok && u.res.inProject(owner.Pkg()) {
		exported := writtenName(t, owner).Exported()
		ref.Symbol = target(u.res.module(owner.Pkg()), owner.Name())
		ref.Exported = &exported
	}
	return ref
}

// typeKey is the package-qualified name of the named type behind pointers and
// type arguments (`*pkg.Array`, `pkg.TypedArray[bool]` are `pkg.Array` and
// `pkg.TypedArray`), or "" for a basic, unnamed or unknown type.
func typeKey(t types.Type) string {
	for t != nil {
		switch v := types.Unalias(t).(type) {
		case *types.Pointer:
			t = v.Elem()
		case *types.Named:
			obj := v.Origin().Obj()
			if obj.Pkg() == nil {
				return obj.Name()
			}
			return obj.Pkg().Path() + "." + obj.Name()
		default:
			return ""
		}
	}
	return ""
}

// writtenName is the name a caller sees for the type: the alias when the
// signature spells one (`type Option = option` exports an unexported type),
// else the declared type itself.
func writtenName(t types.Type, declared *types.TypeName) *types.TypeName {
	if ptr, ok := t.(*types.Pointer); ok {
		t = ptr.Elem()
	}
	if alias, ok := t.(*types.Alias); ok {
		return alias.Obj()
	}
	return declared
}

// fullPath qualifies a type by the import path of its package.
func fullPath(p *types.Package) string {
	return p.Path()
}
