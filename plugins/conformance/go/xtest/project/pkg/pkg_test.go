package pkg_test

import (
	"testing"

	"example.com/xtest/pkg"
)

func TestRun(t *testing.T) {
	if pkg.Run() != 1 {
		t.Fatal("run")
	}
}
