-- The kill switch survives a restart (SPEC rule 6). One row, id = 1: a crash or a service
-- restart must not quietly turn trading back on. Releasing needs a session and a TOTP code
-- (see docs/api.md), so a restart is not a way around that.
CREATE TABLE kill_switch (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    engaged       INTEGER NOT NULL,
    reason        TEXT,
    changed_at_ms INTEGER
);
