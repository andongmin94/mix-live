# MixLive Virtual Audio (development source)

**Unsigned development driver. Do not deploy on a normal PC.**
This is a PortCls/WaveRT software cable, not the SysVAD tone generator.
The Rust app retains all mixing, DSP, VST3 processing, and device selection.

| Root hardware ID | Render interface label | Capture interface label |
|---|---|---|
| Root\MixLive_Call | MixLive Call Send | MixLive Call |
| Root\MixLive_Broadcast | MixLive Broadcast Send | MixLive Broadcast |

The INF declares two fixed hardware IDs. A separately approved provisioning step
must create one root-enumerated device for each ID using a correctly signed package.
Actual Windows endpoint names and enumeration still need confirmation on a test
machine. The existing WASAPI device catalog selects the installed render ID; it
does not synthesize IDs or select devices by their friendly names. Use the
corresponding capture endpoint in the receiving application. No default device
is changed. The Return cable is outside this slice.

## Transfer contract

- 48 kHz stereo interleaved integer PCM32 only; one WaveRT stream in each direction.
  Shared-mode conversion/mixing belongs to Windows, not to this kernel driver.
- An adapter owns one nonpaged ring-delay state, one QPC-derived frame clock and
  one high-resolution timer. Call and Broadcast do not share audio.
- A 96-frame (2 ms) FIFO while both streams run; this is not end-to-end latency.
  No receiver: discard render data. No producer: produce zero capture frames.
  A RUN boundary flushes the FIFO. A missed full DMA wrap or interval over 480
  frames resets the FIFO and clears the capture buffer rather than replaying
  wrapped audio. Buffers are bounded to 4,800 frames.
- No custom IOCTL or control device, network, user-space DSP inside the kernel,
  automatic device creation, recording, or unsigned driver installation.

## Build and verify

Use a matching Windows SDK/WDK build 26100 and Visual Studio 2022 x64 C++ tools.
The path-scoped **Virtual Audio** CI downloads the pinned Microsoft WDK package
10.0.26100.6584 and checks the actual transport, compiles the driver, and checks
the INF with InfVerif. It does not load or install a driver. Local build:

```powershell
cmake -S drivers/virtual-audio -B drivers/virtual-audio/build/transport
cmake --build drivers/virtual-audio/build/transport --config Release
ctest --test-dir drivers/virtual-audio/build/transport -C Release --output-on-failure
.\drivers\virtual-audio\build-driver.ps1 -WdkVersion 10.0.26100.0
```

The script also accepts explicit WdkRoot and SdkRoot when using extracted kits.
It outputs the unsigned .sys, .inf, .pdb and MIT notice. It does not generate a
catalog or sign, stage, install or remove a device. WDK tools must validate the
INF and generate a catalog before a signing submission. Windows 11 requires
trusted driver signing; Microsoft attestation is for testing scenarios and
HLK/WHCP is the production certification route. Do not disable security features.
No signing account, certificate or installation has been authorized in this work.

## Acceptance not yet met

Build-only CI cannot prove PortCls enumeration, shared-mode format negotiation,
kernel lifetime/IRQL correctness, power transitions, driver removal, HVCI or
Discord/OBS reception. Validate these on an isolated Windows 11 test target
with an approved Microsoft-signed package and Driver Verifier/HLK as appropriate.
A successful transport test does not certify the kernel driver.

PortCls/WaveRT code is derived from Cableloom revision
`130f811310082fd6966b17e76f0ac212e129dbf9`; upstream also reports that driver
loading/live audio are unverified. See LICENSE and repository NOTICE.md.
References: [Microsoft SysVAD limitations](https://learn.microsoft.com/en-us/samples/microsoft/windows-driver-samples/sysvad-virtual-audio-device-driver-sample/),
[signing options](https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/driver-signing-offerings),
[InfVerif](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/infverif).
