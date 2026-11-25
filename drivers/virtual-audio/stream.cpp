// Derived from Cableloom, Copyright (c) 2026 Cableloom contributors.
// Source revision 130f811310082fd6966b17e76f0ac212e129dbf9; see LICENSE in this directory.
#include "mixlive.hpp"
Stream::Stream(Adapter* owner, PPORTWAVERTSTREAM p, bool input) : adapter(owner), port(p), capture(input) {
    adapter->add_ref();
    port->AddRef();
}
Stream::~Stream() {
    adapter->state(this, KSSTATE_STOP);
    KIRQL irql;
    KeAcquireSpinLock(&adapter->lock, &irql);
    if (adapter->streams[capture ? 1 : 0] == this) adapter->streams[capture ? 1 : 0] = nullptr;
    KeReleaseSpinLock(&adapter->lock, irql);
    if (mdl) FreeAudioBuffer(mdl, frames * mixlive::frame_bytes);
    port->Release();
    adapter->release();
}
STDMETHODIMP Stream::QueryInterface(REFIID iid, PVOID* out) {
    if (!out) return STATUS_INVALID_PARAMETER;
    *out = nullptr;
    if (IsEqualGUIDAligned(iid, IID_IUnknown) || IsEqualGUIDAligned(iid, IID_IMiniportWaveRTStream) ||
        IsEqualGUIDAligned(iid, IID_IMiniportWaveRTStreamNotification)) {
        *out = static_cast<IMiniportWaveRTStreamNotification*>(this); AddRef(); return STATUS_SUCCESS;
    }
    return STATUS_NOINTERFACE;
}
NTSTATUS Stream::allocate(ULONG count, ULONG requested, PMDL* out, ULONG* actual, ULONG* offset, MEMORY_CACHING_TYPE* cache) {
    if (!out || !actual || !offset || !cache) return STATUS_INVALID_PARAMETER;
    *out = nullptr; *actual = *offset = 0;
    if (count > 8 || requested < 2 * mixlive::frame_bytes || requested > mixlive::max_buffer_frames * mixlive::frame_bytes ||
        requested % mixlive::frame_bytes || (count && requested % (count * mixlive::frame_bytes)))
        return STATUS_INVALID_PARAMETER;
    KIRQL irql;
    KeAcquireSpinLock(&adapter->lock, &irql);
    bool busy = mdl || state == KSSTATE_RUN;
    KeReleaseSpinLock(&adapter->lock, irql);
    if (busy) return STATUS_DEVICE_BUSY;
    PHYSICAL_ADDRESS high; high.QuadPart = MAXLONGLONG;
    PMDL pages = port->AllocatePagesForMdl(high, requested);
    if (!pages) return STATUS_INSUFFICIENT_RESOURCES;
    if (MmGetMdlByteCount(pages) < requested) { port->FreePagesFromMdl(pages); return STATUS_INSUFFICIENT_RESOURCES; }
    auto mapping = static_cast<BYTE*>(port->MapAllocatedPages(pages, MmCached));
    if (!mapping) { port->FreePagesFromMdl(pages); return STATUS_INSUFFICIENT_RESOURCES; }
    RtlZeroMemory(mapping, requested);
    KeAcquireSpinLock(&adapter->lock, &irql);
    if (mdl || state == KSSTATE_RUN) {
        KeReleaseSpinLock(&adapter->lock, irql);
        port->UnmapAllocatedPages(mapping, pages); port->FreePagesFromMdl(pages); return STATUS_DEVICE_BUSY;
    }
    mdl = pages; buffer = mapping; frames = requested / mixlive::frame_bytes; notifications = count; position = 0;
    KeReleaseSpinLock(&adapter->lock, irql);
    *out = pages; *actual = requested; *cache = MmCached;
    return STATUS_SUCCESS;
}
STDMETHODIMP Stream::AllocateAudioBuffer(ULONG requested, PMDL* out, ULONG* actual, ULONG* offset, MEMORY_CACHING_TYPE* cache) {
    return allocate(0, requested, out, actual, offset, cache);
}
STDMETHODIMP Stream::AllocateBufferWithNotification(ULONG count, ULONG requested, PMDL* out, ULONG* actual, ULONG* offset, MEMORY_CACHING_TYPE* cache) {
    if (!count) return STATUS_INVALID_PARAMETER;
    return allocate(count, requested, out, actual, offset, cache);
}
STDMETHODIMP_(VOID) Stream::FreeAudioBuffer(PMDL pages, ULONG size) {
    UNREFERENCED_PARAMETER(size);
    adapter->state(this, KSSTATE_STOP);
    KIRQL irql;
    KeAcquireSpinLock(&adapter->lock, &irql);
    if (!pages || pages != mdl) { KeReleaseSpinLock(&adapter->lock, irql); return; }
    auto old = buffer;
    buffer = nullptr; mdl = nullptr; frames = notifications = 0; position = 0;
    KeReleaseSpinLock(&adapter->lock, irql);
    port->UnmapAllocatedPages(old, pages); port->FreePagesFromMdl(pages);
}
STDMETHODIMP_(VOID) Stream::FreeBufferWithNotification(PMDL pages, ULONG size) { FreeAudioBuffer(pages, size); }
STDMETHODIMP Stream::SetFormat(PKSDATAFORMAT format) {
    if (!ValidFormat(format)) return STATUS_NO_MATCH;
    KIRQL irql; KeAcquireSpinLock(&adapter->lock, &irql);
    bool running = state == KSSTATE_RUN;
    KeReleaseSpinLock(&adapter->lock, irql);
    return running ? STATUS_DEVICE_BUSY : STATUS_SUCCESS;
}
STDMETHODIMP Stream::SetState(KSSTATE next) { return adapter->state(this, next); }
STDMETHODIMP Stream::GetPosition(PKSAUDIO_POSITION result) {
    if (!result) return STATUS_INVALID_PARAMETER;
    KIRQL irql; KeAcquireSpinLock(&adapter->lock, &irql);
    result->PlayOffset = frames ? (position % frames) * mixlive::frame_bytes : 0;
    result->WriteOffset = result->PlayOffset;
    KeReleaseSpinLock(&adapter->lock, irql);
    return STATUS_SUCCESS;
}
STDMETHODIMP_(VOID) Stream::GetHWLatency(KSRTAUDIO_HWLATENCY* result) {
    if (!result) return;
    RtlZeroMemory(result, sizeof(*result));
    // Only the fixed transport FIFO. DMA/OS/timer latency is separate.
    result->FifoSize = capture ? mixlive::delay_frames * mixlive::frame_bytes : 0;
}
STDMETHODIMP Stream::GetPositionRegister(KSRTAUDIO_HWREGISTER* reg) {
    UNREFERENCED_PARAMETER(reg); return STATUS_NOT_SUPPORTED;
}
STDMETHODIMP Stream::GetClockRegister(KSRTAUDIO_HWREGISTER* reg) {
    UNREFERENCED_PARAMETER(reg); return STATUS_NOT_SUPPORTED;
}
STDMETHODIMP Stream::RegisterNotificationEvent(PKEVENT event) {
    if (!event) return STATUS_INVALID_PARAMETER;
    KIRQL irql; KeAcquireSpinLock(&adapter->lock, &irql);
    for (auto existing : events) if (existing == event) { KeReleaseSpinLock(&adapter->lock, irql); return STATUS_OBJECT_NAME_COLLISION; }
    for (auto& slot : events) if (!slot) { slot = event; KeReleaseSpinLock(&adapter->lock, irql); return STATUS_SUCCESS; }
    KeReleaseSpinLock(&adapter->lock, irql); return STATUS_INSUFFICIENT_RESOURCES;
}
STDMETHODIMP Stream::UnregisterNotificationEvent(PKEVENT event) {
    KIRQL irql; KeAcquireSpinLock(&adapter->lock, &irql);
    for (auto& slot : events) if (slot == event && event) { slot = nullptr; KeReleaseSpinLock(&adapter->lock, irql); return STATUS_SUCCESS; }
    KeReleaseSpinLock(&adapter->lock, irql); return STATUS_NOT_FOUND;
}
void Stream::notify(uint64_t previous) {
    if (!notifications || !frames) return;
    ULONG quantum = frames / notifications;
    if (position / quantum != previous / quantum)
        for (auto event : events) if (event) KeSetEvent(event, IO_NO_INCREMENT, FALSE);
}
