package sdk_test

import (
	"os/exec"
	"testing"

	"github.com/stretchr/testify/require"
)

func TestWireIsGeneratedFromTheSchema(t *testing.T) {
	out, err := exec.Command("go", "run", "./gen", "-check").CombinedOutput()

	require.NoError(t, err, string(out))
}
