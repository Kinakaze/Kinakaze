#include <assert.h>
#include <pulse/pulseaudio.h>
#include <stdint.h>
#include <stdio.h>
#include <unistd.h>

static int latency_calls, completions, release_in_callback;
static void latency_changed(pa_stream *stream, void *userdata) {
    assert(userdata == &latency_calls);
    latency_calls++;
    if (release_in_callback) pa_stream_unref(stream);
}
static void completed(pa_stream *stream, int success, void *userdata) {
    assert(success && userdata == &completions);
    assert(pa_stream_get_state(stream) == PA_STREAM_READY);
    completions++;
}
static void done(pa_operation *operation) {
    assert(operation && pa_operation_get_state(operation) == PA_OPERATION_DONE);
    pa_operation_unref(operation);
}
static uint64_t position(pa_stream *stream) {
    pa_usec_t time = UINT64_MAX;
    assert(!pa_stream_get_time(stream, &time));
    return time;
}
static void wait_until(pa_stream *stream, uint64_t target) {
    for (int i = 0; i < 100 && position(stream) < target; i++) usleep(20000);
    assert(position(stream) == target);
}
int main(void) {
    pa_mainloop *loop = pa_mainloop_new();
    assert(loop);
    pa_context *context = pa_context_new(pa_mainloop_get_api(loop), "device timing probe");
    assert(context && !pa_context_connect(context, NULL, 0, NULL));
    pa_sample_spec spec = {PA_SAMPLE_S16LE, 48000, 1};
    pa_stream *stream = pa_stream_new(context, "silent playback", &spec, NULL);
    assert(stream);
    pa_usec_t unchanged = 123;
    assert(pa_stream_get_time(stream, &unchanged) == -PA_ERR_BADSTATE && unchanged == 123);
    assert(pa_context_errno(context) == PA_ERR_BADSTATE);
    assert(!pa_stream_update_timing_info(stream, completed, &completions) && !completions);
    assert(!pa_stream_connect_playback(stream, NULL, NULL, PA_STREAM_START_CORKED, NULL, NULL));
    assert(pa_stream_ref(stream) == stream);
    pa_stream_unref(stream);
    assert(position(stream) == 0);
    usleep(30000);
    assert(position(stream) == 0);
    static int16_t silence[24000];
    assert(!pa_stream_write(stream, silence, sizeof(silence), NULL, 0, PA_SEEK_RELATIVE));
    pa_usec_t pending;
    assert(!pa_stream_get_latency(stream, &pending, NULL) && pending == 500000);
    usleep(30000);
    assert(position(stream) == 0);
    pa_stream_set_latency_update_callback(stream, latency_changed, &latency_calls);
    done(pa_stream_update_timing_info(stream, completed, &completions));
    assert(latency_calls == 1 && completions == 1);
    done(pa_stream_cork(stream, 0, NULL, NULL));
    for (int i = 0; i < 50 && position(stream) == 0; i++) usleep(10000);
    uint64_t playing = position(stream);
    assert(playing > 0 && playing < 500000);
    done(pa_stream_cork(stream, 1, NULL, NULL));
    uint64_t paused = position(stream);
    usleep(50000);
    assert(position(stream) == paused);
    int negative = 1;
    assert(!pa_stream_get_latency(stream, &pending, &negative) && !negative);
    assert(paused + pending <= 500000 && paused + pending >= 499999);
    done(pa_stream_cork(stream, 0, NULL, NULL));
    wait_until(stream, 500000);
    assert(!pa_stream_get_latency(stream, &pending, NULL) && !pending);
    done(pa_stream_cork(stream, 1, NULL, NULL));
    assert(!pa_stream_write(stream, silence, sizeof(silence), NULL, 0, PA_SEEK_RELATIVE));
    done(pa_stream_flush(stream, NULL, NULL));
    assert(position(stream) == 1000000);
    assert(!pa_stream_get_latency(stream, &pending, NULL) && !pending);
    assert(!pa_stream_write(stream, silence, 4800, NULL, 0, PA_SEEK_RELATIVE));
    usleep(30000);
    assert(position(stream) == 1000000);
    done(pa_stream_cork(stream, 0, NULL, NULL));
    wait_until(stream, 1050000);
    assert(pa_stream_get_latency(stream, NULL, NULL) == -PA_ERR_INVALID);
    release_in_callback = 1;
    done(pa_stream_update_timing_info(stream, completed, &completions));
    assert(latency_calls == 2 && completions == 2);
    pa_context_disconnect(context);
    pa_context_unref(context);
    pa_mainloop_free(loop);
    puts("PULSE_NATIVE_TIME_LATENCY_CORK_FLUSH_REFS_OK");
    return 0;
}
