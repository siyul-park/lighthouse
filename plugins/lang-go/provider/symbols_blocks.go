package provider

import (
	"encoding/json"
	"go/ast"
	"go/token"
	"go/types"
	"slices"
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

// blocksFact is the name of the file fact that lists the top-level `const` and
// `var` declarations of a file with the types that relate them.
const blocksFact = "blocks"

// block records a top-level `const` or `var` declaration for the file fact.
func (u *unit) block(d *ast.GenDecl) {
	if d.Tok != token.CONST && d.Tok != token.VAR {
		return
	}
	block := declBlock{Keyword: d.Tok.String(), Line: position(u.fset, d.Pos()).Line}
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
