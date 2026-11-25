// Derived from Cableloom, Copyright (c) 2026 Cableloom contributors.
// Source revision 130f811310082fd6966b17e76f0ac212e129dbf9; see LICENSE in this directory.
#define INITGUID
#include "mixlive.hpp"
#include <stddef.h>
extern void InitializeDescriptors();
static constexpr ULONG tag = 'vLxM';
void* __cdecl operator new(size_t bytes) { return ExAllocatePool2(POOL_FLAG_NON_PAGED, bytes, tag); }
void __cdecl operator delete(void* memory) noexcept { if (memory) ExFreePoolWithTag(memory, tag); }
void __cdecl operator delete(void* memory, size_t) noexcept { if (memory) ExFreePoolWithTag(memory, tag); }
struct Extension {
    ULONG_PTR portcls[PORT_CLASS_DEVICE_EXTENSION_SIZE / sizeof(ULONG_PTR)];
    Adapter* adapter;
};
static_assert(offsetof(Extension, adapter) == PORT_CLASS_DEVICE_EXTENSION_SIZE, "PortCls extension prefix");
static NTSTATUS RegisterPort(PDEVICE_OBJECT device, PIRP irp, PRESOURCELIST resources,
    PWSTR name, REFCLSID clsid, PUNKNOWN miniport, PPORT* result) {
    *result = nullptr;
    PPORT port = nullptr;
    NTSTATUS status = PcNewPort(&port, clsid);
    if (NT_SUCCESS(status)) status = port->Init(device, irp, miniport, nullptr, resources);
    if (NT_SUCCESS(status)) status = PcRegisterSubdevice(device, name, port);
    if (NT_SUCCESS(status)) *result = port;
    else if (port) port->Release();
    return status;
}
static NTSTATUS StartDevice(PDEVICE_OBJECT device, PIRP irp, PRESOURCELIST resources) {
    auto extension = static_cast<Extension*>(device->DeviceExtension);
    if (extension->adapter) return STATUS_INVALID_DEVICE_STATE;
    auto adapter = new Adapter();
    if (!adapter) return STATUS_INSUFFICIENT_RESOURCES;
    NTSTATUS status = adapter->start();
    PPORT ports[4]{};
    PUNKNOWN miniports[4]{};
    if (NT_SUCCESS(status)) {
        miniports[0] = static_cast<IMiniportWaveRT*>(new Wave(adapter, false));
        miniports[1] = static_cast<IMiniportWaveRT*>(new Wave(adapter, true));
        miniports[2] = static_cast<IMiniportTopology*>(new Topology(false));
        miniports[3] = static_cast<IMiniportTopology*>(new Topology(true));
        for (auto m : miniports) if (!m) status = STATUS_INSUFFICIENT_RESOURCES;
    }
    WCHAR render_wave[] = L"RenderWave";
    WCHAR capture_wave[] = L"CaptureWave";
    WCHAR render_topology[] = L"RenderTopology";
    WCHAR capture_topology[] = L"CaptureTopology";
    PWSTR names[] = { render_wave, capture_wave, render_topology, capture_topology };
    for (ULONG i = 0; i < 4 && NT_SUCCESS(status); ++i)
        status = RegisterPort(device, irp, resources, names[i], i < 2 ? CLSID_PortWaveRT : CLSID_PortTopology, miniports[i], &ports[i]);
    if (NT_SUCCESS(status)) status = PcRegisterPhysicalConnection(device, ports[0], 1, ports[2], 0);
    if (NT_SUCCESS(status)) status = PcRegisterPhysicalConnection(device, ports[3], 1, ports[1], 0);
    for (auto m : miniports) if (m) m->Release();
    for (auto p : ports) if (p) p->Release();
    if (NT_SUCCESS(status)) extension->adapter = adapter;
    if (!NT_SUCCESS(status)) { adapter->stop(); adapter->release(); }
    return status;
}
static NTSTATUS AddDevice(PDRIVER_OBJECT driver, PDEVICE_OBJECT pdo) {
    return PcAddAdapterDevice(driver, pdo, StartDevice, 4, sizeof(Extension));
}
static NTSTATUS Pnp(PDEVICE_OBJECT device, PIRP irp) {
    UCHAR minor = IoGetCurrentIrpStackLocation(irp)->MinorFunction;
    if (minor == IRP_MN_STOP_DEVICE || minor == IRP_MN_REMOVE_DEVICE || minor == IRP_MN_SURPRISE_REMOVAL) {
        auto extension = static_cast<Extension*>(device->DeviceExtension);
        auto adapter = extension->adapter;
        extension->adapter = nullptr;
        if (adapter) { adapter->stop(); adapter->release(); }
    }
    return PcDispatchIrp(device, irp);
}
extern "C" NTSTATUS DriverEntry(PDRIVER_OBJECT driver, PUNICODE_STRING path) {
    InitializeDescriptors();
    NTSTATUS status = PcInitializeAdapterDriver(driver, path, AddDevice);
    if (!NT_SUCCESS(status)) return status;
    // No custom control device or user-supplied IOCTL. PortCls owns standard audio I/O.
    driver->MajorFunction[IRP_MJ_PNP] = Pnp;
    return STATUS_SUCCESS;
}
