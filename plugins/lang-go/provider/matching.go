package provider

import (
	"go/ast"
	"go/token"
	"slices"
)

// add merges what another file shows.
func (m *matching) add(other matching) {
	if m.methods == nil {
		m.methods = map[string]bool{}
	}
	for name := range other.methods {
		m.methods[name] = true
	}
	m.open = m.open || other.open
	m.unknown = m.unknown || other.unknown
	m.interfaces = append(m.interfaces, other.interfaces...)
}

// matchingOf reads what a file shows of the types and interfaces it declares.
func matchingOf(file *ast.File) matching {
	m := matching{methods: map[string]bool{}}
	for _, decl := range file.Decls {
		switch decl := decl.(type) {
		case *ast.FuncDecl:
			if decl.Recv != nil {
				m.methods[decl.Name.Name] = true
			}
		case *ast.GenDecl:
			if decl.Tok == token.TYPE {
				m.types(decl)
			}
		}
	}
	return m
}

// types reads the type specs of one declaration.
func (m *matching) types(decl *ast.GenDecl) {
	for _, spec := range decl.Specs {
		ts, ok := spec.(*ast.TypeSpec)
		if !ok {
			continue
		}
		if st, ok := ts.Type.(*ast.StructType); ok && embeds(st) {
			m.open = true
		}
		switch t := ts.Type.(type) {
		case *ast.InterfaceType:
			m.interfaces = append(m.interfaces, explicit(t))
		default:
			if mayBeInterface(t) {
				m.unknown = true
			}
		}
	}
}

// embeds reports whether a struct has an embedded field: its methods are the
// type's too.
func embeds(st *ast.StructType) bool {
	return slices.ContainsFunc(st.Fields.List, func(field *ast.Field) bool { return len(field.Names) == 0 })
}

// explicit are the names of the methods an interface lists itself; the ones it
// takes from embedded interfaces come on top.
func explicit(t *ast.InterfaceType) []string {
	var names []string
	for _, field := range t.Methods.List {
		for _, name := range field.Names {
			names = append(names, name.Name)
		}
	}
	return names
}

// mayMatch reports whether a type of a directory that declares these methods
// can implement an interface of other: it does not when no interface of other
// has all its listed methods among them.
func (m matching) mayMatch(other matching) bool {
	if m.open || other.unknown {
		return true
	}
	for _, names := range other.interfaces {
		if !slices.ContainsFunc(names, func(name string) bool { return !m.methods[name] }) {
			return true
		}
	}
	return false
}
