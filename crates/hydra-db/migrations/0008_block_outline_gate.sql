-- Gate only: circuit documents of schema 4 list the parts inside the block (in_block), which
-- older builds would drop on save. Applying this makes them refuse the database instead.
COMMENT ON COLUMN circuit_revisions.document IS 'Circuit JSON; see hydra-core circuit.rs';
