"""Real Linux io_uring ABI, memory advice and pthread mask regressions."""
import argparse
import json
from pathlib import Path
from init_pool import InitPool, distribution_hashes

CASES = {
    'null-string-format': ('NullStringFormatProbe', 'NULL_STRING_PRECISION_WIDTH_CHECKPOINT_FORMAT_OK'),
    'empty-gnu-hash': ('EmptyGnuHashProbe', 'EMPTY_GNU_HASH_IMPORTS_OK'),
    'readonly-create': ('ReadonlyCreateProbe', 'READONLY_CREATE_DOT_PARENT_FD_ACCESS_OK'),
    'c11-threads': ('C11ThreadProbe', 'C11_THREADS_ONCE_MUTEX_CONDITION_TIMEOUT_JOIN_OK'),
    'runtime-limits': ('RuntimeLimitProbe', 'RUNTIME_IOV_MAX_CLOSEFROM_BOUNDARY_ERRNO_OK'),
    'async-writeback': ('AsyncWritebackProbe', 'ASYNC_WRITEBACK_EXIT_RENAME_UNLINK_OFFSET_ERRORS_OK'),
    'getsubopt': ('GetSuboptProbe', 'GETSUBOPT_MATCH_UNKNOWN_EMPTY_CURSOR_OK'),
    'database-abi': ('DatabaseAbiProbe', 'DATABASE_ABI_QUAD_CPU_CLOCK_EXCEPTION_FLAGS_OK'),
    'mapped-file-growth': ('MappedFileGrowthProbe', 'MAPPED_FILE_PWRITE_GROWTH_RETAINED_INODE_OK'),
    'ioctl-request-width': ('IoctlRequestWidthProbe', 'IOCTL_REQUEST_WIDTH_PTY_OK'),
    'raw-termios-layout': ('RawTermiosLayoutProbe', 'RAW_TERMIOS_LAYOUT_GUARD_SPEEDS_OK'),
    'proc-fd-cwd-exec': ('ProcFdCwdExecProbe', 'PROC_FD_CWD_CLOSE_RENAME_EXEC_OK'),
    'proc-fd-unlink': ('ProcFdUnlinkProbe', 'PROC_FD_UNLINK_RENAME_OPEN_SYMLINK_OK'),
    'agent-spawn-fd-churn': ('AgentSpawnFdChurnProbe', 'AGENT_SPAWN_CONCURRENT_FD_CHURN_OK'),
    'epoll-signal-mask': ('EpollSignalMaskProbe', 'EPOLL_PWAIT_TEMPORARY_MASK_RESTORE_FAULT_OK'),
    'large-exec-arguments': ('LargeExecArgumentProbe', 'LARGE_EXEC_ARGUMENTS_OK'),
    'long-proc-cmdline': ('LongCmdlineProbe', 'LONG_PROC_CMDLINE_OK'),
    'wide-memory-fortify': ('WideMemoryFortifyProbe', 'WIDE_MEMORY_FORTIFY_OK'),
    'proc-fd-mkdir': ('ProcFdMkdirProbe', 'PROC_FD_MKDIR_LIVE_DIRECTORY_OK'),
    'raw-mincore': ('RawMincoreProbe', 'RAW_MINCORE_RESIDENCY_BOUNDS_HOLES_OK'),
    'uring': ('LinuxUringProbe', 'LINUX_URING_MMAP_ASYNC_RW_VECTORS_OVERFLOW_WAKE_OK'),
    'memory-advice': ('MadvisePolicyProbe', 'MADVISE_DUMP_FORK_ZERO_RESTORE_OK'),
    'pthread-mask': ('PthreadSignalMaskProbe', 'PTHREAD_MASK_INHERIT_SIGWAIT_OK'),
    'pty-wait': ('PtyMixedWaitProbe', 'PTY_TCP_NATIVE_WAKE_HANGUP_OK'),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--only', nargs='+', choices=CASES)
    parser.add_argument('--repeat', type=int, default=1)
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error('repeat must be positive')
    root, dist, output = (p.resolve() for p in (args.root, args.dist, args.output))
    output.mkdir(parents=True, exist_ok=True)
    report = dict(root=str(root), dist=str(dist), sha256=distribution_hashes(dist), results=[])
    for iteration in range(args.repeat):
        for name in args.only or CASES:
            probe, marker = CASES[name]
            source = (Path(__file__).resolve().parents[1] / 'tests/guest' / (probe + '.py')).read_text(encoding='utf-8')
            try:
                with InitPool(root, dist, output / f'{name}-{iteration}', size=1, timeout=90) as pool:
                    row = pool.run(['/usr/bin/python3', '-c', source], expect=[marker])
            except Exception as error:
                row = dict(status='failed', error=str(error))
            row.update(name=name, iteration=iteration)
            report['results'].append(row)
            print(json.dumps(dict(name=name, iteration=iteration, status=row['status'])), flush=True)
            report['passed'] = all(row['status'] == 'passed' for row in report['results'])
            (output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
