package sdk

import (
	"fmt"
	"net/url"
	"path/filepath"
)

//go:generate go run ./gen

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
