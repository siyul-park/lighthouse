package provider

import (
	"go/ast"
	"go/types"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

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
func fullPath(p *types.Package) string { return p.Path() }
