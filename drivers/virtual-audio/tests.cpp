#include "core.hpp"
#include <array>
#include <algorithm>
#include <cstdio>
#include <cstdlib>

using namespace mixlive;
#define REQUIRE(x) do { if (!(x)) { std::fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #x); std::exit(1); } } while (0)

static Frame sample(uint64_t position, uint32_t salt) {
    Frame frame{};
    const uint32_t values[2] = {
        static_cast<uint32_t>(position) * 2654435761u + salt,
        ~(static_cast<uint32_t>(position) * 2246822519u + salt)
    };
    for (uint32_t channel = 0; channel < 2; ++channel)
        for (uint32_t b = 0; b < 4; ++b)
            frame.bytes[4 * channel + b] = static_cast<uint8_t>(values[channel] >> (8 * b));
    return frame;
}

static bool equal(const uint8_t* bytes, const Frame& frame) {
    return std::equal(frame.bytes, frame.bytes + frame_bytes, bytes);
}

struct TestCable {
    static constexpr uint32_t size = 257;
    std::array<uint8_t, size * frame_bytes> input{};
    std::array<uint8_t, size * frame_bytes> output{};
    Ring render{input.data(), size, 251};
    Ring capture{output.data(), size, 173};
    CableTransport cable;
    uint64_t total = 0;

    void check(uint32_t count, uint32_t salt) {
        const auto rp = render.position, cp = capture.position;
        for (uint32_t i = 0; i < count; ++i) {
            const Frame frame = sample(total + i, salt);
            std::copy(frame.bytes, frame.bytes + frame_bytes,
                input.data() + ((rp + i) % size) * frame_bytes);
        }
        REQUIRE(cable.advance(count, render, capture));
        REQUIRE(render.position == rp + count);
        REQUIRE(capture.position == cp + count);
        for (uint32_t i = 0; i < count; ++i) {
            const auto index = total + i;
            const Frame expected = index < delay_frames ? Frame{} : sample(index - delay_frames, salt);
            REQUIRE(equal(output.data() + ((cp + i) % size) * frame_bytes, expected));
        }
        total += count;
    }
};

int main() {
    TestCable call, broadcast, returned;
    for (uint32_t i = 0; i < 4000; ++i) {
        // Different source sequences and buffer positions must never bleed into each other.
        call.check(1 + i % 113, 0x13579bdfu);
        broadcast.check(1 + (i * 3) % 211, 0x2468ace0u);
        returned.check(1 + (i * 7) % 233, 0xabcdef01u);
    }

    // Capture alone is silence; render alone is discarded. A RUN boundary resets history.
    Ring absent{};
    call.cable.reset();
    auto cp = call.capture.position;
    call.output.fill(0xff);
    REQUIRE(call.cable.advance(137, absent, call.capture));
    for (uint32_t i = 0; i < 137; ++i)
        REQUIRE(equal(call.output.data() + ((cp + i) % call.size) * frame_bytes, Frame{}));
    REQUIRE(absent.position == 0);
    auto rp = call.render.position;
    REQUIRE(call.cable.advance(59, call.render, absent));
    REQUIRE(call.render.position == rp + 59 && absent.position == 0);
    call.cable.reset();
    call.total = 0;
    call.check(127, 0x01234567u);

    // A whole DMA wrap, or an excessive tick, discards history and clears captured bytes.
    for (uint64_t lost : {uint64_t(call.size), uint64_t(max_tick_frames + 1)}) {
        rp = call.render.position;
        cp = call.capture.position;
        call.output.fill(0xff);
        REQUIRE(!call.cable.advance(lost, call.render, call.capture));
        REQUIRE(call.render.position == rp + lost && call.capture.position == cp + lost);
        REQUIRE(std::all_of(call.output.begin(), call.output.end(), [](uint8_t b) { return b == 0; }));
        call.total = 0;
        call.check(127, 0xfedcba98u);
    }
    // Reset/gap handling of Call must not disturb Broadcast or Return.
    broadcast.check(193, 0x2468ace0u);
    returned.check(229, 0xabcdef01u);

    // DAW/Return keeps receiving while an unrelated output is stopped, then its
    // own receiver disconnects. No pre-disconnect Return samples may be retained.
    returned.cable.reset();
    returned.output.fill(0xff);
    cp = returned.capture.position;
    REQUIRE(returned.cable.advance(137, absent, returned.capture));
    for (uint32_t i = 0; i < 137; ++i)
        REQUIRE(equal(returned.output.data() + ((cp + i) % returned.size) * frame_bytes, Frame{}));
    rp = returned.render.position;
    REQUIRE(returned.cable.advance(59, returned.render, absent));
    REQUIRE(returned.render.position == rp + 59);
    returned.cable.reset();
    returned.total = 0;
    returned.check(127, 0x13572468u);
    call.check(127, 0xfedcba98u);
    broadcast.check(127, 0x2468ace0u);

    FrameClock clock;
    constexpr uint64_t frequency = 10000019;
    clock.reset(0, frequency);
    uint64_t now = 0, total = 0;
    for (uint64_t i = 1; i < 2000000; ++i) {
        now += 731 + i % 97;
        total += clock.advance(now);
    }
    REQUIRE(total == (now / frequency) * sample_rate + (now % frequency) * sample_rate / frequency);
    clock.reset(0, frequency);
    REQUIRE(clock.advance(frequency * 86400) == 86400ull * sample_rate);
    clock.reset(100, frequency);
    REQUIRE(clock.advance(99) == 0);
    clock.reset(0, 0);
    REQUIRE(clock.advance(123456) == 0);
    std::puts("PASS: exact PCM32 stereo transfer, ring wrap, independent Call/Broadcast/Return, RUN reset, absent producer/consumer, discontinuity and QPC frame accounting.");
}
