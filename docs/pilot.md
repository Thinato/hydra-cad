# Release checklist and pilot

## Before each release

Run on one Windows 10, one Windows 11 and one macOS 15 machine with real GPUs, against the
on-site database, using the signed installer from the draft release.

1. Install. Windows: no SmartScreen warning. macOS: no Gatekeeper warning on first launch.
2. Start Hydra. The sign-in window names the right server; a wrong password says so; with
   "Remember me" ticked, the next start signs in without asking.
3. Catalogue: search finds RPE3-06; open a datasheet; the 3D tab shows a valve.
4. Schematic: open a saved circuit, move a valve, connect two ports, undo, redo, save. Open
   History, preview and restore an older revision.
5. Proposals: review a pending proposal; reject one with a note.
6. Export the open circuit as SVG, PDF and BOM. The PDF prints fitted to A4 with readable labels;
   the BOM opens in Excel with accents intact.
7. With an older version installed, start it: it offers the new installer.
8. Pull the network cable: Hydra says it can't reach the database instead of hanging.

Before rolling out a release with new migrations: take a backup, run
`hydra-admin migrate` then `psql -f ops/roles.sql` as the admin, and only then update the
engineers' machines (older apps refuse a newer schema).

## Pilot

Two or three engineers use Hydra for real work for two to four weeks before it goes to
everyone.

- Setup: the database on its final server with TLS, `ops/roles.sql` applied, one login role
  per engineer, nightly backups running, and one restore drill done
  ([ops/backup/README.md](../ops/backup/README.md)).
- Each pilot engineer redraws one real circuit they know (Germani first), imports at least one
  valve from its datasheet through the AI assistant, and exports a PDF and BOM for a real job.
- Collect problems in one place, with the circuit name and what they expected. Fix blockers
  during the pilot; the rest goes on the list for after v1.0.
- Exit: every pilot engineer would rather use Hydra than the old way for their next circuit,
  and no open blocker remains. Then release v1.0 to all engineers.
