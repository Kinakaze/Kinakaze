from elf_probe import run

run('rlimit_probe.c', 'rlimit-probe', 'RLIMIT_POLL_OK')
