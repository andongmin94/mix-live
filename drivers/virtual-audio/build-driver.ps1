# Derived from Cableloom (MIT); see LICENSE.
param(
    [string]$WdkRoot = (Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Windows Kits\10'),
    [ValidatePattern('^\d+\.\d+\.\d+\.\d+$')][string]$WdkVersion = '10.0.26100.0',
    [string]$SdkRoot = $WdkRoot,
    [ValidatePattern('^\d+\.\d+\.\d+\.\d+$')][string]$SdkVersion = $WdkVersion,
    [string]$Output = (Join-Path $PSScriptRoot 'build\package')
)
$ErrorActionPreference = 'Stop'
$km = Join-Path $WdkRoot "Include\$WdkVersion\km"
$lib = Join-Path $WdkRoot "Lib\$WdkVersion\km\x64"
foreach ($item in @((Join-Path $km 'portcls.h'), (Join-Path $lib 'portcls.lib'), (Join-Path $lib 'BufferOverflowFastFailK.lib'))) {
    if (-not (Test-Path -LiteralPath $item)) { throw "Required WDK file is missing: $item. Install a matching Windows SDK/WDK first." }
}
foreach ($header in 'shared\ks.h','ucrt\corecrt.h') {
    $item = Join-Path $SdkRoot "Include\$SdkVersion\$header"
    if (-not (Test-Path -LiteralPath $item)) { throw "Required SDK file is missing: $item. Set SdkRoot and SdkVersion to the matching SDK." }
}
if (([version]$SdkVersion).Build -ne ([version]$WdkVersion).Build) { throw 'SDK and WDK build numbers must match.' }
$vswhere = Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Microsoft Visual Studio\Installer\vswhere.exe'
$vsRange = if (([version]$WdkVersion).Build -ge 28000) { '[18.0,19.0)' } else { '[17.0,18.0)' }
$vs = & $vswhere -latest -version $vsRange -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw "A compatible Visual Studio C++ toolset is required (range $vsRange)." }
$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
$environment = & cmd.exe /d /s /c ('"' + $vcvars + '" >nul && set')
if ($LASTEXITCODE -ne 0) { throw 'Cannot initialize the Visual C++ environment.' }
foreach ($line in $environment) {
    if ($line -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process') }
}
New-Item -ItemType Directory -Path $Output -Force | Out-Null
$Output = (Resolve-Path -LiteralPath $Output).Path
if (Test-Path -LiteralPath (Join-Path $Output 'mixlive-virtual-audio.cat')) {
    throw 'Use a fresh output directory; rebuilding would invalidate an existing catalog.'
}
$common = @('/nologo','/c','/std:c++17','/utf-8','/kernel','/GR-','/EHs-c-','/Zl','/Zc:throwingNew-','/Zc:threadSafeInit-',
    '/W4','/GS','/Oi','/O2','/Zi','/D_AMD64_','/DAMD64','/D_WIN64',
    '/DNTDDI_VERSION=0x0A000008','/D_WIN32_WINNT=0x0A00','/DWINVER=0x0A00',
    "/I$km", "/I$(Join-Path $SdkRoot "Include\$SdkVersion\shared")", "/I$(Join-Path $SdkRoot "Include\$SdkVersion\ucrt")")
$objects = @()
foreach ($name in 'driver','adapter','miniport','stream') {
    $object = Join-Path $Output "$name.obj"
    & cl.exe @common (Join-Path $PSScriptRoot "$name.cpp") "/Fo$object"
    if ($LASTEXITCODE -ne 0) { throw "C++ compilation failed: $name" }
    $objects += $object
}
& link.exe /NOLOGO /DRIVER /SUBSYSTEM:NATIVE,10.00 /MACHINE:X64 /ENTRY:GsDriverEntry /NODEFAULTLIB /MANIFEST:NO /INTEGRITYCHECK /DYNAMICBASE /NXCOMPAT /OPT:REF /OPT:ICF /DEBUG:FULL "/PDBALTPATH:%_PDB%" "/OUT:$Output\mixlive-virtual-audio.sys" "/PDB:$Output\mixlive-virtual-audio.pdb" "/LIBPATH:$lib" @objects ntoskrnl.lib hal.lib portcls.lib BufferOverflowFastFailK.lib
if ($LASTEXITCODE -ne 0) { throw 'Kernel driver link failed.' }
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'mixlive-virtual-audio.inf') -Destination $Output
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'LICENSE') -Destination $Output
# Object files are build intermediates, not part of the unsigned package.
foreach ($object in $objects) { Remove-Item -LiteralPath $object }
Write-Output "Unsigned driver built in $Output. Run prepare-package.ps1 before submitting for Microsoft signing."
