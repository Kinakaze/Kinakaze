"""Exercise real Python SemLock users under fork, spawn and forkserver."""
import multiprocessing as mp


def worker(lock, counter, ready, queue):
    with lock:
        counter.value += 1
    ready.release()
    queue.put(counter.value)


if __name__ == '__main__':
    for method in mp.get_all_start_methods():
        context = mp.get_context(method)
        lock = context.Lock()
        counter = context.Value('i', 0)
        ready = context.Semaphore(0)
        queue = context.Queue()
        children = [context.Process(target=worker, args=(lock, counter, ready, queue))
                    for _ in range(4)]
        for child in children:
            child.start()
        for child in children:
            assert ready.acquire(timeout=10), method
            assert 1 <= queue.get(timeout=10) <= 4, method
            child.join(10)
            assert child.exitcode == 0, (method, child.exitcode)
            child.close()
        assert counter.value == 4, method
        queue.close()
        queue.join_thread()
        print('MULTIPROCESSING_OK', method, flush=True)
