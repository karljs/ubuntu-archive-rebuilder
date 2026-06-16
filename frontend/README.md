# Ubuntu Archive Rebuilder — Frontend

Static HTML/JS UI for browsing rebuild results. Uses [sql.js](https://sql.js.org/) to query a stripped SQLite database directly in the browser.

## Setup

```bash
# Export data from the backend
cd ../backend
./target/release/rebuilder export --output-dir ../frontend/data

# Serve
python3 -m http.server 8000 --directory ../frontend
# Open http://localhost:8000
```

The frontend loads `data/rebuild.db` over HTTP and queries it entirely in the browser via WebAssembly. No server-side API is needed.

## Data layout

```
data/
├── rebuild.db          — all batches, builds, and findings (build logs stripped)
└── logs/<id>.log       — one file per build with a non-null log (fetched on demand)
```

The database is produced by `rebuilder export`. Build logs are stored separately to keep the database file small enough for the browser to load (~2–5 MB per 1000-package batch).

## Tabs

- **Overview** — Success rate matrix across all compiler versions and series. Click any profile row to open Details.
- **Details** — Deep dive into a single batch: sortable build table, issue categories, profile comparisons, and version trends.
- **Compare** — Select any number of batches to compare package-level outcomes side by side.
