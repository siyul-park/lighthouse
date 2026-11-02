package asm

import "testing"

func TestEnter(t *testing.T) {
	if Enter() == 0 {
		t.Fatal("enter")
	}
}
