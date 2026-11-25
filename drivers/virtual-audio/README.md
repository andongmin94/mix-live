# MixLive Virtual Audio (development source)

**Unsigned development driver. Do not deploy on a normal PC.**
This is a PortCls/WaveRT software cable, not the SysVAD tone generator.
The Rust app retains all mixing, DSP, VST3 processing, and device selection.

| Root hardware ID | Render interface label | Capture interface label |
|---|---|---|
| Root\MixLive_Call | MixLive Call Send | MixLive Call |
| Root\MixLive_Broadcast | MixLive Broadcast Send | MixLive Broadcast |
| Root\MixLive_Return | MixLive Return | MixLive Return Capture |

The INF declares three fixed hardware IDs. The development provisioning script can
create one root-enumerated device for each ID only after a signed-package preflight
and explicit administrator confirmation.
Actual Windows endpoint names and enumeration still need confirmation on a test
machine. The existing WASAPI device catalog selects the installed render ID; it
does not synthesize IDs or select devices by their friendly names. Use the
corresponding capture endpoint in the receiving application. No default device
is changed.

For external audio/DAW return, select the installed **MixLive Return** render
endpoint in the producing application, and its **MixLive Return Capture**
endpoint as the AUX input in MixLive (channels 1,2). Select physical headphones
for monitor, Call Send for the call mix, and Broadcast Send for the broadcast mix.
The Return cable itself does not play on physical headphones. Do not capture the
same producing application by process loopback as well: that would duplicate the
signal. Do not route MixLive's own output to Return or select a cable's capture
as its own source/output loop. Return is a separate audio route, not an
access-controlled private device; other authorized Windows audio clients may open it.
These are intended interface labels; actual endpoint labels and IDs are resolved
from Windows only after installation. The current Scene supports one physical
or virtual AUX endpoint in addition to process-captured apps.

## Transfer contract

- 48 kHz stereo interleaved integer PCM32 only; one WaveRT stream in each direction.
  Shared-mode conversion/mixing belongs to Windows, not to this kernel driver.
- An adapter owns one nonpaged ring-delay state, one QPC-derived frame clock and
  one high-resolution timer. Call, Broadcast and Return do not share audio.
- A 96-frame (2 ms) FIFO while both streams run; this is not end-to-end latency.
  No receiver: discard render data. No producer: produce zero capture frames.
  A RUN boundary flushes the FIFO. A missed full DMA wrap or interval over 480
  frames resets the FIFO and clears the capture buffer rather than replaying
  wrapped audio. Buffers are bounded to 4,800 frames.
- No custom IOCTL or control device, network, user-space DSP inside the kernel,
  automatic device creation, recording, or unsigned driver installation.

## Build and verify

Use a matching Windows SDK/WDK build 26100 and Visual Studio 2022 x64 C++ tools.
The path-scoped **Virtual Audio** CI uses the Windows Server 2022 runner's installed
VS 2022 and WDK/SDK build 26100 (the windows-2025 label now selects VS 2026).
It tests the transport, links the driver, runs InfVerif and generates an unsigned
catalog with Inf2Cat. It also checks that the unsigned package is rejected and that
all three cable IDs are absent. It never creates, loads or removes a device. Local build:

```powershell
cmake -S drivers/virtual-audio -B drivers/virtual-audio/build/transport
cmake --build drivers/virtual-audio/build/transport --config Release
ctest --test-dir drivers/virtual-audio/build/transport -C Release --output-on-failure
.\drivers\virtual-audio\build-driver.ps1 -WdkVersion 10.0.26100.0
.\drivers\virtual-audio\prepare-package.ps1 -PackageDirectory drivers\virtual-audio\build\package
```

The script also accepts explicit WdkRoot and SdkRoot when using extracted kits.
The build outputs the unsigned .sys, .inf, .pdb and MIT notice; the separate
package step creates the .cat. Use a fresh directory for each build that needs
a new catalog. Neither step signs, stages or installs anything. The generated
catalog is not a signed driver package. Windows 11 requires trusted driver signing;
Microsoft attestation is for testing scenarios and HLK/WHCP is the production
certification route. Do not disable security features. No signing account,
certificate or installation has been authorized in this work.

## Development device provisioning (not a production installer)

Once an approved Microsoft-signed package is available, use
`manage-devices.ps1 -Action Verify -PackageDirectory <signed-directory>` on the
test machine. SignTool checks the kernel policy and INF/SYS membership in the
catalog. A successful check is not proof that Windows will load the driver.

The default `-Action Status` is read-only. It checks only the three fixed hardware
IDs and reports absent, duplicate, started (matching service, present and no
PnP problem), or not ready. It does not assert that audio endpoints are present.
The `Register` action requires `-Cable Call`, `-Cable Broadcast` or `-Cable Return`,
`-PackageDirectory <signed-directory>`, and `-DevGen <installed-WDK-devgen.exe>`.
Use `-WhatIf` before an explicit confirmed action on an elevated Windows 11 x64
test machine. DevGen is a Microsoft development-only tool, cannot be redistributed,
and is not included in the package. PnPUtil stages the verified package and
installs the matching driver. The script checks actual PnP state afterwards;
errors or required restarts stop the operation without retrying or changing
system security. Existing/duplicate instances are rejected rather than replaced.
`Remove` requires the selected cable and removes only its exact discovered
instance ID after confirmation. The staged driver package remains in the driver
store; package removal/upgrade and a production installer are not implemented.

## Acceptance not yet met

Build-only CI cannot prove PortCls enumeration, shared-mode format negotiation,
kernel lifetime/IRQL correctness, power transitions, driver removal, HVCI or
Discord/OBS reception or DAW/Return capture. Validate these on an isolated Windows 11 test target
with an approved Microsoft-signed package and Driver Verifier/HLK as appropriate.
A successful transport test does not certify the kernel driver.

PortCls/WaveRT code is derived from Cableloom revision
`130f811310082fd6966b17e76f0ac212e129dbf9`; upstream also reports that driver
loading/live audio are unverified. See LICENSE and repository NOTICE.md.
References: [Microsoft SysVAD limitations](https://learn.microsoft.com/en-us/samples/microsoft/windows-driver-samples/sysvad-virtual-audio-device-driver-sample/),
[signing options](https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/driver-signing-offerings),
[InfVerif](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/infverif).
