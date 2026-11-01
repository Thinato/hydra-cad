<#
Nightly Hydra backup: pg_dump -Fc of the database, pg_dumpall --globals-only (roles), and a
sidecar of row counts for hydra-restore-test.ps1. See README.md.

  .\hydra-backup.ps1 [-Dir \\backuphost\hydra]

Settings (parameters win over environment):
  HYDRA_BACKUP_DIR        target directory; put it on ANOTHER machine (UNC share)
  HYDRA_BACKUP_KEEP_DAYS  delete backups older than this (default 30)
  HYDRA_PG_BIN            folder with pg_dump.exe etc. (default: PATH), e.g. C:\Program Files\PostgreSQL\15\bin
  PGHOST PGPORT PGUSER PGDATABASE (default hydra), PGPASSFILE: standard libpq settings; the
  password lives in %APPDATA%\postgresql\pgpass.conf of the account running the task.
  The user must be a superuser (pg_dumpall --globals-only reads role passwords).
#>
param(
    [string]$Dir = $env:HYDRA_BACKUP_DIR,
    [int]$KeepDays = $(if ($env:HYDRA_BACKUP_KEEP_DAYS) { [int]$env:HYDRA_BACKUP_KEEP_DAYS } else { 30 })
)
$ErrorActionPreference = 'Stop'
if (-not $env:PGDATABASE) { $env:PGDATABASE = 'hydra' }
# Keep in step with hydra-backup.sh and hydra-restore-test.*.
$tables = '_sqlx_migrations manufacturers symbol_defs assets valve_types parts blocks part_assets catalogue_proposals circuits circuit_revisions'.Split(' ')
$partials = @()

function Pg([string]$tool) {
    if ($env:HYDRA_PG_BIN) { Join-Path $env:HYDRA_PG_BIN "$tool.exe" } else { "$tool.exe" }
}
# Run a PostgreSQL tool; throw on a non-zero exit. Returns its stdout lines.
function Run([string]$tool, [string[]]$arguments) {
    $out = & (Pg $tool) @arguments
    if ($LASTEXITCODE -ne 0) { throw "$tool failed with exit code $LASTEXITCODE" }
    $out
}
function Counts {
    foreach ($t in $tables) {
        $n = (Run 'psql' @('-X', '-q', '-A', '-t', '-v', 'ON_ERROR_STOP=1', '-c', "SELECT count(*) FROM $t") | Out-String).Trim()
        "$t $n"
    }
}

try {
    if (-not $Dir) { throw 'no target directory: pass -Dir or set HYDRA_BACKUP_DIR' }
    if (-not (Test-Path -LiteralPath $Dir -PathType Container)) { throw "target directory '$Dir' is missing (is the share reachable?)" }

    $serverMajor = ((Run 'psql' @('-X', '-q', '-A', '-t', '-c', 'SHOW server_version_num')) | Out-String).Trim().Substring(0, 2)
    $clientMajor = [regex]::Match(((Run 'pg_dump' @('--version')) | Out-String), '(\d+)').Groups[1].Value
    # A newer pg_dump can emit settings the server rejects on restore (transaction_timeout, 17+ into 15).
    if ($serverMajor -ne $clientMajor) { throw "pg_dump is version $clientMajor but the server is $serverMajor; use matching client tools (HYDRA_PG_BIN)" }

    $stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ')
    $dump = Join-Path $Dir "hydra-$stamp.dump"
    $globals = Join-Path $Dir "hydra-globals-$stamp.sql"
    $sidecar = Join-Path $Dir "hydra-$stamp.counts"
    $partials = @("$dump.partial", "$globals.partial", "$sidecar.partial")

    $before = @(Counts)
    Run 'pg_dump' @('-Fc', '-f', "$dump.partial") | Out-Null
    Run 'pg_restore' @('--list', "$dump.partial") | Out-Null
    $after = @(Counts)
    Run 'pg_dumpall' @('--globals-only', '-f', "$globals.partial") | Out-Null
    # The dump holds between "before" and "after" rows per table (the restore test skips part_assets: the app deletes rows when a model is replaced).
    $lines = for ($i = 0; $i -lt $before.Count; $i++) { "$($before[$i]) $($after[$i].Split(' ')[1])" }
    [IO.File]::WriteAllText("$sidecar.partial", (($lines -join "`n") + "`n"))

    # Rename the dump last: if it exists under its final name, its siblings do too.
    Move-Item -LiteralPath "$globals.partial" -Destination $globals
    Move-Item -LiteralPath "$sidecar.partial" -Destination $sidecar
    Move-Item -LiteralPath "$dump.partial" -Destination $dump
    $partials = @()

    # Prune only after a successful backup, so a failing night never eats the history.
    $cutoff = (Get-Date).AddDays(-$KeepDays)
    Get-ChildItem -LiteralPath $Dir -File | Where-Object {
        $_.Name -match '^hydra-(globals-)?\d{8}T\d{6}Z\.(dump|counts|sql)$' -and $_.LastWriteTime -lt $cutoff
    } | Remove-Item -Force
    Get-ChildItem -LiteralPath $Dir -File -Filter 'hydra-*.partial' | Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-1) } | Remove-Item -Force
    Write-Output "hydra-backup: OK $dump ($((Get-Item -LiteralPath $dump).Length) bytes)"
}
catch {
    foreach ($p in $partials) { Remove-Item -LiteralPath $p -Force -ErrorAction SilentlyContinue }
    [Console]::Error.WriteLine("hydra-backup: FAILED: $($_.Exception.Message)")
    exit 1
}
