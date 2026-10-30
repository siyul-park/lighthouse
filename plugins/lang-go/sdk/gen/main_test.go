package main

import (
	"testing"

	"github.com/stretchr/testify/require"
)

func TestFieldName(t *testing.T) {
	for key, want := range map[string]string{
		"id":              "ID",
		"rule_id":         "RuleID",
		"source_ast":      "SourceAST",
		"protocolVersion": "ProtocolVersion",
		"uri":             "URI",
		"type_ref":        "TypeRef",
	} {
		require.Equal(t, want, fieldName(key), key)
	}
}

func TestWireIsGeneratedFromTheSchema(t *testing.T) {
	err := run("../../../../protocol/schema/lighthouse-protocol-0.4.json", "../wire.go", true)

	require.NoError(t, err)
}
