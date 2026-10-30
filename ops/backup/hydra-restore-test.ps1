<#
Restore a Hydra backup into a scratch database and verify it. Exits non-zero on any problem.

  .\hydra-restore-test.ps1 [-Dir DIR] [-Dump FILE] [-Target DB] [-Globals] [-Keep]

  -Dir     backup directory; newest timestamped hydra-<stamp>.dump is used (default $env:HYDRA_BACKUP_DIR)
  -Dump    restore this dump instead (its .counts / globals siblings are used)
  -Target  scratch database name (default hydra_restore_test); refused if it is the live database
           name ($env:PGDATABASE, default hydra)
  -Globals first apply the globals file (roles). Roles are cluster-wide: only on a scratch server.
  -Keep    do not drop the scratch database afterwards
Environment: HYDRA_ADMIN (path to hydra-admin.exe, required), HYDRA_PG_BIN (see hydra-backup.ps1),
PGHOST PGPORT PGUSER PGPASSFILE (a user allowed to CREATE/DROP DATABASE).
#>
param(
    [string]$Dir = $env:HYDRA_BACKUP_DIR,
    [string]$Dump,
    [string]$Target = 'hydra_restore_test',
    [switch]$Globals,
    [switch]$Keep
)
$ErrorActionPreference = 'Stop'
$live = if ($env:PGDATABASE) { $env:PGDATABASE } else { 'hydra' }
$created = $false   # only ever drop a database this run created

function Pg([string]$tool) {
    if ($env:HYDRA_PG_BIN) { Join-Path $env:HYDRA_PG_BIN "$tool.exe" } else { "$tool.exe" }
}
function Run([string]$tool, [string[]]$arguments) {
    $out = & (Pg $tool) @arguments
    if ($LASTEXITCODE -ne 0) { throw "$tool failed with exit code $LASTEXITCODE" }
    $out
}
function Sql([string]$db, [string[]]$more) {
    (Run 'psql' (@('-X', '-q', '-A', '-t', '-v', 'ON_ERROR_STOP=1', '-d', $db) + $more) | Out-String).Trim()
}

try {
    if ($Target -in @($live, 'hydra', 'hydra_app')) { throw "refusing to restore into '$Target': it is (or looks like) the live database" }
    if ($Target -notmatch '^[A-Za-z0-9_]+$') { throw 'target name must be letters, digits, underscore' }
    if (-not $env:HYDRA_ADMIN -or -not (Test-Path -LiteralPath $env:HYDRA_ADMIN)) { throw 'set HYDRA_ADMIN to the hydra-admin.exe path' }
    if (-not $Dump) {
        if (-not $Dir -or -not (Test-Path -LiteralPath $Dir -PathType Container)) { throw 'no -Dump and no readable backup directory (-Dir / HYDRA_BACKUP_DIR)' }
        $newest = Get-ChildItem -LiteralPath $Dir -File | Where-Object { $_.Name -match '^hydra-\d{8}T\d{6}Z\.dump$' } | Sort-Object Name | Select-Object -Last 1
        if (-not $newest) { throw "no hydra-<stamp>.dump in '$Dir'" }
        $Dump = $newest.FullName
    }
    if (-not (Test-Path -LiteralPath $Dump) -or (Get-Item -LiteralPath $Dump).Length -eq 0) { throw "dump '$Dump' is missing or empty" }
    $stem = $Dump -replace '\.dump$', ''
    $sidecar = "$stem.counts"
    $globalsFile = Join-Path (Split-Path -Parent $Dump) ("hydra-globals-" + ((Split-Path -Leaf $stem) -replace '^hydra-', '') + ".sql")
    if (-not (Test-Path -LiteralPath $sidecar)) { throw "no row-count sidecar '$sidecar'" }
    Write-Output "hydra-restore-test: restoring $Dump into '$Target'"

    if (Sql 'postgres' @('-c', "SELECT 1 FROM pg_database WHERE datname = '$Target'")) { throw "database '$Target' already exists; drop it or pick another -Target" }
    if ($Globals) {
        if (-not (Test-Path -LiteralPath $globalsFile)) { throw "no globals file '$globalsFile'" }
        Sql 'postgres' @('-f', $globalsFile) | Out-Null
    }
    Sql 'postgres' @('-c', "CREATE DATABASE $Target TEMPLATE template0") | Out-Null
    $created = $true

    $opts = @('--exit-on-error')
    if (-not $Globals) { $opts += @('--no-owner', '--no-privileges') }   # roles are not present on a scratch run
    Run 'pg_restore' ($opts + @('-d', $Target, $Dump)) | Out-Null

    $env:DATABASE_URL = "postgres:///$Target"
    & $env:HYDRA_ADMIN check
    if ($LASTEXITCODE -ne 0) { throw 'hydra-admin check failed on the restored database' }

    $bad = $false
    foreach ($line in Get-Content -LiteralPath $sidecar) {
        if (-not $line.Trim()) { continue }
        $table, $lo, $hi = $line.Trim().Split(' ')
        $n = [int64](Sql $Target @('-c', "SELECT count(*) FROM $table"))
        # The app deletes part_assets rows when a model is replaced, so no range check.
        if ($table -eq 'part_assets') { Write-Output "  ${table}: $n rows (not range-checked)"; continue }
        if ($n -lt [int64]$lo -or $n -gt [int64]$hi) {
            [Console]::Error.WriteLine("hydra-restore-test: $table has $n rows, backup recorded $lo..$hi"); $bad = $true
        } else { Write-Output "  ${table}: $n rows (recorded $lo..$hi)" }
    }
    if ($bad) { throw 'row counts do not match the backup' }

    # Triggers must have come back and still fire, not just exist.
    # The expected set is whatever the dump itself lists, so new migrations need no edit here.
    $expected = @((Run 'pg_restore' @('--list', $Dump)) -match ' TRIGGER ').Count
    $triggers = [int64](Sql $Target @('-c', 'SELECT count(*) FROM pg_trigger WHERE NOT tgisinternal'))
    if ($expected -le 0 -or $triggers -ne $expected) { throw "dump lists $expected triggers, restored database has $triggers" }
    if ((Sql $Target @('-c', "SELECT count(*) FROM pg_trigger WHERE tgname = 'symbol_defs_immutable'")) -ne '1') { throw 'symbol_defs_immutable trigger is missing' }
    if ([int64](Sql $Target @('-c', 'SELECT count(*) FROM symbol_defs')) -gt 0) {
        # An UPDATE only the trigger can refuse (a DELETE would also fail on the valve_types foreign key).
        # Windows PowerShell 5.1 turns psql's stderr into a terminating error, so catch it and read its text.
        $err = ''; $updated = $false
        try { $err = (& (Pg 'psql') -X -q -v ON_ERROR_STOP=1 -d $Target -c 'UPDATE symbol_defs SET key = key' 2>&1 | Out-String); $updated = ($LASTEXITCODE -eq 0) } catch { $err = $_.Exception.Message }
        if ($updated) { throw 'symbol_defs rows can be updated: immutability trigger does not fire' }
        if ($err -notmatch 'never edited in place') { throw "symbol_defs update was refused, but not by the immutability trigger: $err" }
    }

    if ($Keep) { Write-Output "hydra-restore-test: OK (kept database '$Target')" }
    else {
        Sql 'postgres' @('-c', "DROP DATABASE $Target WITH (FORCE)") | Out-Null
        $created = $false
        Write-Output "hydra-restore-test: OK (dropped '$Target')"
    }
}
catch {
    [Console]::Error.WriteLine("hydra-restore-test: FAILED: $($_.Exception.Message)")
    if ($created -and -not $Keep) {
        try { Sql 'postgres' @('-c', "DROP DATABASE IF EXISTS $Target WITH (FORCE)") | Out-Null } catch { [Console]::Error.WriteLine("hydra-restore-test: could not drop '$Target'") }
    }
    exit 1
}
