#requires -Version 5.1
<#
.SYNOPSIS
  Uninstall btrfs-win-driver silently and assert nothing it installed is
  left behind (#7).

.DESCRIPTION
  The counterpart of verify-silent.ps1. Runs Setup.exe /uninstall /quiet
  under a hard timeout, then checks every footprint Product.wxs creates is
  gone:

    - the install folder and btrfs.exe in it;
    - the BtrfsWatcher service;
    - the install folder on the machine PATH;
    - the WinFsp launcher registration (HKLM\SOFTWARE\WOW6432Node\WinFsp\
      Services\btrfs-mount);
    - the "Mount as Btrfs" verb on .img files;
    - the Start-menu folder;
    - an Add/Remove Programs entry naming btrfs-win-driver.

  WinFsp itself is NOT checked: the bundle marks it permanent, because
  other programs may rely on it.
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$SetupPath,

  [int]$TimeoutSeconds = 240,

  [string]$InstallDir = (Join-Path $env:ProgramFiles 'btrfs-win-driver')
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path $SetupPath)) {
  throw "Setup.exe not found at: $SetupPath"
}

$logPath = Join-Path ([System.IO.Path]::GetDirectoryName((Resolve-Path $SetupPath))) 'verify-uninstall.log'
if (Test-Path $logPath) { Remove-Item $logPath -Force }

Write-Host "verify-uninstall: launching '$SetupPath /uninstall /quiet /norestart' (timeout ${TimeoutSeconds}s, log: $logPath)"
$proc = Start-Process -FilePath $SetupPath `
  -ArgumentList '/uninstall', '/quiet', '/norestart', '/log', $logPath `
  -PassThru

if (-not $proc.WaitForExit($TimeoutSeconds * 1000)) {
  try { $proc.Kill() } catch {}
  if (Test-Path $logPath) { Get-Content $logPath -Tail 60 }
  throw "Setup.exe /uninstall /quiet did not exit within ${TimeoutSeconds}s."
}

$exit = $proc.ExitCode
Write-Host "verify-uninstall: Setup.exe exited with code $exit"
if ($exit -ne 0 -and $exit -ne 3010) {
  if (Test-Path $logPath) { Get-Content $logPath -Tail 80 }
  throw "Setup.exe /uninstall /quiet returned exit code $exit (expected 0 or 3010)."
}

$left = @()
if (Test-Path $InstallDir) {
  $left += "the install folder $InstallDir ($((Get-ChildItem $InstallDir -Recurse | Measure-Object).Count) entries)"
}
if (Get-Service -Name 'BtrfsWatcher' -ErrorAction SilentlyContinue) {
  $left += 'the BtrfsWatcher service'
}
$machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
if (($machinePath -split ';') -contains $InstallDir -or ($machinePath -split ';') -contains "$InstallDir\") {
  $left += "$InstallDir on the machine PATH"
}
foreach ($key in @(
    'HKLM:\SOFTWARE\WOW6432Node\WinFsp\Services\btrfs-mount',
    'HKLM:\SOFTWARE\WinFsp\Services\btrfs-mount',
    'Registry::HKEY_CLASSES_ROOT\SystemFileAssociations\.img\shell\MountAsBtrfs')) {
  if (Test-Path $key) { $left += "the registry key $key" }
}
$startMenu = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\btrfs-win-driver'
if (Test-Path $startMenu) { $left += "the Start-menu folder $startMenu" }
foreach ($root in @(
    'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall',
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall')) {
  Get-ChildItem $root -ErrorAction SilentlyContinue | ForEach-Object {
    $name = (Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue).DisplayName
    if ($name -like '*btrfs-win-driver*') { $left += "the Add/Remove Programs entry '$name'" }
  }
}

if ($left.Count -gt 0) {
  if (Test-Path $logPath) { Get-Content $logPath -Tail 40 }
  throw "verify-uninstall: the uninstall left behind:`n  $($left -join "`n  ")"
}
Write-Host "verify-uninstall: PASS -- exit $exit, nothing left behind."
