package provider

import (
	"encoding/json"
	"fmt"
	"go/ast"
	"go/token"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"unicode/utf8"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"golang.org/x/tools/go/packages"
)

var (
	astNodeType  = reflect.TypeOf((*ast.Node)(nil)).Elem()
	tokenType    = reflect.TypeOf(token.Token(0))
	tokenPosType = reflect.TypeOf(token.Pos(0))
)

// collectSources serializes the same parsed Go AST used by native analysis.
func (r *run) collectSources(pkgs []*packages.Package) {
	seen := map[string]bool{}
	for _, pkg := range pkgs {
		if pkg.Fset == nil {
			continue
		}
		for _, file := range pkg.Syntax {
			rel, ok := r.rel(pkg.Fset.Position(file.Package).Filename)
			if !ok || !r.astRequested[rel] || seen[rel] {
				continue
			}
			seen[rel] = true
			host, ok := r.overlays[filepath.Join(r.root, filepath.FromSlash(rel))]
			if !ok {
				var err error
				host, err = os.ReadFile(filepath.Join(r.root, filepath.FromSlash(rel)))
				if err != nil {
					r.result.Incomplete = append(r.result.Incomplete, sdk.Incomplete{
						Path: rel, Reason: fmt.Sprintf("cannot read source-AST host text: %v", err),
					})
					continue
				}
			}
			source, err := sourceAST(rel, file, pkg.Fset, string(host))
			if err != nil {
				r.result.Incomplete = append(r.result.Incomplete, sdk.Incomplete{
					Path: rel, Reason: fmt.Sprintf("cannot expose Go source AST: %v", err),
				})
				continue
			}
			r.result.Sources = append(r.result.Sources, source)
		}
	}
}

// sourceAST is the serialized syntax tree of one file, with the byte range of
// each node in the host text.
func sourceAST(rel string, file *ast.File, fset *token.FileSet, host string) (sdk.Source, error) {
	if !utf8.ValidString(host) {
		return sdk.Source{}, fmt.Errorf("host text is not valid UTF-8")
	}
	sourceMap := map[string]sdk.Mapping{}
	value, err := astValue(reflect.ValueOf(file), "", fset, host, sourceMap)
	if err != nil {
		return sdk.Source{}, err
	}
	encoded, err := json.Marshal(value)
	if err != nil {
		return sdk.Source{}, fmt.Errorf("encode Go AST: %w", err)
	}
	return sdk.Source{Path: rel, AST: encoded, Mapping: sourceMap}, nil
}

// astValue serializes a parser node by its real AST fields and maps each node's
// logical JSON Pointer to its original byte range. Parser back-references and
// position fields are deliberately kept out of the AST payload.
func astValue(value reflect.Value, pointer string, fset *token.FileSet, host string, sourceMap map[string]sdk.Mapping) (any, error) {
	if !value.IsValid() {
		return nil, nil
	}
	if value.Kind() == reflect.Interface {
		if value.IsNil() {
			return nil, nil
		}
		return astValue(value.Elem(), pointer, fset, host, sourceMap)
	}
	if value.Kind() == reflect.Pointer {
		if value.IsNil() {
			return nil, nil
		}
		if value.Type().Implements(astNodeType) {
			node := value.Interface().(ast.Node)
			serialized, err := astNodeValue(value.Elem(), pointer, fset, host, sourceMap)
			if err != nil {
				return nil, err
			}
			start, end, ok := astRange(node, fset, host)
			if ok {
				rangeValue := [2]uint32{uint32(start), uint32(end)}
				textPointer := pointer + "/source"
				sourceMap[pointer] = sdk.Mapping{
					Range: &rangeValue, TextPointer: textPointer,
				}
				text := host[start:end]
				sourceMap[textPointer] = sdk.Mapping{
					Range: &rangeValue,
					TextMap: []sdk.TextMapSegment{{
						TextRange:   [2]uint32{0, uint32(len(text))},
						SourceRange: rangeValue,
					}},
				}
				serialized["source"] = text
			}
			return serialized, nil
		}
		return nil, nil
	}
	if value.Type() == tokenPosType {
		return nil, nil
	}
	if value.Type() == tokenType {
		return token.Token(value.Int()).String(), nil
	}
	switch value.Kind() {
	case reflect.Bool:
		return value.Bool(), nil
	case reflect.String:
		return value.String(), nil
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		return value.Int(), nil
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return value.Uint(), nil
	case reflect.Slice, reflect.Array:
		if value.Kind() == reflect.Slice && value.IsNil() {
			return []any{}, nil
		}
		items := make([]any, value.Len())
		for i := range value.Len() {
			child := pointer + "/" + fmt.Sprint(i)
			item, err := astValue(value.Index(i), child, fset, host, sourceMap)
			if err != nil {
				return nil, err
			}
			items[i] = item
		}
		return items, nil
	case reflect.Struct:
		fields, err := astStructValue(value, pointer, fset, host, sourceMap)
		if err != nil {
			return nil, err
		}
		return fields, nil
	default:
		return nil, nil
	}
}

func astNodeValue(value reflect.Value, pointer string, fset *token.FileSet, host string, sourceMap map[string]sdk.Mapping) (map[string]any, error) {
	kind := value.Type().Name()
	object := map[string]any{"kind": kind}
	fields, err := astStructValue(value, pointer, fset, host, sourceMap)
	if err != nil {
		return nil, err
	}
	for key, field := range fields {
		object[key] = field
	}
	return object, nil
}

func astStructValue(value reflect.Value, pointer string, fset *token.FileSet, host string, sourceMap map[string]sdk.Mapping) (map[string]any, error) {
	object := make(map[string]any, value.NumField())
	typeOf := value.Type()
	for i := 0; i < value.NumField(); i++ {
		fieldType := typeOf.Field(i)
		if fieldType.PkgPath != "" || omitASTField(fieldType.Name, fieldType.Type) {
			continue
		}
		fieldPointer := pointer + "/" + escapePointer(fieldType.Name)
		field, err := astValue(value.Field(i), fieldPointer, fset, host, sourceMap)
		if err != nil {
			return nil, fmt.Errorf("%s.%s: %w", typeOf.Name(), fieldType.Name, err)
		}
		object[fieldType.Name] = field
	}
	return object, nil
}

func omitASTField(name string, fieldType reflect.Type) bool {
	if fieldType == tokenPosType || strings.HasSuffix(name, "Pos") || strings.HasSuffix(name, "End") {
		return true
	}
	switch name {
	case "Obj", "Scope", "Unresolved", "Imports", "File":
		return true
	}
	return false
}

func astRange(node ast.Node, fset *token.FileSet, host string) (int, int, bool) {
	if _, ok := node.(*ast.File); ok {
		return 0, len(host), true
	}
	if !node.Pos().IsValid() || !node.End().IsValid() {
		return 0, 0, false
	}
	start := fset.PositionFor(node.Pos(), false).Offset
	end := fset.PositionFor(node.End(), false).Offset
	if start < 0 || end < start || end > len(host) ||
		!hostIsBoundary(host, start) || !hostIsBoundary(host, end) {
		return 0, 0, false
	}
	return start, end, true
}

func hostIsBoundary(host string, offset int) bool {
	return offset >= 0 && offset <= len(host) && (offset == len(host) || utf8.RuneStart(host[offset]))
}

func escapePointer(value string) string {
	return strings.ReplaceAll(strings.ReplaceAll(value, "~", "~0"), "/", "~1")
}
