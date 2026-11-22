"""UTF-8 lexer for Active: both layouts, units, strings, interpolation.

Implements the lexical contract in spec/grammar.md §1 (G06 unit maximal
munch; H001 unclosed string; E005/E006/E007 hard errors).
"""

from __future__ import annotations

from dataclasses import dataclass

from . import tokens as T
from .diag import Diagnostic, ParseFailure, Span, error, info
from .units import UNITS


def _is_ident_start(ch: str) -> bool:
    return ch.isascii() and (ch.isalpha() or ch == "_")


def _is_ident_char(ch: str) -> bool:
    return ch.isascii() and (ch.isalnum() or ch == "_")


def _is_hex(ch: str) -> bool:
    return ch in "0123456789abcdefABCDEF"


@dataclass
class LexResult:
    tokens: list[T.Token]
    comments: list[T.Comment]
    diagnostics: list[Diagnostic]


class Lexer:
    def __init__(self, source: str, name: str = "<source>"):
        self.src = source
        self.name = name
        self.i = 0
        self.line = 1
        self.col = 1
        self.tokens: list[T.Token] = []
        self.comments: list[T.Comment] = []
        self.diagnostics: list[Diagnostic] = []
        self.indents: list[int] = [0]
        self.bracket_depth = 0
        self.at_line_start = True
        self.line_has_code = False

    # -- position helpers -------------------------------------------------
    def _span(self, start_line: int, start_col: int) -> Span:
        return Span(start_line, start_col, self.line, self.col)

    def _peek(self, offset: int = 0) -> str:
        j = self.i + offset
        return self.src[j] if j < len(self.src) else ""

    def _advance(self) -> str:
        ch = self.src[self.i]
        self.i += 1
        if ch == "\n":
            self.line += 1
            self.col = 1
        else:
            self.col += 1
        return ch

    def _emit(
        self,
        kind: str,
        value=None,
        span: Span | None = None,
        unit: str | None = None,
        parts=None,
    ) -> None:
        self.tokens.append(T.Token(kind, value, span, unit, parts))
        if kind not in (T.NEWLINE, T.INDENT, T.DEDENT, T.EOF):
            self.line_has_code = True

    def _fail(self, code: str, message: str, start_line: int, start_col: int) -> None:
        raise ParseFailure(
            [
                error(
                    code,
                    message,
                    span=self._span(start_line, start_col),
                    file=self.name,
                )
            ]
        )

    # -- main -------------------------------------------------------------
    def run(self) -> LexResult:
        if self._peek() == "\ufeff":
            self._advance()
        while self.i < len(self.src):
            if self.at_line_start and self.bracket_depth == 0:
                self._handle_indent()
                if self.i >= len(self.src):
                    break
                if not self.at_line_start:
                    continue
            ch = self._peek()
            if ch == " " or ch == "\t" or ch == "\r":
                self._advance()
                continue
            if ch == "\n":
                self._advance()
                self._emit(T.NEWLINE, None, Span(self.line - 1, 1, self.line - 1, 1))
                self.at_line_start = True
                self.line_has_code = False
                continue
            if ch == "#":
                self._comment()
                continue
            if ch == '"' or ch == "'":
                self._string()
                continue
            if ch.isdigit():
                self._number()
                continue
            if _is_ident_start(ch):
                self._ident()
                continue
            self._operator()
        # Close every open indent level.
        while len(self.indents) > 1:
            self.indents.pop()
            self._emit(T.DEDENT, None, Span(self.line, self.col, self.line, self.col))
        self._emit(T.EOF, None, Span(self.line, self.col, self.line, self.col))
        return LexResult(self.tokens, self.comments, self.diagnostics)

    # -- indentation ------------------------------------------------------
    def _handle_indent(self) -> None:
        start_line, start_col = self.line, self.col
        width = 0
        j = self.i
        while j < len(self.src) and self.src[j] in " \t":
            if self.src[j] == "\t":
                self._fail("E006", "tab in leading whitespace", start_line, start_col)
            width += 1
            j += 1
        # Blank or comment-only lines never change the indent stack.
        if j >= len(self.src) or self.src[j] in "\n#":
            while self.i < j:
                self._advance()
            self.at_line_start = False
            return
        while self.i < j:
            self._advance()
        self.at_line_start = False
        if width > self.indents[-1]:
            self.indents.append(width)
            self._emit(
                T.INDENT, width, Span(start_line, start_col, self.line, self.col)
            )
        elif width < self.indents[-1]:
            while len(self.indents) > 1 and width < self.indents[-1]:
                self.indents.pop()
                self._emit(
                    T.DEDENT, width, Span(start_line, start_col, self.line, self.col)
                )
            if width != self.indents[-1]:
                self._fail("E007", "inconsistent dedentation", start_line, start_col)

    # -- comments ---------------------------------------------------------
    def _comment(self) -> None:
        line = self.line
        trailing = self.line_has_code
        self._advance()  # '#'
        start = self.i
        while self.i < len(self.src) and self._peek() != "\n":
            self._advance()
        self.comments.append(T.Comment(line, self.src[start : self.i], trailing))

    # -- identifiers and reserved tokens ----------------------------------
    def _ident(self) -> None:
        start_line, start_col = self.line, self.col
        start = self.i
        while self.i < len(self.src) and _is_ident_char(self._peek()):
            self._advance()
        text = self.src[start : self.i]
        span = self._span(start_line, start_col)
        if text == "true" or text == "false":
            self._emit(T.BOOL, text == "true", span)
        elif text == "null":
            self._emit(T.NULL, None, span)
        elif text == "sha256" and self._peek() == ":":
            # sha256:<64 hex> is one reserved token (L-P06). A malformed
            # pin is E015, never a silent ordinary key-value.
            self._advance()  # ':'
            hstart = self.i
            while self.i < len(self.src) and _is_hex(self._peek()):
                self._advance()
            pin = self.src[hstart : self.i]
            if len(pin) != 64 or (
                self.i < len(self.src) and _is_ident_char(self._peek())
            ):
                self.diagnostics.append(
                    error(
                        "E015",
                        "malformed include pin (sha256 requires 64 hex digits)",
                        span=self._span(start_line, start_col),
                        file=self.name,
                    )
                )
            self._emit(T.PIN, pin, self._span(start_line, start_col))
        else:
            self._emit(T.IDENT, text, span)

    def _is_pin(self) -> bool:
        if self._peek() != ":":
            return False
        j = self.i + 1
        n = 0
        while j < len(self.src) and _is_hex(self.src[j]):
            n += 1
            j += 1
        if n != 64:
            return False
        return j >= len(self.src) or not _is_ident_char(self.src[j])

    # -- numbers and units ------------------------------------------------
    def _number(self) -> None:
        start_line, start_col = self.line, self.col
        start = self.i
        while self.i < len(self.src) and (
            self._peek().isdigit() or self._peek() == "_"
        ):
            self._advance()
        is_float = False
        if self._peek() == "." and self._peek(1).isdigit():
            is_float = True
            self._advance()
            while self.i < len(self.src) and self._peek().isdigit():
                self._advance()
        text = self.src[start : self.i].replace("_", "")
        value = float(text) if is_float else int(text)
        # A unit attaches with no whitespace. A letter/underscore/% starts
        # one; '/' starts one only when a known suffix follows, so 1/2 is
        # division and 1/s is a rate.
        unit = None
        if self._unit_starts_here():
            ustart = self.i
            ustart_line, ustart_col = self.line, self.col
            while self.i < len(self.src) and self._peek() in _unit_chars():
                self._advance()
            run = self.src[ustart : self.i]
            if run in UNITS:
                unit = run
            elif (
                run.startswith("/")
                and not run.startswith("/req/")
                and run not in ("/s", "/m", "/h")
            ):
                # Not a known '/'-suffix: leave it as the division operator.
                self.i = ustart
                self.col -= len(run)
            else:
                self.diagnostics.append(
                    error(
                        "E002",
                        f"unknown unit suffix `{run}`",
                        span=self._span(ustart_line, ustart_col),
                        file=self.name,
                    )
                )
                unit = None
        self._emit(
            T.FLOAT if is_float else T.INT,
            value,
            self._span(start_line, start_col),
            unit,
        )

    def _unit_starts_here(self) -> bool:
        ch = self._peek()
        if ch in ("%",):
            return True
        if ch.isascii() and (ch.isalpha() or ch == "_"):
            return True
        if ch in ("μ", "µ"):
            return True
        if ch == "/":
            nxt = self._peek(1)
            return nxt.isascii() and nxt.isalpha()
        return False

    # -- strings ----------------------------------------------------------
    def _string(self) -> None:
        start_line, start_col = self.line, self.col
        quote = self._peek()
        if self._peek(1) == quote and self._peek(2) == quote:
            self._multi_string(quote, start_line, start_col)
        else:
            self._single_string(quote, start_line, start_col)

    def _single_string(self, quote: str, start_line: int, start_col: int) -> None:
        self._advance()  # opening quote
        parts: list[T.StringPart] = []
        buf: list[str] = []
        while self.i < len(self.src):
            ch = self._peek()
            if ch == quote:
                self._advance()
                if buf or not parts:
                    parts.append(
                        T.StringPart(
                            "lit", "".join(buf), self._span(start_line, start_col)
                        )
                    )
                self._emit(
                    T.STRING, None, self._span(start_line, start_col), parts=parts
                )
                return
            if ch == "\n":
                break
            if ch == "\\":
                self._advance()
                esc = self._peek()
                if esc in ("\\", quote):
                    self._advance()
                    buf.append(esc)
                elif quote == '"' and esc in ("n", "t", "r", "0", "$"):
                    self._advance()
                    buf.append(
                        {"n": "\n", "t": "\t", "r": "\r", "0": "\0", "$": "$"}[esc]
                    )
                else:
                    buf.append("\\")
            elif quote == '"' and ch == "$" and self._peek(1) == "{":
                if buf:
                    parts.append(
                        T.StringPart(
                            "lit", "".join(buf), self._span(start_line, start_col)
                        )
                    )
                    buf = []
                inner, inner_span = self._interpolation()
                parts.append(T.StringPart("expr", inner, inner_span))
            else:
                buf.append(self._advance())
        # H001: unclosed string to EOF.
        self.diagnostics.append(
            info(
                "H001",
                "unclosed string healed at end of input",
                span=self._span(start_line, start_col),
                file=self.name,
            )
        )
        if buf or not parts:
            parts.append(
                T.StringPart("lit", "".join(buf), self._span(start_line, start_col))
            )
        self._emit(T.STRING, None, self._span(start_line, start_col), parts=parts)

    def _multi_string(self, quote: str, start_line: int, start_col: int) -> None:
        for _ in range(3):
            self._advance()
        raw: list[str] = []
        closed = False
        while self.i < len(self.src):
            if (
                self._peek() == quote
                and self._peek(1) == quote
                and self._peek(2) == quote
            ):
                for _ in range(3):
                    self._advance()
                closed = True
                break
            raw.append(self._advance())
        if not closed:
            self.diagnostics.append(
                info(
                    "H001",
                    "unclosed multi-line string healed at end of input",
                    span=self._span(start_line, start_col),
                    file=self.name,
                )
            )
        body = _dedent_multiline("".join(raw))
        parts = self._string_parts(body, quote == '"', start_line, start_col)
        self._emit(T.STRING, None, self._span(start_line, start_col), parts=parts)

    def _string_parts(
        self, body: str, interpolate: bool, start_line: int, start_col: int
    ) -> list[T.StringPart]:
        if not interpolate:
            return [T.StringPart("lit", body, self._span(start_line, start_col))]
        # The body was already lexed as raw text; interpolation is detected
        # here. Escapes were consumed by the multi-line scanner as raw
        # characters, so process the simple escapes now.
        parts: list[T.StringPart] = []
        buf: list[str] = []
        i = 0
        while i < len(body):
            ch = body[i]
            if ch == "\\" and i + 1 < len(body):
                esc = body[i + 1]
                if esc in ("n", "t", "r", "0", "\\", '"', "$"):
                    buf.append(
                        {
                            "n": "\n",
                            "t": "\t",
                            "r": "\r",
                            "0": "\0",
                            "\\": "\\",
                            '"': '"',
                            "$": "$",
                        }[esc]
                    )
                    i += 2
                    continue
                buf.append("\\")
                i += 1
                continue
            if ch == "$" and i + 1 < len(body) and body[i + 1] == "{":
                if buf:
                    parts.append(
                        T.StringPart(
                            "lit", "".join(buf), self._span(start_line, start_col)
                        )
                    )
                    buf = []
                depth = 1
                j = i + 2
                while j < len(body) and depth:
                    if body[j] == "{":
                        depth += 1
                    elif body[j] == "}":
                        depth -= 1
                    j += 1
                inner = body[i + 2 : j - 1] if depth == 0 else body[i + 2 :]
                parts.append(
                    T.StringPart("expr", inner, self._span(start_line, start_col))
                )
                i = j
                continue
            buf.append(ch)
            i += 1
        if buf or not parts:
            parts.append(
                T.StringPart("lit", "".join(buf), self._span(start_line, start_col))
            )
        return parts

    def _interpolation(self) -> tuple[str, Span]:
        start_line, start_col = self.line, self.col
        self._advance()  # '$'
        self._advance()  # '{'
        start = self.i
        depth = 1
        while self.i < len(self.src) and depth:
            ch = self._peek()
            if ch == '"' or ch == "'":
                # Skip a nested string, aware of escapes.
                q = ch
                self._advance()
                while self.i < len(self.src):
                    c = self._peek()
                    if c == "\\":
                        self._advance()
                        if self.i < len(self.src):
                            self._advance()
                        continue
                    self._advance()
                    if c == q:
                        break
                continue
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    inner = self.src[start : self.i]
                    self._advance()
                    return inner, self._span(start_line, start_col)
            self._advance()
        # E012: unterminated interpolation.
        raise ParseFailure(
            [
                error(
                    "E012",
                    "unterminated interpolation",
                    span=self._span(start_line, start_col),
                    file=self.name,
                )
            ]
        )

    # -- operators --------------------------------------------------------
    def _operator(self) -> None:
        start_line, start_col = self.line, self.col
        ch = self._peek()
        two = self.src[self.i : self.i + 2]
        if two in ("?:", "==", "!=", "<=", ">=", "&&", "||"):
            self._advance()
            self._advance()
            self._emit(T.OP, two, self._span(start_line, start_col))
            return
        if ch in "+-*/<>!":
            self._advance()
            self._emit(T.OP, ch, self._span(start_line, start_col))
            return
        if ch in "[]{}(),.=:":
            self._advance()
            if ch in "[{(":
                self.bracket_depth += 1
            elif ch in "]})":
                self.bracket_depth = max(0, self.bracket_depth - 1)
            self._emit(T.PUNCT, ch, self._span(start_line, start_col))
            return
        if ch == "@":
            self._advance()
            self._emit(T.AT, "@", self._span(start_line, start_col))
            return
        self._advance()
        self.diagnostics.append(
            error(
                "E005",
                f"unexpected character `{ch}`",
                span=self._span(start_line, start_col),
                file=self.name,
            )
        )


def _unit_chars() -> set[str]:
    from .units import UNIT_CHARS

    return UNIT_CHARS


def _dedent_multiline(body: str) -> str:
    if body.startswith("\r\n"):
        body = body[2:]
    elif body.startswith("\n"):
        body = body[1:]
    lines = body.split("\n")
    nonblank = [line for line in lines if line.strip()]
    if nonblank:
        prefix = nonblank[0][: len(nonblank[0]) - len(nonblank[0].lstrip(" \t"))]
        for line in nonblank[1:]:
            lead = line[: len(line) - len(line.lstrip(" \t"))]
            n = 0
            while n < len(prefix) and n < len(lead) and prefix[n] == lead[n]:
                n += 1
            prefix = prefix[:n]
        if prefix:
            lines = [line.removeprefix(prefix) for line in lines]
    if lines and not lines[-1].strip():
        lines.pop()
    return "\n".join("" if not line.strip() else line for line in lines)


def lex(source: str, name: str = "<source>") -> LexResult:
    return Lexer(source, name).run()
