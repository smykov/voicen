# licenses-stale guard fixtures (T-063)

Run by `scripts/licenses/stale-check.sh` against the real Makefile recipe
`licenses-stale` (`LICENSES_DIR=<case>/fresh NOTICES=<case>/committed/THIRD-PARTY-NOTICES.txt`):

| Case | fresh/ | committed/ | Expected |
|---|---|---|---|
| `equal` | notices | the same bytes | exit 0 |
| `stale` | `License: Apache-2.0` | `License: MIT` (one line differs) | non-zero, "is stale", the changed line |
| `missing` | notices | none | non-zero, "is not committed" |

Not real notices: fixture text only.
