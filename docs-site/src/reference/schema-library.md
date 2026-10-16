# Schema library

The standard schema library (`schemas/`, C-062) will ship versioned,
reusable schemas and include fragments: HTTP servers, databases,
observability, feature flags, and AI agents. Pulled in with a
literal include — `include "schemas/http-server.solid"` — once it
lands. Each schema is validated by the test suite against good and
bad sample documents, and rendered reference pages are generated
from the schemas themselves (`solid doc`), so the documentation
below can never drift from the enforced contract.

> The library lands with C-062; this page tracks it.
