<#
.SYNOPSIS
    Install frms: copy files into the install dir and create Start Menu / Desktop
    shortcuts. frms is a desktop GUI app, so it is NOT registered as a service
    (unlike TSR Prism).

.DESCRIPTION
    The deploy zip unpacks flat (frms.exe, README.md, _scripts\) with this script
    sitting in _scripts\. Running it from there copies frms.exe into the install
    dir and creates shortcuts.

    - Idempotent: safe to re-run to upgrade. Stops a running frms first so the
      .exe isn't locked during the copy.
    - All-users install (default) goes to "C:\Program Files\frms" and needs
      Administrator. Use -PerUser for a no-admin install under
      %LOCALAPPDATA%\Programs\frms.
    - frms drives its AI agents through the Claude Code CLI ("claude"); if it
      isn't on PATH the installer prints how to get it (the app also prompts).

.EXAMPLE
    # All-users (run in an elevated PowerShell):
    powershell -ExecutionPolicy Bypass -File .\_scripts\install-frms.ps1

.EXAMPLE
    # Per-user, no admin, also add to PATH:
    powershell -ExecutionPolicy Bypass -File .\_scripts\install-frms.ps1 -PerUser -AddToPath
#>
[CmdletBinding()]
param(
    # Unzip root = parent of this script's _scripts dir. Resolved below if not
    # passed; $PSScriptRoot is unreliable inside param() defaults on PS 5.1.
    [string]$Source,
    [switch]$PerUser,             # install under %LOCALAPPDATA% (no admin needed)
    [switch]$NoDesktopShortcut,   # skip the Desktop shortcut
    [switch]$AddToPath            # add the install dir to PATH
)

$ErrorActionPreference = "Stop"

$AppName = "frms"
$ExeName = "frms.exe"

if (-not $Source) {
    $scriptDir = $PSScriptRoot
    if (-not $scriptDir) { $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path }
    $Source = Split-Path $scriptDir -Parent
}

if (-not (Test-Path (Join-Path $Source $ExeName))) {
    Write-Error "$ExeName not found in source '$Source'. Run this from the unzipped frms folder."
    exit 1
}

# ---- Resolve install location + shortcut folders ----------------------------
$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if ($PerUser) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\$AppName"
    $StartMenu  = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
    $DesktopDir = [Environment]::GetFolderPath("Desktop")
    $PathScope  = "User"
} else {
    if (-not $isAdmin) {
        Write-Error "All-users install needs Administrator. Re-run elevated, or use -PerUser."
        exit 1
    }
    $InstallDir = Join-Path $env:ProgramFiles $AppName
    $StartMenu  = Join-Path $env:ProgramData "Microsoft\Windows\Start Menu\Programs"
    $DesktopDir = Join-Path $env:PUBLIC "Desktop"
    $PathScope  = "Machine"
}

Write-Host "frms installer" -ForegroundColor Cyan
Write-Host "==================================" -ForegroundColor Cyan
Write-Host "Source     : $Source"
Write-Host "Install dir: $InstallDir"

# ---- 1. Stop a running frms so its exe isn't locked -------------------------
Get-Process -Name $AppName -ErrorAction SilentlyContinue | ForEach-Object {
    Write-Host "Stopping running $AppName (PID $($_.Id))" -ForegroundColor Yellow
    Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
}

# ---- 2. Lay down files (exclude this _scripts dir from the install) ----------
if (-not (Test-Path $InstallDir)) { New-Item -Path $InstallDir -ItemType Directory -Force | Out-Null }
Write-Host "Copying files into $InstallDir ..."
# robocopy: /E recurse incl. empty dirs. Exit codes 0-7 are success (8+ = error).
& robocopy $Source $InstallDir /E /XD "_scripts" /NFL /NDL /NJH /NJS /NP | Out-Null
if ($LASTEXITCODE -ge 8) {
    Write-Error "robocopy failed with exit code $LASTEXITCODE"
    exit 1
}

$ExePath = Join-Path $InstallDir $ExeName

# ---- 3. Shortcuts (Start Menu always; Desktop unless suppressed) ------------
function New-AppShortcut([string]$LnkPath, [string]$Target) {
    $ws = New-Object -ComObject WScript.Shell
    $sc = $ws.CreateShortcut($LnkPath)
    $sc.TargetPath       = $Target
    $sc.WorkingDirectory = Split-Path $Target -Parent
    $sc.IconLocation     = $Target   # frms.exe has no embedded .ico yet → generic icon
    $sc.Description       = "frms IDE"
    $sc.Save()
}

if (-not (Test-Path $StartMenu)) { New-Item -Path $StartMenu -ItemType Directory -Force | Out-Null }
New-AppShortcut (Join-Path $StartMenu "$AppName.lnk") $ExePath
Write-Host "Start Menu shortcut created."

if (-not $NoDesktopShortcut) {
    New-AppShortcut (Join-Path $DesktopDir "$AppName.lnk") $ExePath
    Write-Host "Desktop shortcut created."
}

# ---- 4. Optional: add to PATH -----------------------------------------------
if ($AddToPath) {
    $cur = [Environment]::GetEnvironmentVariable("Path", $PathScope)
    if ($cur -notlike "*$InstallDir*") {
        $new = ($cur.TrimEnd(';') + ";" + $InstallDir)
        [Environment]::SetEnvironmentVariable("Path", $new, $PathScope)
        Write-Host "Added $InstallDir to $PathScope PATH (restart terminals to pick it up)."
    } else {
        Write-Host "$InstallDir already on $PathScope PATH."
    }
}

Write-Host "`n$AppName installed to $ExePath" -ForegroundColor Green

# ---- 5. Claude Code CLI check (frms needs it to run agents) ------------------
$claude = Get-Command claude -ErrorAction SilentlyContinue
if (-not $claude) {
    Write-Warning "Claude Code CLI ('claude') was not found on PATH."
    Write-Host    "  frms needs it to run AI agents. Install Node.js (https://nodejs.org),"
    Write-Host    "  then:  npm install -g @anthropic-ai/claude-code"
    Write-Host    "  then run 'claude' once to sign in (a Claude Pro/Max subscription)."
} else {
    Write-Host "Claude Code CLI found: $($claude.Source)" -ForegroundColor Green
}

Write-Host "`nLaunch frms from the Start Menu or Desktop shortcut." -ForegroundColor Cyan
