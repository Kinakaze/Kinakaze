#include <cassert>
#include <chrono>
#include <cstdio>
#include <future>
#include <thread>

int main() {
    using namespace std::chrono_literals;
    for (int iteration = 0; iteration < 50; ++iteration) {
        std::promise<int> promise;
        auto result = promise.get_future().share();
        assert(result.wait_for(1ms) == std::future_status::timeout);
        std::thread([completion = std::move(promise), iteration]() mutable {
            completion.set_value(iteration);
        }).detach();
        assert(result.wait_for(2s) == std::future_status::ready);
        assert(result.get() == iteration);
    }
    std::puts("CPP_SHARED_FUTURE_OK");
    std::fflush(stdout);
    for (int iteration = 0; iteration < 20; ++iteration) {
        std::promise<int> promise;
        auto result = promise.get_future();
        std::thread([completion = std::move(promise), iteration]() mutable {
            completion.set_value_at_thread_exit(iteration);
        }).detach();
        assert(result.wait_for(2s) == std::future_status::ready);
        assert(result.get() == iteration);
    }
    std::puts("CPP_THREAD_EXIT_FUTURE_OK");
    std::fflush(stdout);
    auto asynchronous = std::async(std::launch::async, [] { return 42; });
    assert(asynchronous.get() == 42);
    std::future<int> abandoned;
    {
        std::promise<int> promise;
        abandoned = promise.get_future();
    }
    bool broken = false;
    try {
        abandoned.get();
    } catch (const std::future_error &error) {
        broken = error.code() == std::make_error_code(std::future_errc::broken_promise);
    }
    assert(broken);
    std::puts("CPP_FUTURES_OK");
}
