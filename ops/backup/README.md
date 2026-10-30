# Hydra database backups

The app needs a live connection, so the on-site Postgres server is a single point of failure. Every night a script writes three files to a directory **on another machine**:

| File | What |
|---|---|
| `hydra-<UTC stamp>.dump` | `pg_dump -Fc` of the `hydra` database (schema, data, triggers, 3D model bytea) |
| `hydra-globals-<stamp>.sql` | `pg_dumpall --globals-only`: **roles** and their password hashes (each user signs in with their own role) |
| `hydra-<stamp>.counts` | row count of every table before and after the dump, used by the restore test |

Files are written as `*.partial` and renamed only when complete (the dump last), so a half-written backup never looks real. Backups older than 30 days are deleted, only after a successful run. Any failure prints `hydra-backup: FAILED: <reason>` and exits 1, which the scheduler records.

## Rules that matter

- **Client tools must match the server major version** (PostgreSQL 15 server: `pg_dump` 15; 18: 18). Older clients refuse to dump a newer server. A *newer* client "works" but writes settings an older server rejects on restore, for example `SET transaction_timeout` from `pg_dump` 17+ into 15 (`unrecognized configuration parameter`). The backup script therefore refuses to run on a mismatch. Restore with the same major as the dump, or newer (restoring 15 dumps into 18 is fine, the other way is not).
- **The backup user must be a superuser**: `pg_dumpall --globals-only` reads `pg_authid`. Keep its password in `.pgpass` / `pgpass.conf`, never in scripts or command lines.
- **Target directory on another machine** (UNC share or NFS mount). A backup on the same disk dies with the server. The service account needs write access; nothing else needs to.
- **Backups are owner-only** (`umask 077` in the script, `UMask=0077` in the unit). `RequiresMountsFor=` in `hydra-backup.service` must name the same path as `HYDRA_BACKUP_DIR`, so a dropped mount fails the run instead of filling the local disk. On Windows, restrict the share's and folder's ACL to the backup account and admins yourself: the script cannot.
- Only files named `hydra-<YYYYMMDD>T<HHMMSS>Z.*` are pruned or picked as newest; a manual `hydra-before-v1.1.dump` is left alone.
- Roles live in the cluster, not the database. A dump without the globals file restores data but not who may log in or who owns what.

## Setup: Linux

1. Install `postgresql-client-<server major>` (from the PGDG repo if the distro lacks it). Mount the target, e.g. `/mnt/hydra-backups` (NFS/CIFS in `/etc/fstab`).
2. Create user `hydra-backup` and `~hydra-backup/.pgpass` (`chmod 600`): `localhost:5432:*:postgres:<password>`.
3. Copy `ops/backup` to `/opt/hydra/ops/backup`, edit the `Environment=` lines in `hydra-backup.service`, then:
   ```sh
   sudo cp hydra-backup.service hydra-backup.timer /etc/systemd/system/
   sudo systemctl daemon-reload && sudo systemctl enable --now hydra-backup.timer
   sudo systemctl start hydra-backup.service && journalctl -u hydra-backup -n 20   # first run now
   ```
   Cron alternative: `0 2 * * * hydra-backup PGHOST=localhost PGUSER=postgres HYDRA_BACKUP_DIR=/mnt/hydra-backups /opt/hydra/ops/backup/hydra-backup.sh >>/var/log/hydra-backup.log 2>&1`.

## Setup: Windows Server (EDB installer)

1. The installer's tools are in `C:\Program Files\PostgreSQL\15\bin` (use the matching major). Create `%APPDATA%\postgresql\pgpass.conf` **for the service account** (log on as it once, or write it into its profile): `localhost:5432:*:postgres:<password>`.
2. Give the service account write access to the UNC share, e.g. `\\backuphost\hydra`, and the "Log on as a batch job" right.
3. From an elevated PowerShell:
   ```powershell
   .\register-task.ps1 -Account 'DOMAIN\svc-hydra-backup' -BackupDir '\\backuphost\hydra'
   Start-ScheduledTask 'Hydra backup'; Get-ScheduledTaskInfo 'Hydra backup'   # LastTaskResult 0 = OK
   ```
   If scripts are blocked, the task already passes `-ExecutionPolicy Bypass`; for manual runs use `powershell -ExecutionPolicy Bypass -File .\hydra-backup.ps1 -Dir ...`.

## Restore test (monthly drill, and in CI on every push)

Restores into a scratch database (never the live name), runs `hydra-admin check`, compares each table's row count with the range in the `.counts` file (except `part_assets`: the app deletes rows there when a 3D model is replaced), checks that the immutability triggers exist and still refuse an `UPDATE` with their own error, then drops the scratch database. Any problem exits 1.

```sh
export HYDRA_ADMIN=/opt/hydra/hydra-admin PGHOST=localhost PGUSER=postgres
./hydra-restore-test.sh --dir /mnt/hydra-backups            # newest dump; --dump FILE for a specific one; --keep to inspect
```
```powershell
$env:HYDRA_ADMIN='C:\hydra\hydra-admin.exe'; $env:HYDRA_PG_BIN='C:\Program Files\PostgreSQL\15\bin'
.\hydra-restore-test.ps1 -Dir '\\backuphost\hydra'
```

**Monthly drill**: run it on the newest *and* one older dump, ideally from a different machine than the DB server (proves the dump is usable without the server). Then once, on a scratch machine, add `--globals` to prove the roles restore (do not use `--globals` on the live cluster: roles are cluster-wide and already exist). Write the date down.

## Real restore: replacing a broken server

1. Install PostgreSQL of the **same major** as the dumps (or newer) with matching client tools; start it; make it reachable at the address users already have (or plan step 6).
2. Pick the newest complete backup (`hydra-<stamp>.dump` plus the same-stamp globals file). Roles first, as a superuser:
   ```sh
   psql -X -d postgres -v ON_ERROR_STOP=1 -f hydra-globals-<stamp>.sql
   ```
   The `postgres` role already exists on a new server: its `CREATE ROLE` error is expected, so run without `ON_ERROR_STOP` if it stops there and read the output.
3. Create the database and restore, keeping owners and grants:
   ```sh
   createdb -T template0 hydra
   pg_restore --exit-on-error -d hydra hydra-<stamp>.dump
   ```
4. Verify: `DATABASE_URL=postgres:///hydra ./hydra-admin check` must print `OK`; `hydra-admin models` checks stored 3D models.
5. Configure `pg_hba.conf` and `listen_addresses` like the old server (users connect from other machines), reload.
6. Point users at the new server (same DNS name, or change the connection settings in each Hydra app), have one user open a circuit, and re-register the backup job on the new machine.

Work done after the last backup (up to a day) is lost; circuits are append-only revisions, so users can re-save what they had open.

## Not covered

No point-in-time recovery (WAL archiving) and no encryption of dumps: they hold role password hashes, so restrict the share to the backup account and the admins.
