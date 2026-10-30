"""What one request knows of the project: its parsed files, their declarations and how names reach them."""

import ast
from dataclasses import dataclass, field

from lang_python.symbols import Decl, FileDecls
from lang_python.tests import is_test_file
from lang_python.modules import module_path
from lang_python.extent import Lines
from lang_python.project.files import Project, is_stub
from lang_python.project.scopes import ScopeTree
from lang_python.comments import Tokens


@dataclass(eq=False)
class ParsedFile:
    """A source file read and parsed once."""

    rel: str
    text: str
    lines: Lines
    tree: ast.Module | None
    error: str | None
    module: str
    decls: FileDecls | None = None
    _tokens: Tokens | None = None
    _scopes: ScopeTree | None = None
    cache: dict = field(default_factory=dict)

    @property
    def tokens(self) -> Tokens:
        """The comments and significant tokens of the file, read on first use."""
        if self._tokens is None:
            self._tokens = Tokens(self.text, self.lines)
        return self._tokens

    def scopes(self) -> ScopeTree:
        """The `symtable` scopes of the file, read on first use."""
        if self._scopes is None:
            self._scopes = ScopeTree(self.text, self.rel)
        return self._scopes


def _syntax_problem(error: BaseException) -> str:
    """The first problem of a file that does not parse, as `line:col: message`."""
    if isinstance(error, SyntaxError):
        return "%d:%d: %s" % (error.lineno or 1, error.offset or 1, error.msg)
    return "1:1: %s" % error


class Model:
    """The parsed files of a project and what they declare."""

    def __init__(self, project: Project):
        self.project = project
        self._files: dict[str, ParsedFile | None] = {}
        self._names: dict[str, list[Decl]] | None = None
        self.resolving: set[str] = set()
        """The variables whose declared type is being worked out: a value that names itself settles nothing."""

    def file(self, rel: str) -> ParsedFile | None:
        """The parsed file at a project-relative path; None when it cannot be read."""
        if rel not in self._files:
            self._files[rel] = self._parse(rel)
        return self._files[rel]

    def _parse(self, rel: str) -> ParsedFile | None:
        text = self.project.read(rel)
        if text is None:
            return None
        module = module_path(rel)
        lines = Lines(text)
        try:
            tree = ast.parse(text, filename=rel)
        except (SyntaxError, ValueError, RecursionError, MemoryError) as error:
            return ParsedFile(rel, text, lines, None, _syntax_problem(error), module)
        parsed = ParsedFile(rel, text, lines, tree, None, module)
        parsed.decls = FileDecls(rel, module, tree, text, is_test_file(rel) and not is_stub(rel))
        return parsed

    def decls(self, rel: str) -> FileDecls | None:
        """The declarations of the file at a path; None when it cannot be read or does not parse."""
        parsed = self.file(rel)
        return None if parsed is None else parsed.decls

    def named(self, name: str) -> list[Decl]:
        """The declarations of the project that bear a name; a use of a name none bears names nothing of the project."""
        if self._names is None:
            self._names = {}
            for rel in sorted(self.project.files):
                if is_stub(rel) and rel[:-1] in self.project.files:
                    continue
                decls = self.decls(rel)
                for decl in decls.list if decls is not None else []:
                    self._names.setdefault(decl.name, []).append(decl)
        return self._names.get(name, [])
