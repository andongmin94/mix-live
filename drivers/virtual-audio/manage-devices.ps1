# Development-only device provisioning. DevGen is an installed WDK test tool, never distributed.
# The production installer is outside this script's scope. No test signing, trust-store
# changes, default endpoint changes, force removal, or automatic reboot.
[CmdletBinding(SupportsShouldProcess, ConfirmImpact = 'High')]
param(
    [ValidateSet('Status', 'Verify', 'Register', 'Remove')][string]$Action = 'Status',
    [ValidateSet('Call', 'Broadcast')][string]$Cable,
    [string]$PackageDirectory,
    [string]$SignTool = (Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe'),
    [string]$DevGen
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$hardware = [ordered]@{ Call = 'Root\MixLive_Call'; Broadcast = 'Root\MixLive_Broadcast' }
$pnputil = Join-Path $env:SystemRoot 'System32\pnputil.exe'

function Find-Cable([string]$Name) {
    $id = $hardware[$Name]
    # Compare exact hardware IDs, not friendly names, aliases or inferred endpoint IDs.
    # Win32_PnPEntity exposes HardwareID, Present, Service and ConfigManagerErrorCode.
    Get-CimInstance -ClassName Win32_PnPEntity | Where-Object {
        $_.PNPDeviceID -like 'ROOT\*' -and $_.HardwareID -contains $id
    }
}

function Read-Cable([string]$Name) {
    $devices = @(Find-Cable $Name)
    if ($devices.Count -eq 0) {
        return [pscustomobject]@{ Cable = $Name; HardwareId = $hardware[$Name]; State = 'Absent'; InstanceId = $null; ProblemCode = $null }
    }
    if ($devices.Count -gt 1) {
        return [pscustomobject]@{ Cable = $Name; HardwareId = $hardware[$Name]; State = 'Duplicate'; InstanceId = $null; ProblemCode = $null }
    }
    $device = $devices[0]
    $state = if ($device.Present -and $device.Service -eq 'MixLiveVirtualAudio' -and $device.ConfigManagerErrorCode -eq 0) {
        'Started'
    } else { 'NotReady' }
    [pscustomobject]@{ Cable = $Name; HardwareId = $hardware[$Name]; State = $state
        InstanceId = $device.PNPDeviceID; ProblemCode = $device.ConfigManagerErrorCode }
}

function Verify-Package {
    if (!$PackageDirectory) { throw 'Specify -PackageDirectory with the Microsoft-signed driver package.' }
    $package = (Resolve-Path -LiteralPath $PackageDirectory).Path
    $inf = Join-Path $package 'mixlive-virtual-audio.inf'
    $sys = Join-Path $package 'mixlive-virtual-audio.sys'
    $cat = Join-Path $package 'mixlive-virtual-audio.cat'
    foreach ($path in @($inf, $sys, $cat, $SignTool)) {
        if (!(Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing required file: $path" }
    }
    # Verify both the kernel signing policy and the catalog membership of INF and SYS.
    # PnP and Code Integrity remain the final authorities. No certificate is added.
    & $SignTool verify /kp /v $cat | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "Kernel-policy signature verification failed: catalog ($LASTEXITCODE)" }
    foreach ($member in @($inf, $sys)) {
        & $SignTool verify /kp /v /c $cat $member | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "Kernel-policy signature verification failed: $member ($LASTEXITCODE)" }
    }
    # This helper is not a generic installer for arbitrary INFs. Match our fixed
    # package identity and both hardware IDs before invoking a system tool.
    $content = Get-Content -LiteralPath $inf -Raw
    foreach ($pattern in @(
        '(?im)^\s*CatalogFile\s*=\s*mixlive-virtual-audio\.cat\s*$',
        '(?im)^\s*AddService\s*=\s*MixLiveVirtualAudio,',
        '(?im)^\s*%CallDevice%\s*=\s*Call,Root\\MixLive_Call\s*$',
        '(?im)^\s*%BroadcastDevice%\s*=\s*Broadcast,Root\\MixLive_Broadcast\s*$'
    )) {
        if ($content -notmatch $pattern) { throw 'The signed INF does not match the MixLive cable package identity.' }
    }
    return $inf
}

function Require-TestAdministrator {
    $os = Get-CimInstance -ClassName Win32_OperatingSystem
    if ($os.ProductType -ne 1 -or [int]$os.BuildNumber -lt 22000 -or
        [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne [System.Runtime.InteropServices.Architecture]::X64) {
        throw 'Device registration/removal requires a dedicated Windows 11 x64 client test machine.'
    }
    $principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    if (!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Run the development operation from an elevated PowerShell terminal.'
    }
}

function Invoke-Pnp([string[]]$Arguments) {
    & $pnputil @Arguments | Out-Host
    if ($LASTEXITCODE -eq 3010) { throw 'PnPUtil reports a restart is required. No automatic restart; inspect status before continuing.' }
    if ($LASTEXITCODE -ne 0) { throw "PnPUtil failed ($LASTEXITCODE); inspect device status. No automatic retry or rollback." }
}

if ($Action -eq 'Status') {
    foreach ($name in $hardware.Keys) { Read-Cable $name }
    return
}
if ($Action -eq 'Verify') {
    $null = Verify-Package
    Write-Output 'Catalog signature and INF/SYS membership pass kernel-policy verification. No installation performed.'
    return
}
if (!$Cable) { throw 'Select -Cable Call or -Cable Broadcast for a device operation.' }
Require-TestAdministrator
$current = Read-Cable $Cable
if ($current.State -eq 'Duplicate') { throw 'Multiple matching devices exist. Inspect exact instances manually; refusing bulk changes.' }

if ($Action -eq 'Register') {
    $inf = Verify-Package
    if ($current.State -ne 'Absent') { throw 'The requested cable already exists. Inspect Status or explicitly remove it first.' }
    if (!$DevGen -or !(Test-Path -LiteralPath $DevGen -PathType Leaf)) {
        throw 'Provide the locally installed Microsoft WDK devgen.exe via -DevGen. It is for testing only.'
    }
    if ((Get-Item -LiteralPath $DevGen).Name -ine 'devgen.exe') { throw 'Expected the WDK devgen.exe tool.' }
    if (!$PSCmdlet.ShouldProcess($hardware[$Cable], 'Stage verified package, create one root device and install its matching driver')) { return }
    Invoke-Pnp @('/add-driver', $inf)
    & $DevGen /add /bus ROOT /instanceid "MixLive_$Cable" /hardwareid $hardware[$Cable] | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "DevGen failed ($LASTEXITCODE); inspect Status before retrying." }
    Invoke-Pnp @('/add-driver', $inf, '/install')
    $result = Read-Cable $Cable
    $result
    if ($result.State -ne 'Started') { throw 'The cable is not confirmed started; inspect device status and Windows installation logs.' }
    return
}

if ($current.State -eq 'Absent') { $current; return }
if (!$PSCmdlet.ShouldProcess($current.InstanceId, 'Remove only the selected MixLive device instance')) { return }
Invoke-Pnp @('/remove-device', $current.InstanceId)
$result = Read-Cable $Cable
$result
if ($result.State -ne 'Absent') { throw 'Device removal has not been confirmed. The driver package is not deleted automatically.' }
