// Derived from Cableloom, Copyright (c) 2026 Cableloom contributors.
// Source revision 130f811310082fd6966b17e76f0ac212e129dbf9; see LICENSE in this directory.
#pragma once
#include <stdint.h>
namespace mixlive {
inline constexpr uint32_t sample_rate = 48000;
inline constexpr uint32_t frame_bytes = 8;
inline constexpr uint32_t delay_frames = 96;
inline constexpr uint32_t max_tick_frames = 480;
inline constexpr uint32_t max_buffer_frames = 4800;
struct Frame { uint8_t bytes[frame_bytes]; };
static_assert(sizeof(Frame) == frame_bytes, "PCM32 stereo frame");

struct Ring {
    uint8_t* bytes = nullptr;
    uint32_t frames = 0;
    uint64_t position = 0;
    bool active() const { return bytes && frames; }
    uint8_t* current() const { return bytes + (position % frames) * frame_bytes; }
};

// One independent instance per PnP adapter, serialized by the adapter spin lock.
// Render is discarded when the receiver is not RUN; no voice is retained for a later receiver.
// A capture-only stream receives silence. No processing, allocation or kernel/user IOCTL.
class CableTransport {
public:
    void reset() {
        cursor_ = 0;
        for (auto& frame : history_) frame = {};
    }
    // Returns false for a timing discontinuity, rather than rereading a wrapped DMA ring.
    // All work is bounded by max_tick_frames or the validated max_buffer_frames.
    bool advance(uint64_t elapsed, Ring& render, Ring& capture) {
        const bool r = render.active(), c = capture.active();
        if (!r && !c) return true;
        if (elapsed > max_tick_frames || (r && elapsed >= render.frames) ||
            (c && elapsed >= capture.frames)) {
            reset();
            if (r) render.position += elapsed;
            if (c) {
                for (uint32_t i = 0; i < capture.frames * frame_bytes; ++i) capture.bytes[i] = 0;
                capture.position += elapsed;
            }
            return false;
        }
        for (uint64_t i = 0; i < elapsed; ++i) {
            if (c) {
                Frame output{};
                if (r) {
                    output = history_[cursor_];
                    const uint8_t* input = render.current();
                    for (uint32_t b = 0; b < frame_bytes; ++b) history_[cursor_].bytes[b] = input[b];
                    if (++cursor_ == delay_frames) cursor_ = 0;
                }
                uint8_t* target = capture.current();
                for (uint32_t b = 0; b < frame_bytes; ++b) target[b] = output.bytes[b];
                ++capture.position;
            }
            if (r) ++render.position;
        }
        return true;
    }
private:
    Frame history_[delay_frames]{};
    uint32_t cursor_ = 0;
};

// Count elapsed QPC time, including fractional frame carry, not timer callback count.
class FrameClock {
public:
    void reset(uint64_t now, uint64_t frequency) { last_ = now; frequency_ = frequency; carry_ = 0; }
    uint64_t advance(uint64_t now) {
        if (!frequency_ || now < last_) { last_ = now; carry_ = 0; return 0; }
        uint64_t delta = now - last_;
        last_ = now;
        uint64_t partial = (delta % frequency_) * sample_rate + carry_;
        uint64_t frames = (delta / frequency_) * sample_rate + partial / frequency_;
        carry_ = partial % frequency_;
        return frames;
    }
private:
    uint64_t last_ = 0, frequency_ = 0, carry_ = 0;
};
}
