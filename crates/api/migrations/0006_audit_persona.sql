-- The persona that was active when a step happened (SPEC rule 10). NULL for rows written
-- before personas existed and for steps that belong to no conversation (kill switch, login).
ALTER TABLE audit ADD COLUMN persona TEXT;
