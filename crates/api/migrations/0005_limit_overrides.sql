-- Trading limits tightened from the UI. One global row (id = 1) that applies to every user
-- and persona; the gate enforces the config limits intersected with these values, so an
-- override can only make the limits stricter. Removing the row restores the config limits.
CREATE TABLE limit_overrides (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    overrides  TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    updated_by TEXT
);
