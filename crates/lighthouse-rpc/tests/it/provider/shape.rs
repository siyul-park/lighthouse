//! Test cases, summaries, control flow, dispatchers and forwarders as the
//! provider reports them.

use lighthouse_model::ucm::{FlowKind, Target, TestStyle};

use super::fixture::*;

#[test]
fn test_cases_report_run_depth_style_and_targets() {
    let source = r#"package p

import "testing"

func Run(int) {}

func TestTable(t *testing.T) {
	tests := []struct{ in int }{{1}, {2}}
	for _, tc := range tests {
		t.Run("case", func(t *testing.T) { Run(tc.in) })
	}
}

func TestNamedTable(t *testing.T) {
	type tc struct{ in int }
	for _, c := range []tc{{1}} {
		t.Run("case", func(t *testing.T) { Run(c.in) })
	}
}

func TestNested(t *testing.T) {
	t.Run("outer", func(t *testing.T) {
		t.Run("inner", func(t *testing.T) { Run(1) })
	})
}

func TestPlain(t *testing.T) { helper(t) }

func Testing() {}

func helper(t *testing.T) {}
"#;
    let Some(f) = Fixture::new(&[("p_test.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p_test.go");
    let cases: Vec<_> = fragment
        .tests
        .iter()
        .map(|c| (c.symbol.as_str(), c.nesting, c.style))
        .collect();
    assert_eq!(
        cases,
        [
            (".::TestTable#test", 1, TestStyle::Table),
            (".::TestNamedTable#test", 1, TestStyle::Table),
            (".::TestNested#test", 2, TestStyle::Scenario),
            (".::TestPlain#test", 0, TestStyle::Scenario),
        ]
    );
    let table: Vec<_> = fragment.tests[0].targets.iter().map(target).collect();
    assert_eq!(table, [".::Run"]);
    assert_eq!(
        fragment.tests[3].targets,
        [Target::Path(".::helper".to_owned())]
    );
    let kinds: Vec<_> = fragment.symbols.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "function", "test", "test", "test", "test", "function", "function"
        ]
    );
}

#[test]
fn summary_counts_statements_signature_and_tokens() {
    let source = r#"package p

func f(a, b int, xs ...string) (int, error) {
	for _, x := range xs {
		if x == "" {
			return 0, nil
		}
	}
	return a + b, nil
}
"#;
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    let s = summary(&fragment, "::f#");
    assert_eq!((s.max_nesting, s.statements, s.top_level), (2, 4, 2));
    assert_eq!((s.params, s.returns), (3, 2));
    assert!(s.tokens > 20);
    assert!(s.forwards_to.is_none());
}

#[test]
fn flow_events_normalize_go_control_flow() {
    let source = r#"package p

func f(n int, xs []int) int {
	if n > 0 && n < 10 || n == 42 {
		for _, x := range xs {
			if x == 0 {
				continue
			} else if x < 0 {
				break
			} else {
				n++
			}
		}
	}
	switch n {
	case 1:
		n = 2
	}
	g := func() {
		if n > 3 {
			n--
		}
	}
	g()
	if n == 7 {
		goto done
	}
done:
	return f(n-1, xs)
}
"#;
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    let s = summary(&fragment, "::f#");
    assert_eq!(
        s.flow,
        [
            flow(FlowKind::If, 0),
            logic(1),
            logic(1),
            flow(FlowKind::Loop, 1),
            flow(FlowKind::If, 2),
            flow(FlowKind::ElseIf, 2),
            flow(FlowKind::Else, 2),
            switch(1, false),
            flow(FlowKind::If, 1),
            flow(FlowKind::If, 0),
            flow(FlowKind::Jump, 1),
            flow(FlowKind::Recursion, 0),
        ]
    );
    assert_eq!(s.max_nesting, 3);
    assert_eq!(s.statements, 16);
}

#[test]
fn labeled_jumps_and_method_recursion_are_flow_events() {
    let source = "package p\n\ntype T struct{}\n\nfunc (t *T) walk(n int) {\nouter:\n\tfor i := 0; i < n; i++ {\n\t\tfor {\n\t\t\tcontinue outer\n\t\t}\n\t}\n\tt.walk(n - 1)\n}\n";
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    assert_eq!(
        summary(&fragment, "walk").flow,
        [
            flow(FlowKind::Loop, 0),
            flow(FlowKind::Loop, 1),
            flow(FlowKind::Jump, 2),
            flow(FlowKind::Recursion, 0),
        ]
    );
}

#[test]
fn select_and_type_switch_count_arms_as_decisions() {
    let source = r#"package p

func f(v any, a, b chan int) {
	switch v.(type) {
	case int:
	case string:
	default:
	}
	select {
	case <-a:
	case <-b:
	default:
	}
}
"#;
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    assert_eq!(
        summary(&fragment, "::f#").flow,
        [switch(2, false), switch(2, false)]
    );
}

#[test]
fn dispatchers_and_forwarders_are_recognized() {
    let source = r#"package p

type S struct{}

func name(k int) string {
	switch k {
	case 0:
		return "a"
	default:
		return "b"
	}
}

func mixed(k int) string {
	switch k {
	case 0:
		return "a"
	default:
		k++
		return "b"
	}
}

func (s *S) load(k string) error { return s.read(k) }

func (s *S) read(k string) error { return nil }

func wrap(a int, rest ...int) { inner(a, rest...) }

func reorder(a, b int) { inner(b, a) }

func inner(a int, rest ...int) {}
"#;
    let Some(f) = Fixture::new(&[("p.go", source)]) else {
        return;
    };
    let indexed = f.index();
    let fragment = f.fragment(&indexed, "p.go");
    let name = summary(&fragment, "::name#");
    assert_eq!(
        (name.top_level, name.flow.clone()),
        (1, vec![switch(1, true)])
    );
    assert_eq!(summary(&fragment, "::mixed#").flow, [switch(1, false)]);
    assert_eq!(
        summary(&fragment, "S::load").forwards_to,
        Some(Target::Path(".::S::read".to_owned()))
    );
    assert_eq!(
        summary(&fragment, "::wrap#").forwards_to,
        Some(Target::Path(".::inner".to_owned()))
    );
    assert_eq!(summary(&fragment, "::reorder#").forwards_to, None);
    assert_eq!(summary(&fragment, "S::read").forwards_to, None);
}
