-- Append-only audit log (SPEC rule 10). One row per step of a chat turn.
CREATE TABLE audit (
    id         INTEGER PRIMARY KEY,
    ts         TEXT NOT NULL,
    session_id TEXT,
    kind       TEXT NOT NULL,
    payload    TEXT NOT NULL
);

CREATE INDEX audit_session_idx ON audit (session_id, id);
CREATE INDEX audit_kind_idx ON audit (kind, id);

-- Defence in depth: rows can be inserted, never changed or removed.
CREATE TRIGGER audit_no_update BEFORE UPDATE ON audit
BEGIN
    SELECT RAISE(ABORT, 'audit is append-only');
END;

CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit
BEGIN
    SELECT RAISE(ABORT, 'audit is append-only');
END;
