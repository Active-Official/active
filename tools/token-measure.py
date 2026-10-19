"""Token-cost comparison: equivalent Active vs JSON vs YAML documents.

Supporting evidence only (01 section 5): published numbers inform, never
drive language design; readability wins ties. Tokenizer: tiktoken
cl100k_base when importable (a GPT-4-class approximation, labeled in
output), else a documented chars-per-4 heuristic (labeled as such).
Corpus: the Kit A/B/C after-files plus the playground service example,
each paired with a hand-written JSON and YAML equivalent kept beside
this script under tools/token-corpus/.
Usage: python tools/token-measure.py
"""

import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent / "token-corpus"

try:
    import tiktoken

    ENCODING = tiktoken.get_encoding("cl100k_base")
    METHOD = "tiktoken cl100k_base (GPT-4-class approximation)"
except ImportError:
    ENCODING = None
    METHOD = "chars/4 heuristic (tiktoken absent)"


def count(text):
    if ENCODING is not None:
        return len(ENCODING.encode(text))
    return max(1, len(text) // 4)


def main():
    print("method:", METHOD)
    rows = []
    for active_path in sorted(ROOT.glob("*.active")):
        stem = active_path.stem
        texts = {"active": active_path.read_text(encoding="utf-8")}
        for fmt, ext in (("json", ".json"), ("yaml", ".yaml")):
            sidecar = active_path.with_suffix(ext)
            if sidecar.is_file():
                texts[fmt] = sidecar.read_text(encoding="utf-8")
        row = {"case": stem}
        for fmt, text in texts.items():
            row[fmt] = count(text)
        rows.append(row)
    total = {}
    for row in rows:
        print(
            "%-24s %s"
            % (row["case"], " ".join(f"{k}={v}" for k, v in row.items() if k != "case"))
        )
        for k, v in row.items():
            if k != "case":
                total[k] = total.get(k, 0) + v
    print("TOTAL", " ".join(f"{k}={v}" for k, v in sorted(total.items())))
    return 0


if __name__ == "__main__":
    sys.exit(main())
