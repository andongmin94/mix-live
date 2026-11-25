# Validate a freshly built, unsigned driver package. Never sign or stage it.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$PackageDirectory,
    [string]$KitRoot = (Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Windows Kits\10'),
    [string]$KitVersion = '10.0.26100.0'
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$package = (Resolve-Path -LiteralPath $PackageDirectory).Path
$inf = Join-Path $package 'mixlive-virtual-audio.inf'
$sys = Join-Path $package 'mixlive-virtual-audio.sys'
$cat = Join-Path $package 'mixlive-virtual-audio.cat'
foreach ($path in @($inf, $sys, (Join-Path $package 'mixlive-virtual-audio.pdb'), (Join-Path $package 'LICENSE'))) {
    if (!(Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing package file: $path" }
}
if (Test-Path -LiteralPath $cat) { throw 'The catalog already exists; use a fresh unsigned build directory.' }

# The installed kit may place validation tools under bin or Tools. Require the
# requested version and architecture, not the newest executable on PATH.
$infverif = @(Get-ChildItem -LiteralPath $KitRoot -Recurse -File -Filter InfVerif.exe |
    Where-Object { $_.FullName -like "*\$KitVersion\x64\InfVerif.exe" })
if ($infverif.Count -ne 1) { throw "Expected one x64 InfVerif for WDK $KitVersion" }
$inf2cat = Join-Path $KitRoot "bin\$KitVersion\x86\Inf2Cat.exe"
if (!(Test-Path -LiteralPath $inf2cat -PathType Leaf)) { throw "Missing WDK Inf2Cat: $inf2cat" }
foreach ($mode in '/w', '/h') {
    & $infverif[0].FullName $mode $inf
    if ($LASTEXITCODE -ne 0) { throw "InfVerif $mode failed (exit $LASTEXITCODE)" }
}
# Catalog generation is not a signature. The 26100 kit checks the 21H2, 22H2/23H2,
# and 24H2 Windows 11 x64 families. Do not infer certification for other OS builds.
& $inf2cat "/driver:$package" '/os:10_CO_X64,10_NI_X64,10_GE_X64'
if ($LASTEXITCODE -ne 0 -or !(Test-Path -LiteralPath $cat -PathType Leaf)) {
    throw "Inf2Cat did not generate the unsigned catalog (exit $LASTEXITCODE)"
}
Write-Output "Unsigned package and catalog prepared: $package"
