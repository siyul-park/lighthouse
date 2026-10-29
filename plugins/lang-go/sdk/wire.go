// Package sdk speaks the Lighthouse plugin protocol: LSP-framed JSON-RPC 2.0
// and the wire types of docs/plugin-protocol.md. It knows nothing about
// languages or Lighthouse rules.
package sdk

import (
	"encoding/json"
	"fmt"
	"net/url"
	"path/filepath"
)

// Implementation names a peer: the host as clientInfo, the plugin as serverInfo.
type Implementation struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

// WorkspaceFolder is a root of the workspace, as in LSP.
type WorkspaceFolder struct {
	// URI is the `file://` URI of the directory, percent-encoded.
	URI  string `json:"uri"`
	Name string `json:"name"`
}

// InitializeParams is the request of the `initialize` method.
type InitializeParams struct {
	ProtocolVersion string         `json:"protocolVersion"`
	ClientInfo      Implementation `json:"clientInfo"`
	// WorkspaceFolders holds exactly one folder for now: the project root.
	WorkspaceFolders []WorkspaceFolder `json:"workspaceFolders"`
}

// InitializeResult is the response of the `initialize` method.
type InitializeResult struct {
	ProtocolVersion string `json:"protocolVersion"`
	// ServerInfo.Name is the plugin id.
	ServerInfo   Implementation     `json:"serverInfo"`
	Capabilities ServerCapabilities `json:"capabilities"`
}

// ServerCapabilities is what the plugin offers.
type ServerCapabilities struct {
	Languages []ProviderManifest `json:"languages"`
}

// ProviderManifest is what a language provider declares: the language a
// plugin serves, how files select it and what it guarantees.
type ProviderManifest struct {
	ID           string      `json:"id"`
	Globs        []string    `json:"globs"`
	Priority     int         `json:"priority"`
	Fallback     bool        `json:"fallback,omitempty"`
	Conventions  Conventions `json:"conventions"`
	Capabilities []string    `json:"capabilities"`
}

// Conventions are the file conventions a language declares for the whole host.
type Conventions struct {
	TestGlobs []string `json:"test_globs"`
	// ConstructorPrefixes are the name prefixes that make a function a constructor.
	ConstructorPrefixes []string `json:"constructor_prefixes"`
	// TestNaming is how tests are named after the symbols they own.
	TestNaming TestNaming `json:"test_naming"`
}

// TestNaming ties a top-level test function to the public symbol it owns.
type TestNaming struct {
	// Prefix of an owner test's name; a test without it is not one.
	Prefix string `json:"prefix"`
	// SnakeCase compares names in snake case.
	SnakeCase bool `json:"snake_case"`
	// Variants makes TestGet_Missing a second owner of Get.
	Variants bool `json:"variants"`
	// Ancestors counts the tests of a module that tests an ancestor module.
	Ancestors bool `json:"ancestors"`
}

// ProjectRef locates the project an index request refers to.
type ProjectRef struct {
	Root string `json:"root"`
}

// FileRef names a project-relative file and its content hash.
type FileRef struct {
	Path string `json:"path"`
	Hash string `json:"hash"`
}

// Overlay is the text of a file the host holds unsaved, analyzed instead of the disk.
type Overlay struct {
	Path string `json:"path"`
	Text string `json:"text"`
}

// CacheRef is the directory a provider may keep derived results in.
type CacheRef struct {
	Dir string `json:"dir"`
}

// Context carries the per-language options of lighthouse.toml.
type Context struct {
	Options  map[string]json.RawMessage `json:"options"`
	Overlays []Overlay                  `json:"overlays,omitempty"`
	// Cache is absent when the host runs without a cache.
	Cache *CacheRef `json:"cache,omitempty"`
}

// IndexInput uses one provider interface for a file or selected source regions.
type IndexInput struct {
	Kind      string        `json:"kind"`
	File      *FileRef      `json:"file,omitempty"`
	Regions   *EmbeddedFile `json:"regions,omitempty"`
	SourceAST bool          `json:"source_ast,omitempty"`
}

// IndexParams is the request of the index method.
type IndexParams struct {
	Project  ProjectRef   `json:"project"`
	Language string       `json:"language"`
	Inputs   []IndexInput `json:"inputs"`
	Context  Context      `json:"context"`
}

// IndexResult is the response of the `index` method. Its lists must encode as
// `[]` when empty, never `null`.
type IndexResult struct {
	Fragments  []Fragment   `json:"fragments"`
	Sources    []Source     `json:"sources"`
	Notices    []string     `json:"notices"`
	Incomplete []Incomplete `json:"incomplete"`
	// Encoded are fragments that are already JSON, merged in by path when the
	// result is encoded; Fragments and Encoded never hold the same path.
	Encoded []EncodedFragment `json:"-"`
}

// Source is the provider's structurally faithful AST plus its separate map.
type Source struct {
	Path    string             `json:"path"`
	Ast     json.RawMessage    `json:"ast"`
	Mapping map[string]Mapping `json:"mapping"`
}

// Mapping is keyed by JSON Pointer; it never repeats the pointer.
type Mapping struct {
	Range       *[2]uint32       `json:"range,omitempty"`
	TextPointer *string          `json:"text_pointer,omitempty"`
	TextMap     []TextMapSegment `json:"text_map,omitempty"`
}

// TextMapSegment maps an extracted-source byte range to original host bytes.
type TextMapSegment struct {
	TextRange   [2]uint32 `json:"text_range"`
	SourceRange [2]uint32 `json:"source_range"`
}

// EmbeddedFile sends selected source regions to a analysis language provider.
type EmbeddedFile struct {
	Path      string            `json:"path"`
	Hash      string            `json:"hash"`
	Text      string            `json:"text"`
	Generated bool              `json:"generated"`
	Test      bool              `json:"test"`
	Facts     map[string]string `json:"facts"`
	Sources   []EmbeddedSource  `json:"sources"`
}

// EmbeddedSource carries selected text and its byte-by-byte mapping to the host.
type EmbeddedSource struct {
	ID            string           `json:"id"`
	RuleID        string           `json:"rule_id"`
	NodePointer   string           `json:"node_pointer"`
	SourcePointer string           `json:"source_pointer"`
	Provider      string           `json:"provider"`
	Syntax        *string          `json:"syntax,omitempty"`
	Context       json.RawMessage  `json:"context,omitempty"`
	Text          string           `json:"text"`
	SourceRange   [2]uint32        `json:"source_range"`
	TextMap       []TextMapSegment `json:"text_map,omitempty"`
}

// Incomplete reports why a file, or the whole request when Path is empty, was
// not fully analyzed.
type Incomplete struct {
	Path   string `json:"path,omitempty"`
	Reason string `json:"reason"`
}

// Fragment is the code model of one file.
type Fragment struct {
	File      FileInfo          `json:"file"`
	Modules   []Module          `json:"modules"`
	Symbols   []Symbol          `json:"symbols"`
	Edges     []Edge            `json:"edges"`
	Functions []FunctionSummary `json:"functions"`
	Tests     []TestCase        `json:"tests"`
	Comments  []Comment         `json:"comments"`
}

// FileInfo identifies the file of a Fragment.
type FileInfo struct {
	Path      string            `json:"path"`
	Generated bool              `json:"generated,omitempty"`
	Facts     map[string]string `json:"facts,omitempty"`
}

// Module is a unit of dependency such as a package or directory.
type Module struct {
	Path   string `json:"path"`
	Name   string `json:"name,omitempty"`
	TestOf string `json:"test_of,omitempty"`
}

// Position is a one-based line and column.
type Position struct {
	Line int `json:"line"`
	Col  int `json:"col"`
}

// Span is a source range.
type Span struct {
	Start Position `json:"start"`
	End   Position `json:"end"`
}

// Symbol is a declared name with its kind, owner and location.
type Symbol struct {
	ID         string `json:"id"`
	Kind       string `json:"kind"`
	Visibility string `json:"visibility"`
	Owner      string `json:"owner,omitempty"`
	File       string `json:"file"`
	Span       Span   `json:"span"`
	// Extent is the whole declaration, doc comments included; nil when the
	// provider reports none for the symbol.
	Extent *Span  `json:"extent,omitempty"`
	Doc    string `json:"doc,omitempty"`
	Name   string `json:"name"`
	Role   string `json:"role,omitempty"`
	// Optional marks a field a caller may leave out: a pointer, slice, map,
	// function, channel or interface.
	Optional bool `json:"optional,omitempty"`
	// TypeRef is the type of a field.
	TypeRef *TypeRef `json:"type_ref,omitempty"`
}

// TypeRef is a type as a signature or a field writes it: Text is the type with
// full package paths, Symbol the project type it names (a kind-less symbol
// id), Exported whether that type is exported.
type TypeRef struct {
	Text     string `json:"text"`
	Symbol   string `json:"symbol,omitempty"`
	Exported *bool  `json:"exported,omitempty"`
}

// Signature is the parameter and result types of a function, receiver
// excluded, one entry per occurrence.
type Signature struct {
	Params  []TypeRef `json:"params"`
	Results []TypeRef `json:"results"`
}

// Event is something a function body does that a rule may flag.
type Event struct {
	Kind   string `json:"kind"`
	Span   Span   `json:"span"`
	Detail string `json:"detail,omitempty"`
}

// Node is a module or a symbol: exactly one field is set.
type Node struct {
	Module string `json:"module,omitempty"`
	Symbol string `json:"symbol,omitempty"`
}

// Edge is a relation from a Node to a target symbol or module.
type Edge struct {
	Kind       string `json:"kind"`
	From       Node   `json:"from"`
	To         string `json:"to"`
	Resolution string `json:"resolution"`
	// Site is the span of the identifier that names the target.
	Site *Span `json:"site,omitempty"`
}

// Flow is one normalized control-flow event of a function body.
type Flow struct {
	Kind      string `json:"kind"`
	Nesting   int    `json:"nesting"`
	Arms      int    `json:"arms,omitempty"`
	Operators int    `json:"operators,omitempty"`
	Returning bool   `json:"returning,omitempty"`
}

// FunctionSummary is the measured shape of one function or method.
type FunctionSummary struct {
	Symbol     string     `json:"symbol"`
	MaxNesting int        `json:"max_nesting"`
	Statements int        `json:"statements"`
	TopLevel   int        `json:"top_level"`
	Params     int        `json:"params"`
	Returns    int        `json:"returns"`
	Tokens     int        `json:"tokens"`
	Flow       []Flow     `json:"flow"`
	ForwardsTo string     `json:"forwards_to,omitempty"`
	Signature  *Signature `json:"signature,omitempty"`
	Events     []Event    `json:"events,omitempty"`
	// Implementation marks a method that satisfies an interface; Constructs a
	// function without a receiver that returns a type of its package.
	Implementation   bool `json:"implementation,omitempty"`
	Constructs       bool `json:"constructs,omitempty"`
	ManualAssertions int  `json:"manual_assertions,omitempty"`
}

// TestCase describes how a test entry point is written and what it targets.
type TestCase struct {
	Symbol  string   `json:"symbol"`
	Nesting int      `json:"nesting"`
	Style   string   `json:"style"`
	Targets []string `json:"targets"`
}

// Comment is a comment group of a file: adjacent comments with no blank line
// between them, as the source reads, markers included.
type Comment struct {
	Span       Span   `json:"span"`
	Text       string `json:"text"`
	AttachedTo string `json:"attached_to,omitempty"`
}

// ProtocolVersion is the wire protocol version this package implements.
const ProtocolVersion = "0.4"

// SemanticEdges names the capability of reporting type-resolved edges.
const SemanticEdges = "semantic-edges"

// Extent names the capability of reporting the full range of declarations.
const Extent = "extent"

// Overlays names the capability of analyzing overlay text instead of the disk.
const Overlays = "overlays"

// ReferenceSites names the capability of reporting where references occur.
const ReferenceSites = "reference-sites"

// Path is the directory the folder names; it fails for a URI that is not a
// file URI of this machine.
func (f WorkspaceFolder) Path() (string, error) {
	u, err := url.Parse(f.URI)
	if err != nil {
		return "", err
	}
	if u.Scheme != "file" || (u.Host != "" && u.Host != "localhost") {
		return "", fmt.Errorf("%q is not a file URI", f.URI)
	}
	return filepath.Clean(filepath.FromSlash(u.Path)), nil
}

// FileInputs returns physical-file inputs, preserving the request order.
func (p IndexParams) FileInputs() []FileRef {
	var files []FileRef
	for _, input := range p.Inputs {
		if input.Kind == "file" && input.File != nil {
			files = append(files, *input.File)
		}
	}
	return files
}
