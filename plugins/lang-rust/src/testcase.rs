//! Test cases: the attribute that makes a function a test and the style of
//! its body.

use std::collections::HashSet;

use lighthouse_protocol::TestStyle;
use syn::{
    Attribute, Block, Expr, Pat, Stmt,
    visit::{self, Visit},
};

use crate::macros;

#[derive(Default)]
struct Finder {
    tables: HashSet<String>,
    found: bool,
}

impl<'ast> Visit<'ast> for Finder {
    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if let Stmt::Local(local) = stmt
            && let (Pat::Ident(name), Some(init)) = (&local.pat, &local.init)
            && is_table(&init.expr)
        {
            self.tables.insert(name.ident.to_string());
        }
        visit::visit_stmt(self, stmt);
    }

    fn visit_expr_for_loop(&mut self, f: &'ast syn::ExprForLoop) {
        let base = iterated(&f.expr);
        let named = matches!(base, Expr::Path(p) if p.path.get_ident()
            .is_some_and(|i| self.tables.contains(&i.to_string())));
        self.found |= named || is_table(base);
        visit::visit_expr_for_loop(self, f);
    }
}

/// `table` when the test loops over cases written in the test (a literal
/// array or `vec!` of tuples or structs, directly or through a `let`), or when
/// an `rstest`/`test_case` function carries several cases; else `scenario`.
pub fn style(attrs: &[Attribute], body: &Block) -> TestStyle {
    if declared_cases(attrs) >= 2 {
        return TestStyle::Table;
    }
    let mut finder = Finder::default();
    finder.visit_block(body);
    if finder.found {
        TestStyle::Table
    } else {
        TestStyle::Scenario
    }
}

fn declared_cases(attrs: &[Attribute]) -> usize {
    let paths: Vec<String> = attrs.iter().map(crate::util::attr_path).collect();
    let count = |names: &[&str]| paths.iter().filter(|p| names.contains(&p.as_str())).count();
    let rstest = count(&["rstest", "rstest::rstest"]) > 0;
    let rstest_cases = if rstest {
        count(&["case", "rstest::case"])
    } else {
        0
    };
    rstest_cases + count(&["test_case", "test_case::test_case"])
}

/// The collection a loop walks, behind references and iterator adapters.
fn iterated(mut e: &Expr) -> &Expr {
    loop {
        match e {
            Expr::Reference(r) => e = &r.expr,
            Expr::Paren(p) => e = &p.expr,
            Expr::MethodCall(m)
                if matches!(
                    m.method.to_string().as_str(),
                    "iter" | "iter_mut" | "into_iter" | "enumerate" | "copied" | "cloned" | "rev"
                ) =>
            {
                e = &m.receiver;
            }
            _ => return e,
        }
    }
}

/// A literal array, or `vec!`, whose elements are tuples or structs.
fn is_table(e: &Expr) -> bool {
    let first = match e {
        Expr::Reference(r) => return is_table(&r.expr),
        Expr::Array(a) => a.elems.first().cloned(),
        Expr::Macro(m) if macros::is_named(&m.mac, "vec") => {
            macros::arguments(&m.mac).and_then(|a| a.into_iter().next())
        }
        _ => None,
    };
    matches!(first, Some(Expr::Tuple(_) | Expr::Struct(_)))
}
