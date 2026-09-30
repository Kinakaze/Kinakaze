# Binary80 math and complex ABI support

`sources.json` records the upstream URLs and SHA-256 hashes. The musl v1.2.5
sources retain their original license notices; `catrigl.c` comes from FreeBSD
14.3 and retains its BSD license. The packaged notices live in
`../../third-party-notices.txt`.

Local adaptations:

- `catrigl.c` includes `complex_impl.h` in place of FreeBSD platform headers.
- `expm1l.c` scales the final fraction directly for large positive exponents,
  avoiding premature overflow near the binary80 upper boundary.
- `libm.h` supplies namespaced kernels, binary80 layout, and x87 helpers.
- `bridge.c` uses pointer arguments, temporarily enables 64-bit x87 precision,
  restores the caller's control word, and keeps Windows complex return
  conventions inside C. Rust wrappers supply the actual System V ABI.

The single/double complex entry points promote their inputs to binary80 and
round back to their public format. Long-double arguments retain their full
significand and exponent range. The inverse functions use FreeBSD's scaled
branch handling; trigonometric range reduction uses musl's full binary80
kernels, including large arguments.

Regression: `python tests/guest/run-complex-math.py --dist <dist> --worker <worker>
--link-dir <elf-imports>`. The ELF checks versioned imports, all three complex
calling conventions, branch cuts, signed zero, infinities, tiny arguments,
extended range/precision, errno, and x87 control-word preservation.
