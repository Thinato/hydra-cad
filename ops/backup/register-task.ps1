<#
Register the nightly Hydra backup in Task Scheduler. Run once from an elevated PowerShell.

  .\register-task.ps1 -Account 'DOMAIN\svc-hydra-backup' -BackupDir '\\backuphost\hydra' [-At 02:00] [-PgBin 'C:\Program Files\PostgreSQL\15\bin']

The account needs: "Log on as a batch job", write access to BackupDir, and a pgpass.conf in its
profile (%APPDATA%\postgresql\pgpass.conf) with a superuser entry. You are asked for its password
once; Windows stores it for the task. Use a group-managed service account instead by passing
-Gmsa (then no password is asked).
#>
param(
    [Parameter(Mandatory)][string]$Account,
    [Parameter(Mandatory)][string]$BackupDir,
    [string]$At = '02:00',
    [string]$PgBin = 'C:\Program Files\PostgreSQL\15\bin',
    [string]$PgHost = 'localhost',
    [string]$PgUser = 'postgres',
    [int]$KeepDays = 30,
    [switch]$Gmsa
)
$ErrorActionPreference = 'Stop'
$script = Join-Path $PSScriptRoot 'hydra-backup.ps1'
# Task Scheduler gives the task no environment of ours, so the settings travel in the command.
$command = "`$env:PGHOST='$PgHost'; `$env:PGUSER='$PgUser'; `$env:HYDRA_PG_BIN='$PgBin'; " +
    "& '$script' -Dir '$BackupDir' -KeepDays $KeepDays; exit `$LASTEXITCODE"
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command `"$command`""
$trigger = New-ScheduledTaskTrigger -Daily -At $At
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -ExecutionTimeLimit (New-TimeSpan -Hours 2) -RestartCount 2 -RestartInterval (New-TimeSpan -Minutes 15)
if ($Gmsa) {
    $principal = New-ScheduledTaskPrincipal -UserId $Account -LogonType Password -RunLevel Limited
    Register-ScheduledTask -TaskName 'Hydra backup' -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Force | Out-Null
} else {
    $cred = Get-Credential -UserName $Account -Message 'Password of the backup service account'
    Register-ScheduledTask -TaskName 'Hydra backup' -Action $action -Trigger $trigger -Settings $settings `
        -User $cred.UserName -Password $cred.GetNetworkCredential().Password -RunLevel Limited -Force | Out-Null
}
Write-Output "Registered 'Hydra backup' (daily $At as $Account). Test it: Start-ScheduledTask 'Hydra backup'; Get-ScheduledTaskInfo 'Hydra backup'"
