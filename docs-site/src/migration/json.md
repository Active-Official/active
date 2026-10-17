# From JSON

JSON is data without comments, trailing commas, or types beyond its
six. ACTIVE evaluates to JSON cleanly (`ACTIVE convert --to json`),
so migration is lossless in the data direction and additive coming
back: comments, references, units, and schemas.

JSON in:

```json
{"server": {"host": "localhost", "port": 8080}}
```

ACTIVE out:

```active
{{#include ./json.active}}
```

Inline tables (`{ a = 1 }`) cover the dense JSON style during
transition; expand to block tables when humans own the file. JSON
data maps losslessly (`ACTIVE import --from json`: objects, arrays,
scalars round-trip; what JSON cannot say — comments, references,
units — gets added going forward, never guessed coming in).
