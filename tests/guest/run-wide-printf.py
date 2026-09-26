from elf_probe import run

run('WidePrintfProbe.c', 'wide-printf-probe', 'WIDE_PRINTF_ABI_STREAMS_OK',
    cflags=('-include', 'tests/guest/WideFormatProbe.c'),
    root_files=('usr/lib/locale/C.utf8/LC_CTYPE',))
