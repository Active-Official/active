# From JSON

JSON is data without comments, trailing commas, or types beyond its
six. SOLID evaluates to JSON cleanly (`solid convert --to json`),
so migration is lossless in the data direction and additive coming
back: comments, references, units, and schemas.

JSON in:

```json
{"server": {"host": "localhost", "port": 8080}}
```

SOLID out:

```solid
{{#include ./json.solid}}
```

Inline tables (`{ a = 1 }`) cover the dense JSON style during
transition; expand to block tables when humans own the file. JSON
data maps losslessly (`solid import --from json`: objects, arrays,
scalars round-trip; what JSON cannot say — comments, references,
units — gets added going forward, never guessed coming in).
