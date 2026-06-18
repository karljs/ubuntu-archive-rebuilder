# Ubuntu Archive Rebuilder Frontend

Static HTML/JS UI for browsing rebuild results. Uses
[sql.js](https://sql.js.org/) to query a stripped SQLite database
directly in the browser.

## Setup

```bash
# Export data from the backend
cd ../backend
./target/release/rebuilder export --output-dir ../frontend/data

# Serve
python3 -m http.server 8000 --directory ../frontend
# Open http://localhost:8000
```

## Data layout

```
data/
├── rebuild.db
└── logs/<id>.log
```

It should fail gracefully if there are no logs, as the temporary
Github Pages hosting has very limited disk space.
