<#
.SYNOPSIS
  Mount a Btrfs image read-only as a Windows drive letter via WinFsp.

.DESCRIPTION
  Thin PowerShell wrapper around `btrfs.exe mount`. Accepts an optional
  image path; if omitted, opens an Open-File dialog so the user can pick
  one. Picks the first free drive letter (Z:..D:) and runs the mount in
  the foreground -- Ctrl-C unmounts.

  Installed alongside btrfs.exe in `%ProgramFiles%\btrfs-win-driver\`.

.PARAMETER ImagePath
  Path to a Btrfs image (.img) or whole-disk image. Optional.

.PARAMETER DriveLetter
  Force a specific drive letter (e.g. 'X:'). Optional. Default: first free.

.PARAMETER Part
  1-indexed partition number for whole-disk images. Optional.

.EXAMPLE
  Mount-Btrfs.ps1 C:\images\rootfs.img
  Mount-Btrfs.ps1 -ImagePath disk.img -DriveLetter Y: -Part 1
  Mount-Btrfs.ps1                                # opens file picker
#>

[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string]$ImagePath,

    [Parameter()]
    [string]$DriveLetter,

    [Parameter()]
    [int]$Part
)

$ErrorActionPreference = 'Stop'

# ----------------------------------------------------------------------------
# Locate btrfs.exe. Installed copy lives next to this script in Program Files;
# fall back to PATH for dev runs.
# ----------------------------------------------------------------------------
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$exeNextToScript = Join-Path $scriptDir 'btrfs.exe'

if (Test-Path $exeNextToScript) {
    $btrfsExe = $exeNextToScript
} else {
    $cmd = Get-Command btrfs.exe -ErrorAction SilentlyContinue
    if (-not $cmd) {
        Write-Error "btrfs.exe not found next to this script ($scriptDir) or on PATH."
        exit 1
    }
    $btrfsExe = $cmd.Source
}

# ----------------------------------------------------------------------------
# If no image path was supplied, pop the Open-File dialog.
# ----------------------------------------------------------------------------
if (-not $ImagePath) {
    Add-Type -AssemblyName System.Windows.Forms
    $dlg = New-Object System.Windows.Forms.OpenFileDialog
    $dlg.Title = 'Select a Btrfs image to mount'
    $dlg.Filter = 'Disk images (*.img;*.iso;*.bin;*.raw)|*.img;*.iso;*.bin;*.raw|All files (*.*)|*.*'
    $dlg.CheckFileExists = $true
    if ($dlg.ShowDialog() -ne [System.Windows.Forms.DialogResult]::OK) {
        Write-Host 'Cancelled.'
        exit 0
    }
    $ImagePath = $dlg.FileName
}

if (-not (Test-Path $ImagePath)) {
    Write-Error "Image not found: $ImagePath"
    exit 1
}

# ----------------------------------------------------------------------------
# Pick a drive letter if the user didn't supply one. Walk Z..D and grab the
# first letter that isn't currently a PSDrive.
# ----------------------------------------------------------------------------
function Get-FreeDriveLetter {
    $used = @(Get-PSDrive -PSProvider FileSystem | ForEach-Object { $_.Name.ToUpper() })
    foreach ($c in [char[]]'ZYXWVUTSRQPONMLKJIHGFED') {
        if ($used -notcontains "$c") { return "${c}:" }
    }
    throw 'No free drive letter available (D..Z all in use).'
}

if (-not $DriveLetter) {
    $DriveLetter = Get-FreeDriveLetter
}

# Normalise: 'X' -> 'X:'.
if ($DriveLetter -notmatch ':$') { $DriveLetter += ':' }

# ----------------------------------------------------------------------------
# Run the mount in the foreground. Ctrl-C in the console unmounts.
# ----------------------------------------------------------------------------
$args = @('mount', $ImagePath, '--drive', $DriveLetter)
if ($PSBoundParameters.ContainsKey('Part')) {
    $args += @('--part', "$Part")
}
# The driver mounts read-only; there is no write mode to choose.
Write-Host "Mounting $ImagePath at $DriveLetter (read-only) ..."
Write-Host "Ctrl-C in this window to unmount."
Write-Host ""

& $btrfsExe @args
exit $LASTEXITCODE
