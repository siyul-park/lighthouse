//! The query a decision's `where` is: non-recursive Datalog with built-in
//! comparisons, `violation(S) :- ...` for each conjunction.

use std::fmt;

/// A value written in the expression, or an option list.
#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    List(Vec<Const>),
}

/// What a path starts from: the selected subject, the file it is in, or a
/// member of a child relation an atom binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Root {
    Subject,
    File,
    Binder(usize),
}

/// A field of a root, plus an integer offset (`x + 1`).
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub root: Root,
    pub fields: Vec<String>,
    pub offset: i64,
}

/// A side of a comparison. An option is a constant once the project's options
/// are known; it is kept by name so one query serves every configuration.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Path(Path),
    Option(Vec<String>),
    Const(Const),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    NotIn,
    Starts,
    NotStarts,
    Ends,
    NotEnds,
    Contains,
    NotContains,
}

impl Op {
    /// The operator true exactly when this one is false.
    pub fn negated(self) -> Self {
        match self {
            Self::Eq => Self::Ne,
            Self::Ne => Self::Eq,
            Self::Lt => Self::Ge,
            Self::Ge => Self::Lt,
            Self::Le => Self::Gt,
            Self::Gt => Self::Le,
            Self::In => Self::NotIn,
            Self::NotIn => Self::In,
            Self::Starts => Self::NotStarts,
            Self::NotStarts => Self::Starts,
            Self::Ends => Self::NotEnds,
            Self::NotEnds => Self::Ends,
            Self::Contains => Self::NotContains,
            Self::NotContains => Self::Contains,
        }
    }

    /// The operator that holds with the sides swapped, when there is one.
    pub fn flipped(self) -> Option<Self> {
        match self {
            Self::Eq | Self::Ne => Some(self),
            Self::Lt => Some(Self::Gt),
            Self::Gt => Some(Self::Lt),
            Self::Le => Some(Self::Ge),
            Self::Ge => Some(Self::Le),
            _ => None,
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::In => "in",
            Self::NotIn => "not in",
            Self::Starts => "starts",
            Self::NotStarts => "not starts",
            Self::Ends => "ends",
            Self::NotEnds => "not ends",
            Self::Contains => "contains",
            Self::NotContains => "not contains",
        }
    }
}

/// `left op right`; at most one side is a path.
#[derive(Debug, Clone, PartialEq)]
pub struct Cmp {
    pub left: Operand,
    pub op: Op,
    pub right: Operand,
}

/// `binder` is a member of the child relation `list`.
#[derive(Debug, Clone, PartialEq)]
pub struct Atom {
    pub binder: usize,
    pub list: Path,
}

/// One conjunction: the atoms bind members, and the comparisons and the empty
/// relations hold of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cq {
    pub atoms: Vec<Atom>,
    pub cmps: Vec<Cmp>,
    /// Containers with no element: `size(path) == 0`.
    pub empties: Vec<Path>,
    /// Containers with some element: `size(path) > 0`.
    pub filled: Vec<Path>,
}

/// The violations of a decision: a subject violates it when it satisfies any
/// conjunction.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ucq {
    pub cqs: Vec<Cq>,
    pub binders: usize,
}

impl fmt::Display for Const {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("null"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Int(i) => write!(f, "{i}"),
            Self::Str(s) => write!(f, "{s:?}"),
            Self::List(items) => {
                f.write_str("[")?;
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str("]")
            }
        }
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.root {
            Root::Subject => f.write_str("S")?,
            Root::File => f.write_str("File")?,
            Root::Binder(id) => write!(f, "B{id}")?,
        }
        for field in &self.fields {
            write!(f, ".{field}")?;
        }
        match self.offset {
            0 => Ok(()),
            n => write!(f, "{n:+}"),
        }
    }
}

impl fmt::Display for Operand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => write!(f, "{path}"),
            Self::Option(fields) => write!(f, "options.{}", fields.join(".")),
            Self::Const(value) => write!(f, "{value}"),
        }
    }
}

impl fmt::Display for Ucq {
    /// The Datalog text, one rule for each conjunction.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for cq in &self.cqs {
            f.write_str("violation(S) :- subject(S)")?;
            for atom in &cq.atoms {
                write!(f, ", member(B{}, {})", atom.binder, atom.list)?;
            }
            for path in &cq.empties {
                write!(f, ", empty({path})")?;
            }
            for path in &cq.filled {
                write!(f, ", filled({path})")?;
            }
            for cmp in &cq.cmps {
                write!(f, ", {} {} {}", cmp.left, cmp.op.text(), cmp.right)?;
            }
            f.write_str(".\n")?;
        }
        Ok(())
    }
}
