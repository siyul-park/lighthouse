package provider

import (
	"go/ast"
	"go/types"
	"sync"

	"golang.org/x/tools/go/packages"
)

// optionConstructors are the constructors of a package that take an option
// type, by the option type: a function without a receiver that has R or ...R
// as a parameter and returns U, *U or (U, error) for a type U of the package.
type optionConstructors struct {
	once sync.Once
	by   map[string][]constructor
}

// constructor is a function that builds the type built, declared in file.
type constructor struct {
	built *types.TypeName
	file  string
}

// builtBy is the type that the constructors taking the option type build: the
// only one, or among several the one built by a constructor in the file of the
// builder. It is nil when there is none or the choice stays open.
func (o *optionConstructors) builtBy(option types.Type, file string) *types.TypeName {
	all := o.by[types.TypeString(types.Unalias(option), nil)]
	if built := distinct(all, func(constructor) bool { return true }); built != nil {
		return built
	}
	return distinct(all, func(c constructor) bool { return c.file == file })
}

// constructorsOf returns the option-taking constructors of a package,
// computed once.
func (r *resolver) constructorsOf(pkg *packages.Package) *optionConstructors {
	cached, _ := r.constructors.LoadOrStore(pkg, &optionConstructors{})
	set := cached.(*optionConstructors)
	set.once.Do(func() { set.by = collectConstructors(pkg) })
	return set
}

func collectConstructors(pkg *packages.Package) map[string][]constructor {
	by := map[string][]constructor{}
	for _, file := range pkg.Syntax {
		for _, decl := range file.Decls {
			d, ok := decl.(*ast.FuncDecl)
			if !ok || d.Recv != nil {
				continue
			}
			fn, ok := pkg.TypesInfo.Defs[d.Name].(*types.Func)
			if !ok {
				continue
			}
			sig := fn.Type().(*types.Signature)
			built, ok := builtType(sig.Results(), pkg.Types)
			if !ok {
				continue
			}
			c := constructor{built: built, file: pkg.Fset.Position(d.Pos()).Filename}
			for i := range sig.Params().Len() {
				param := sig.Params().At(i).Type()
				if slice, ok := param.(*types.Slice); ok && sig.Variadic() && i == sig.Params().Len()-1 {
					param = slice.Elem()
				}
				key := types.TypeString(types.Unalias(param), nil)
				by[key] = append(by[key], c)
			}
		}
	}
	return by
}

// builtType is the type of pkg that results are: U, *U or (U, error).
func builtType(results *types.Tuple, pkg *types.Package) (*types.TypeName, bool) {
	if results.Len() == 2 && !isError(results.At(1).Type()) || results.Len() < 1 || results.Len() > 2 {
		return nil, false
	}
	named, ok := receiverNamed(results.At(0).Type())
	if !ok || named.Obj().Pkg() != pkg {
		return nil, false
	}
	return named.Origin().Obj(), true
}

// distinct is the one type that the constructors accepted by keep build, nil
// if they build none or several.
func distinct(all []constructor, keep func(constructor) bool) *types.TypeName {
	var found *types.TypeName
	for _, c := range all {
		if !keep(c) || c.built == found {
			continue
		}
		if found != nil {
			return nil
		}
		found = c.built
	}
	return found
}
