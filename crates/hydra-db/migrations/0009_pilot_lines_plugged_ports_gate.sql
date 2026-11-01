-- Gate only: circuit documents of schema 5 mark pilot and drain lines and plugged ports, and
-- relief and flow control symbol definitions may have a pilot port X; older builds would drop
-- these on save. Applying this makes them refuse the database instead.
COMMENT ON COLUMN circuit_revisions.document IS 'Circuit JSON (schema 5); see hydra-core circuit.rs';
