// Derived from Cableloom, Copyright (c) 2026 Cableloom contributors.
// Source revision 130f811310082fd6966b17e76f0ac212e129dbf9; see LICENSE in this directory.
#include "mixlive.hpp"
static VOID TimerCallback(PEX_TIMER timer, PVOID context) {
    UNREFERENCED_PARAMETER(timer); static_cast<Adapter*>(context)->tick();
}
Adapter::Adapter() { KeInitializeSpinLock(&lock); transport_.reset(); }
Adapter::~Adapter() { stop(); }
NTSTATUS Adapter::start() {
    timer_ = ExAllocateTimer(TimerCallback, this, EX_TIMER_HIGH_RESOLUTION);
    return timer_ ? STATUS_SUCCESS : STATUS_INSUFFICIENT_RESOURCES;
}
void Adapter::stop() {
    KIRQL irql;
    KeAcquireSpinLock(&lock, &irql);
    if (stopped_) { KeReleaseSpinLock(&lock, irql); return; }
    stopped_ = true;
    for (auto s : streams) if (s) s->state = KSSTATE_STOP;
    transport_.reset();
    auto timer = timer_;
    timer_ = nullptr;
    KeReleaseSpinLock(&lock, irql);
    // PASSIVE_LEVEL; drains the callback before the adapter or its buffers can be freed.
    if (timer) ExDeleteTimer(timer, TRUE, TRUE, nullptr);
}
NTSTATUS Adapter::state(Stream* s, KSSTATE next) {
    if (next < KSSTATE_STOP || next > KSSTATE_RUN) return STATUS_INVALID_PARAMETER;
    KIRQL irql; KeAcquireSpinLock(&lock, &irql);
    if (stopped_) {
        s->state = KSSTATE_STOP;
        KeReleaseSpinLock(&lock, irql);
        return next == KSSTATE_STOP ? STATUS_SUCCESS : STATUS_DEVICE_NOT_READY;
    }
    if (next == KSSTATE_RUN && (!s->buffer || !s->frames)) { KeReleaseSpinLock(&lock, irql); return STATUS_INVALID_DEVICE_STATE; }
    bool was_running = false;
    for (auto current : streams) if (current && current->state == KSSTATE_RUN) was_running = true;
    bool change = (s->state == KSSTATE_RUN) != (next == KSSTATE_RUN);
    // Attribute elapsed frames to the old RUN set before the new stream joins.
    if (change && was_running) advance_locked();
    s->state = next;
    if (next == KSSTATE_STOP) s->position = 0;
    // Flush queued audio at a RUN boundary; never replay a previous session.
    if (change) transport_.reset();
    bool running = false;
    for (auto current : streams) if (current && current->state == KSSTATE_RUN) running = true;
    if (running && !was_running) {
        LARGE_INTEGER frequency;
        auto now = KeQueryPerformanceCounter(&frequency);
        clock_.reset(now.QuadPart, frequency.QuadPart);
        ExSetTimer(timer_, -10000, 10000, nullptr);
    } else if (!running && was_running) ExCancelTimer(timer_, nullptr);
    KeReleaseSpinLock(&lock, irql);
    return STATUS_SUCCESS;
}
void Adapter::tick() {
    KIRQL irql; KeAcquireSpinLock(&lock, &irql);
    if (!stopped_) advance_locked();
    KeReleaseSpinLock(&lock, irql);
}

void Adapter::advance_locked() {
    auto render = streams[0], capture = streams[1];
    const bool r = render && render->state == KSSTATE_RUN && render->buffer && render->frames;
    const bool c = capture && capture->state == KSSTATE_RUN && capture->buffer && capture->frames;
    if (!r && !c) return;
    mixlive::Ring source{}, sink{};
    if (r) source = {render->buffer, render->frames, render->position};
    if (c) sink = {capture->buffer, capture->frames, capture->position};
    const auto elapsed = clock_.advance(KeQueryPerformanceCounter(nullptr).QuadPart);
    const auto rp = source.position, cp = sink.position;
    // A lost timer interval is a discontinuity, not permission to replay old audio.
    transport_.advance(elapsed, source, sink);
    if (r) { render->position = source.position; render->notify(rp); }
    if (c) { capture->position = sink.position; capture->notify(cp); }
}
