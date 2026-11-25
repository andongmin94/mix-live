// Derived from Cableloom, Copyright (c) 2026 Cableloom contributors.
// Source revision 130f811310082fd6966b17e76f0ac212e129dbf9; see LICENSE in this directory.
#pragma once
#include <ntddk.h>
#include <portcls.h>
#include <ks.h>
#include <ksmedia.h>
#include "core.hpp"

class Stream;
class Adapter;
#define MIXLIVE_UNKNOWN \
    STDMETHODIMP QueryInterface(REFIID iid, PVOID* object) override; \
    STDMETHODIMP_(ULONG) AddRef() override { return static_cast<ULONG>(InterlockedIncrement(&refs_)); } \
    STDMETHODIMP_(ULONG) Release() override { LONG n = InterlockedDecrement(&refs_); if (!n) delete this; return static_cast<ULONG>(n); }
class Adapter {
public:
    Adapter();
    ~Adapter();
    void add_ref() { InterlockedIncrement(&refs_); }
    void release() { if (!InterlockedDecrement(&refs_)) delete this; }
    NTSTATUS start();
    void stop();
    NTSTATUS state(Stream* stream, KSSTATE state);
    void tick();
    KSPIN_LOCK lock;
    Stream* streams[2]{};
private:
    LONG refs_ = 1;
    bool stopped_ = false;
    PEX_TIMER timer_ = nullptr;
    // Called with lock held, including at a stream RUN boundary.
    void advance_locked();
    mixlive::CableTransport transport_;
    mixlive::FrameClock clock_;
};
class Stream final : public IMiniportWaveRTStreamNotification {
public:
    Stream(Adapter* adapter, PPORTWAVERTSTREAM port, bool capture);
    ~Stream();
    MIXLIVE_UNKNOWN
    IMP_IMiniportWaveRTStream;
    IMP_IMiniportWaveRTStreamNotification;
    NTSTATUS allocate(ULONG notifications, ULONG requested, PMDL* mdl, ULONG* actual, ULONG* offset, MEMORY_CACHING_TYPE* cache);
    // All following fields are protected by adapter->lock.
    Adapter* adapter;
    PPORTWAVERTSTREAM port;
    bool capture;
    KSSTATE state = KSSTATE_STOP;
    PMDL mdl = nullptr;
    BYTE* buffer = nullptr;
    ULONG frames = 0, notifications = 0;
    uint64_t position = 0;
    PKEVENT events[8]{};
    void notify(uint64_t previous);
private:
    LONG refs_ = 1;
};
class Wave final : public IMiniportWaveRT {
public:
    Wave(Adapter* adapter, bool capture);
    ~Wave();
    MIXLIVE_UNKNOWN
    IMP_IMiniportWaveRT;
private:
    LONG refs_ = 1;
    Adapter* adapter_;
    bool capture_;
};
class Topology final : public IMiniportTopology {
public:
    explicit Topology(bool capture) : capture_(capture) {}
    MIXLIVE_UNKNOWN
    IMP_IMiniportTopology;
private:
    LONG refs_ = 1;
    bool capture_;
};
PCFILTER_DESCRIPTOR* WaveDescriptor(bool capture);
PCFILTER_DESCRIPTOR* TopologyDescriptor(bool capture);
bool ValidFormat(PKSDATAFORMAT format);
