<#
.SYNOPSIS
Copy the Microsoft Visual C++ 2015-2022 x64 runtime libraries into the
directory that tauri.conf.json bundles as an application resource.

.DESCRIPTION
The ONNX Runtime the application downloads at first launch imports
MSVCP140.dll, MSVCP140_1.dll, VCRUNTIME140.dll and VCRUNTIME140_1.dll in its
ordinary import table. A clean Windows installation has none of them: they
come with the "Visual C++ 2015-2022 Redistributable", which is not installed
unless something else put it there. Without them the runtime cannot be loaded
at all (Windows error 126, "the specified module could not be found") and the
application has nothing to run models on.

Rather than make the user install the redistributable, or run Microsoft's
bootstrapper from an installer hook (which wants a network connection at
install time and administrator rights that a per-user install does not take),
the files are deployed app-local: copied into the application directory next
to the executable, which is the first directory in the Windows standard DLL
search order and therefore the one the ONNX Runtime's imports resolve from.
Microsoft's Visual Studio distributable code terms permit this deployment, and
the files are taken from the Redist tree of the Visual Studio installation on
this machine rather than from anywhere else.

x64 only, which is the only Windows architecture the project builds. See the
note in src-tauri/vendor/windows-crt/ for the rest of the reasoning, and
.github/workflows/ci.yml for the job that proves this script still resolves a
directory on the current runner image.

.PARAMETER Destination
The directory to copy into. The release workflow passes
src-tauri/vendor/windows-crt, which tauri.conf.json bundles with the
"vendor/windows-crt/*" resource glob.

.PARAMETER VerifyOnly
Resolve and report the redistributable directory without copying anything.
Used by CI to catch a runner image that has moved or dropped the files, on
every pull request rather than during a tagged release.
#>
param(
    [string] $Destination,
    [switch] $VerifyOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if (-not $VerifyOnly -and [string]::IsNullOrWhiteSpace($Destination)) {
    throw 'stage-vc-runtime.ps1: -Destination is required unless -VerifyOnly is passed.'
}

# The four libraries the ONNX Runtime import table actually names. Everything
# else in the redistributable folder is copied too (it is small, and the set
# is self-consistent), but a missing one of these four is a hard failure: it
# means the folder this script found is not the redistributable folder.
$required = @(
    'msvcp140.dll',
    'msvcp140_1.dll',
    'vcruntime140.dll',
    'vcruntime140_1.dll'
)

# vswhere is installed with every Visual Studio since 2017 and lives at a fixed
# path, which is the documented way to find an installation without guessing
# the edition. Guessing is what breaks when a runner image moves from
# Enterprise to Community.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere)) {
    throw "stage-vc-runtime.ps1: vswhere.exe not found at $vswhere; no Visual Studio installation to take the redistributable from."
}

$vsRoot = (& $vswhere -latest -products * -property installationPath) | Select-Object -First 1
if ([string]::IsNullOrWhiteSpace($vsRoot)) {
    throw 'stage-vc-runtime.ps1: vswhere reported no Visual Studio installation.'
}

# VC\Redist\MSVC\<toolset version>\x64\Microsoft.VC<nnn>.CRT. Both the toolset
# version and the VC<nnn> number change with the compiler, so neither is
# written down here; the newest match wins.
$redistRoot = Join-Path $vsRoot 'VC\Redist\MSVC'
if (-not (Test-Path -LiteralPath $redistRoot)) {
    throw "stage-vc-runtime.ps1: no redistributable tree at $redistRoot; the Visual Studio installation has no 'C++ Redistributable MSMs/MSIs' component."
}

$crt = Get-ChildItem -LiteralPath $redistRoot -Directory |
    ForEach-Object { Join-Path $_.FullName 'x64' } |
    Where-Object { Test-Path -LiteralPath $_ } |
    ForEach-Object { Get-ChildItem -LiteralPath $_ -Directory -Filter 'Microsoft.VC*.CRT' } |
    Sort-Object -Property FullName |
    Select-Object -Last 1

if ($null -eq $crt) {
    throw "stage-vc-runtime.ps1: no Microsoft.VC*.CRT directory under $redistRoot."
}

Write-Host "Visual C++ redistributable: $($crt.FullName)"

$missing = $required | Where-Object { -not (Test-Path -LiteralPath (Join-Path $crt.FullName $_)) }
if ($missing) {
    throw "stage-vc-runtime.ps1: $($crt.FullName) is missing $($missing -join ', '); it is not the runtime redistributable this build needs."
}

Get-ChildItem -LiteralPath $crt.FullName -Filter '*.dll' |
    ForEach-Object { Write-Host ("  {0,-32} {1,9:N0} bytes" -f $_.Name, $_.Length) }

if ($VerifyOnly) {
    Write-Host 'Verify only: nothing copied.'
    exit 0
}

New-Item -ItemType Directory -Force -Path $Destination | Out-Null
Copy-Item -Path (Join-Path $crt.FullName '*.dll') -Destination $Destination -Force

# Copying is not proof: an antivirus hook or a locked file can drop one
# silently, and the failure would then only appear on a user's machine as a
# runtime that will not load.
$stillMissing = $required | Where-Object { -not (Test-Path -LiteralPath (Join-Path $Destination $_)) }
if ($stillMissing) {
    throw "stage-vc-runtime.ps1: $($stillMissing -join ', ') did not arrive in $Destination."
}

Write-Host "Staged the Visual C++ runtime into $Destination"
