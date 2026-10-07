# scripts/vm/win-overwrite-via-mount.ps1 -- the matrix's in-place write op.
#
# Mounts the volume, opens an EXISTING file without truncating it, writes
# -Content (UTF-8, no BOM) at -Offset, flushes it through to the image and
# unmounts. -Expect says what has to happen:
#
#   written  the open, the write and the flush all succeed.
#   refused  opening the file for writing fails, as it must on a read-only
#            mount or for a file the driver cannot overwrite in place. A
#            file that is not there at all is a failure, not a refusal.
#
# This repository's op rather than the harness's: the harness's
# win-write.ps1 replaces a file (File.WriteAllBytes truncates it first),
# which is exactly what an in-place write cannot do. The mount lifecycle is
# still the harness's Invoke-WithMount, from -HarnessRoot.
#
# The flush matters. Windows writes through its cache, so without it the
# bytes would reach the driver only when the lazy writer got round to them,
# which may be after the mount is gone.

param(
    [Parameter(Mandatory=$true)] [string]$HarnessRoot,
    [Parameter(Mandatory=$true)] [string]$BinaryCmd,
    [Parameter(Mandatory=$true)] [string]$ReadyLine,
    [Parameter(Mandatory=$true)] [string]$Drive,
    [Parameter(Mandatory=$true)] [string]$Path,
    [Parameter(Mandatory=$true)] [int64]$Offset,
    [Parameter(Mandatory=$true)] [string]$Content,
    [Parameter(Mandatory=$true)] [ValidateSet('written', 'refused')] [string]$Expect
)

. (Join-Path $HarnessRoot 'scripts\vm\_lib.ps1')

$op = {
    param($DriveLetter)
    $target = Resolve-MountedPath -DriveLetter $DriveLetter -Path $Path
    if (-not (Test-Path -LiteralPath $target -PathType Leaf)) {
        throw "$target is not a file on the mount"
    }
    $bytes = New-Object System.Text.UTF8Encoding($false)
    $bytes = $bytes.GetBytes($Content)
    # A refusal has to come HERE, at the open: one that came later, from
    # the write or the flush, would reach an application after it had been
    # told its write succeeded.
    $stream = $null
    try {
        $stream = [System.IO.File]::Open(
            $target,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::Read)
    } catch {
        if ($Expect -eq 'refused') {
            Write-Output "refused at the open, as it must be, ${target}: $($_.Exception.Message)"
            return
        }
        throw
    }
    try {
        if ($Expect -eq 'refused') {
            throw "${target} was opened for writing, and the open must have been refused"
        }
        [void]$stream.Seek($Offset, [System.IO.SeekOrigin]::Begin)
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush($true)
    } finally {
        $stream.Dispose()
    }
    Write-Output "wrote $($bytes.Length) bytes to ${target} at $Offset"
}.GetNewClosure()
Invoke-WithMount -BinaryCmd $BinaryCmd -ReadyLine $ReadyLine -Drive $Drive -ScriptBlock $op
