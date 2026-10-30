-- Per-user preferences (docs/api.md, "Personas and user settings"). One row per signed-in
-- user; with login off the fixed key 'local' is used. The JSON holds the whole settings
-- object (persona, llm_provider, voice, watchlist, aliases).
CREATE TABLE user_settings (
    username   TEXT PRIMARY KEY COLLATE NOCASE,
    settings   TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
