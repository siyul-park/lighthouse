package provider

import (
	"fmt"
	"go/types"
	"slices"
	"strconv"
	"strings"
)

// providedKeys are the keys of the methods a value of the type can be called
// with when it is addressable, promoted ones included, sorted and unique.
func providedKeys(named *types.Named) []string {
	named, ok := instantiated(named)
	if !ok {
		return nil
	}
	set := types.NewMethodSet(types.NewPointer(named))
	keys := make([]string, 0, set.Len())
	for i := range set.Len() {
		if fn, ok := set.At(i).Obj().(*types.Func); ok {
			keys = append(keys, methodKey(fn))
		}
	}
	return sortedKeys(keys)
}

// requiredKeys are the keys of the methods of an interface, embedded ones
// included, sorted and unique.
func requiredKeys(iface *types.Interface) []string {
	keys := make([]string, 0, iface.NumMethods())
	for i := range iface.NumMethods() {
		keys = append(keys, methodKey(iface.Method(i)))
	}
	return sortedKeys(keys)
}

func sortedKeys(keys []string) []string {
	slices.Sort(keys)
	return slices.Compact(keys)
}

// methodKey names a method so that two keys are equal exactly when the methods
// have the same name, the same package when the name is unexported, and
// identical signatures. types.TypeString does not do: an alias prints by its
// own name, and a type of another package by its package's name, which two
// packages may share. Types are written in a canonical form: aliases resolved
// at every level, named types qualified by their full package path and
// followed by their type arguments, and nothing that identical types do not
// share (parameter names).
func methodKey(fn *types.Func) string {
	return fn.Name() + qualifier(fn.Exported(), fn.Pkg()) + ":" + signatureKey(fn.Type().(*types.Signature))
}

// qualifier is the quoted path of the package that declares an unexported name.
func qualifier(exported bool, pkg *types.Package) string {
	if exported || pkg == nil {
		return ""
	}
	return strconv.Quote(pkg.Path())
}

// identityKey is the canonical form of a type.
func identityKey(t types.Type) string {
	switch t := types.Unalias(t).(type) {
	case *types.Basic:
		return types.Typ[t.Kind()].Name()
	case *types.Named:
		return namedKey(t)
	case *types.Pointer:
		return "*" + identityKey(t.Elem())
	case *types.Slice:
		return "[]" + identityKey(t.Elem())
	case *types.Array:
		return "[" + strconv.FormatInt(t.Len(), 10) + "]" + identityKey(t.Elem())
	case *types.Map:
		return "map[" + identityKey(t.Key()) + "]" + identityKey(t.Elem())
	case *types.Chan:
		return []string{types.SendRecv: "chan(", types.SendOnly: "chan<-(", types.RecvOnly: "<-chan("}[t.Dir()] + identityKey(t.Elem()) + ")"
	case *types.Signature:
		return signatureKey(t)
	case *types.Struct:
		return structKey(t)
	case *types.Interface:
		return "interface{" + joined(t.NumMethods(), ";", func(i int) string { return methodKey(t.Method(i)) }) + "}"
	case *types.TypeParam:
		return "$" + t.Obj().Name()
	}
	return t.String()
}

func namedKey(t *types.Named) string {
	obj := t.Obj()
	key := obj.Name()
	if obj.Pkg() != nil {
		key = strconv.Quote(obj.Pkg().Path()) + "." + key
	}
	if args := t.TypeArgs(); args.Len() > 0 {
		key += "[" + joined(args.Len(), ",", func(i int) string { return identityKey(args.At(i)) }) + "]"
	}
	return key
}

func signatureKey(sig *types.Signature) string {
	params, results := sig.Params(), sig.Results()
	in := joined(params.Len(), ",", func(i int) string {
		t := params.At(i).Type()
		if slice, ok := t.(*types.Slice); ok && sig.Variadic() && i == params.Len()-1 {
			return "..." + identityKey(slice.Elem())
		}
		return identityKey(t)
	})
	out := joined(results.Len(), ",", func(i int) string { return identityKey(results.At(i).Type()) })
	return "func(" + in + ")(" + out + ")"
}

func structKey(t *types.Struct) string {
	return "struct{" + joined(t.NumFields(), ";", func(i int) string {
		field := t.Field(i)
		return fmt.Sprintf("%s%s %t %s %q", field.Name(), qualifier(field.Exported(), field.Pkg()), field.Embedded(), identityKey(field.Type()), t.Tag(i))
	}) + "}"
}

// joined is the n strings at(0)..at(n-1) with sep between them.
func joined(n int, sep string, at func(int) string) string {
	parts := make([]string, n)
	for i := range parts {
		parts[i] = at(i)
	}
	return strings.Join(parts, sep)
}
